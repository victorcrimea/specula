//! The stream Worker (init.md 2.4): the WebSocket, the match's segments and
//! the playback clock. The page is only handed wire messages to apply, so
//! its scene moves forward one turn at a time, or is replaced by a keyframe
//! on a seek.
//!
//! Page to worker: u8 command, f64 argument. Worker to page: u8 `POST_SCENE`,
//! f64 real milliseconds per turn (0: live, measure the pace), then a wire
//! message; or u8 `POST_STATUS`, u32 first turn, u32 newest turn, u32
//! playhead turn, u8 `STATUS_*` flags, f32 speed, u64 bytes of live
//! messages and u64 bytes of segments received since the worker started.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::rc::Weak;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen_futures::JsFuture;
use web_sys::BinaryType;
use web_sys::DedicatedWorkerGlobalScope;
use web_sys::MessageEvent;
use web_sys::Response;
use web_sys::WebSocket;
use web_sys::WorkerGlobalScope;

use crate::TURN_MS;
use crate::stream;
use crate::stream::Scene;

pub const POST_SCENE: u8 = 1;
pub const POST_STATUS: u8 = 2;
pub const STATUS_LIVE: u8 = 1;
pub const STATUS_PAUSED: u8 = 2;
/// Shows the keyframe at or before the turn; for dragging the timeline.
pub const COMMAND_SCRUB: u8 = 1;
/// Shows exactly the turn.
pub const COMMAND_SEEK: u8 = 2;
pub const COMMAND_PLAY: u8 = 3;
pub const COMMAND_PAUSE: u8 = 4;
/// In game time: 1 is one game second per second.
pub const COMMAND_SPEED: u8 = 5;
pub const COMMAND_LIVE: u8 = 6;

/// Only a few segments around the playhead are decoded at a time
/// (init.md 2.4).
const CACHED_SEGMENTS: usize = 4;
/// About 50 s of live deltas at normal speed: playback a little behind live
/// needs no fetch, even of the segment still being written.
const RECENT_DELTAS: usize = 256;
/// After a longer stall, such as a throttled background tab, playback
/// continues from now instead of racing through the missed turns.
const MAX_BEHIND_MS: f64 = 1000.0;
const RETRY_MS: f64 = 1000.0;

struct Segment {
    index: u32,
    /// (turn, message); the first is the keyframe.
    messages: Vec<(u32, Vec<u8>)>,
}

enum Next {
    Message(u32, Vec<u8>),
    /// A segment is being fetched; its arrival resumes playback.
    Waiting,
}

struct Playback {
    this: Weak<RefCell<Playback>>,
    scope: DedicatedWorkerGlobalScope,
    socket: WebSocket,
    /// Calls `resume`, for timers.
    wake: js_sys::Function,
    /// Every live message applied: the newest state of the match.
    live_scene: Scene,
    newest_turn: u32,
    /// Live deltas without a gap between them; a live keyframe breaks the
    /// chain and clears it.
    recent: VecDeque<(u32, Vec<u8>)>,
    /// Least recently used first, all of the current match.
    segments: Vec<Segment>,
    fetching: Option<u32>,
    live: bool,
    paused: bool,
    /// The timeline is being dragged: keyframes only, no playback.
    scrubbing: bool,
    /// The segment whose keyframe the page shows while scrubbing.
    scrub_index: Option<u32>,
    speed: f32,
    /// The last turn posted to the page while not live.
    playhead: u32,
    /// When the next turn is due, on the performance clock.
    next_due: f64,
    /// A seek waiting for its segment: turn, exact.
    pending_seek: Option<(u32, bool)>,
    timer: Option<i32>,
    /// Received since the worker started, for the page's statistics.
    live_bytes: u64,
    segment_bytes: u64,
}

impl Playback {
    fn now(&self) -> f64 {
        self.scope
            .performance()
            .map_or_else(js_sys::Date::now, |performance| performance.now())
    }

    fn turn_ms(&self) -> f64 {
        TURN_MS / f64::from(self.speed)
    }

    fn has_match(&self) -> bool {
        self.live_scene.map_size != 0
    }

    fn index_of(&self, turn: u32) -> u32 {
        turn.saturating_sub(self.live_scene.first_turn) / self.live_scene.segment_turns
    }

    fn on_live(&mut self, message: Vec<u8>) {
        let previous_match = self.live_scene.match_id;
        if let Err(error) = self.live_scene.apply(&message, 1.0) {
            // Every later delta would build on a half-patched scene.
            web_sys::console::error_1(&format!("bad stream message: {error}").into());
            let _ = self.socket.close();
            return;
        }
        let turn = self.live_scene.turn;
        self.newest_turn = turn;
        if stream::is_keyframe(&message) {
            self.recent.clear();
            if self.live_scene.match_id != previous_match {
                // Nothing cached or shown belongs to the new match.
                self.segments.clear();
                self.go_live(false);
            }
        } else {
            if self.recent.len() == RECENT_DELTAS {
                self.recent.pop_front();
            }
            self.recent.push_back((turn, message.clone()));
        }
        if self.live {
            self.post_scene(0.0, &message);
        }
        self.post_status();
    }

    fn on_command(&mut self, command: u8, value: f64) {
        match command {
            COMMAND_SCRUB => self.seek(value as u32, false),
            COMMAND_SEEK => self.seek(value as u32, true),
            COMMAND_PLAY => self.play(),
            COMMAND_PAUSE => self.pause(),
            COMMAND_SPEED => self.set_speed(value as f32),
            COMMAND_LIVE => self.go_live(true),
            _ => {}
        }
    }

    /// `jump`: the page is behind and needs the live state as a keyframe.
    fn go_live(&mut self, jump: bool) {
        let was_live = self.live;
        self.live = true;
        self.paused = false;
        self.scrubbing = false;
        self.scrub_index = None;
        self.pending_seek = None;
        // Live runs at the match's own pace.
        self.speed = self.live_scene.game_speed;
        self.cancel_timer();
        if jump && !was_live && self.has_match() {
            self.post_scene(0.0, &self.live_scene.encode_keyframe());
        }
        self.post_status();
    }

    fn pause(&mut self) {
        if self.live {
            self.live = false;
            self.playhead = self.newest_turn;
        }
        self.paused = true;
        self.cancel_timer();
        self.post_status();
    }

    fn play(&mut self) {
        if !self.paused {
            return;
        }
        self.paused = false;
        self.next_due = self.now() + self.turn_ms();
        self.post_status();
        self.schedule(self.turn_ms());
    }

    fn set_speed(&mut self, speed: f32) {
        // Live plays at the match's pace, so the page offers no speed there.
        if self.live || !speed.is_finite() || speed <= 0.0 {
            return;
        }
        self.speed = speed;
        self.next_due = self.now() + self.turn_ms();
        self.post_status();
        if !self.paused {
            self.schedule(self.turn_ms());
        }
    }

    fn seek(&mut self, turn: u32, exact: bool) {
        if !self.has_match() {
            return;
        }
        let turn = turn.clamp(self.live_scene.first_turn, self.newest_turn);
        self.pending_seek = None;
        if exact && turn >= self.newest_turn {
            self.go_live(true);
            return;
        }
        self.live = false;
        self.scrubbing = !exact;
        self.cancel_timer();
        let index = self.index_of(turn);
        let Some(position) = self.cached(index) else {
            self.pending_seek = Some((turn, exact));
            self.request_segment(index);
            self.post_status();
            return;
        };
        if !exact {
            if self.scrub_index != Some(index) {
                self.scrub_index = Some(index);
                let (keyframe_turn, keyframe) = &self.segments[position].messages[0];
                self.playhead = *keyframe_turn;
                self.post_scene(0.0, keyframe);
            }
            self.post_status();
            return;
        }
        self.scrub_index = None;
        let mut scene = Scene::default();
        if let Err(error) = self.rebuild(&mut scene, position, turn) {
            web_sys::console::error_1(&format!("bad segment {index}: {error}").into());
            self.segments.remove(position);
            self.post_status();
            return;
        }
        self.playhead = scene.turn;
        self.post_scene(self.turn_ms(), &scene.encode_keyframe());
        self.next_due = self.now() + self.turn_ms();
        self.post_status();
        if !self.paused {
            self.schedule(self.turn_ms());
        }
    }

    /// Applies the segment's keyframe and its deltas up to `turn`, then the
    /// recent live deltas if the segment ends before it.
    fn rebuild(&self, scene: &mut Scene, position: usize, turn: u32) -> Result<(), &'static str> {
        let messages = &self.segments[position].messages;
        let keyframe_turn = messages[0].0;
        scene.apply(&messages[0].1, 1.0)?;
        for (message_turn, message) in &messages[1..] {
            // The delta into the keyframe's turn is for playback crossing
            // into this segment; the keyframe already holds that turn.
            if *message_turn == keyframe_turn {
                continue;
            }
            if *message_turn > turn {
                return Ok(());
            }
            scene.apply(message, 1.0)?;
        }
        if self
            .recent
            .front()
            .is_some_and(|(front, _)| *front <= scene.turn + 1)
        {
            for (message_turn, message) in &self.recent {
                if *message_turn > turn {
                    break;
                }
                if *message_turn > scene.turn {
                    scene.apply(message, 1.0)?;
                }
            }
        }
        Ok(())
    }

    fn resume(&mut self) {
        self.timer = None;
        match self.pending_seek.take() {
            Some((turn, exact)) => self.seek(turn, exact),
            None => self.tick(),
        }
    }

    fn tick(&mut self) {
        if self.live || self.paused || self.scrubbing || self.pending_seek.is_some() {
            return;
        }
        let now = self.now();
        let turn_ms = self.turn_ms();
        if now - self.next_due > MAX_BEHIND_MS {
            self.next_due = now;
        }
        let mut moved = false;
        while self.next_due <= now {
            if self.playhead >= self.newest_turn {
                // Caught up: the page is at the newest turn, so the next
                // live delta applies to it as it is.
                self.go_live(false);
                return;
            }
            match self.next_message(self.playhead) {
                Next::Message(turn, message) => {
                    self.post_scene(turn_ms, &message);
                    self.playhead = turn;
                    self.next_due += turn_ms;
                    moved = true;
                }
                Next::Waiting => {
                    self.next_due = now;
                    if moved {
                        self.post_status();
                    }
                    return;
                }
            }
        }
        if moved {
            self.post_status();
        }
        self.schedule(self.next_due - now);
    }

    /// The first message after turn `after`, from the recent live deltas or
    /// a segment.
    fn next_message(&mut self, after: u32) -> Next {
        if self
            .recent
            .front()
            .is_some_and(|(front, _)| *front <= after + 1)
            && let Some((turn, message)) = self.recent.iter().find(|(turn, _)| *turn > after)
        {
            return Next::Message(*turn, message.clone());
        }
        let newest_index = self.index_of(self.newest_turn);
        let mut index = self.index_of(after + 1);
        while index <= newest_index {
            let Some(position) = self.cached(index) else {
                self.request_segment(index);
                return Next::Waiting;
            };
            // Skipping the keyframe leaves the delta into its turn, which is
            // how playback crosses a segment boundary without a jump.
            if let Some((turn, message)) = self.segments[position].messages[1..]
                .iter()
                .find(|(turn, _)| *turn > after)
            {
                return Next::Message(*turn, message.clone());
            }
            if index == newest_index {
                // The open segment has grown since it was fetched.
                self.segments.remove(position);
                self.request_segment(index);
                return Next::Waiting;
            }
            index += 1;
        }
        Next::Waiting
    }

    /// The segment's position in `segments`, marked most recently used.
    fn cached(&mut self, index: u32) -> Option<usize> {
        let position = self
            .segments
            .iter()
            .position(|segment| segment.index == index)?;
        let segment = self.segments.remove(position);
        self.segments.push(segment);
        Some(self.segments.len() - 1)
    }

    /// One fetch at a time; the one in flight resumes whatever waits, which
    /// then asks for what it still needs.
    fn request_segment(&mut self, index: u32) {
        if self.fetching.is_some() {
            return;
        }
        self.fetching = Some(index);
        let match_id = self.live_scene.match_id;
        let url = format!("/match/{match_id:016x}/seg/{index}");
        let this = self.this.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let fetched = fetch_bytes(&url).await;
            let size = fetched.as_ref().map_or(0, |bytes| bytes.len() as u64);
            let result = fetched
                .and_then(|bytes| stream::read_segment(&bytes).map_err(String::from))
                .and_then(|messages| {
                    messages
                        .into_iter()
                        .map(|message| Ok((stream::message_turn(&message)?, message)))
                        .collect::<Result<Vec<_>, &'static str>>()
                        .map_err(String::from)
                });
            if let Some(playback) = this.upgrade() {
                let mut playback = playback.borrow_mut();
                playback.segment_bytes += size;
                playback.segment_arrived(match_id, index, result);
            }
        });
    }

    fn segment_arrived(
        &mut self,
        match_id: u64,
        index: u32,
        result: Result<Vec<(u32, Vec<u8>)>, String>,
    ) {
        self.fetching = None;
        if match_id != self.live_scene.match_id {
            self.resume();
            return;
        }
        match result {
            Ok(messages) => {
                if self.segments.len() == CACHED_SEGMENTS {
                    self.segments.remove(0);
                }
                self.segments.push(Segment { index, messages });
                self.resume();
            }
            Err(error) => {
                web_sys::console::error_1(&format!("segment {index}: {error}").into());
                self.schedule(RETRY_MS);
            }
        }
    }

    fn schedule(&mut self, delay_ms: f64) {
        self.cancel_timer();
        self.timer = self
            .scope
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                &self.wake,
                delay_ms.max(0.0).ceil() as i32,
            )
            .ok();
    }

    fn cancel_timer(&mut self) {
        if let Some(timer) = self.timer.take() {
            self.scope.clear_timeout_with_handle(timer);
        }
    }

    fn post_scene(&self, turn_ms: f64, message: &[u8]) {
        let mut out = Vec::with_capacity(9 + message.len());
        out.push(POST_SCENE);
        out.extend_from_slice(&turn_ms.to_le_bytes());
        out.extend_from_slice(message);
        self.post(&out);
    }

    fn post_status(&self) {
        let playhead = if self.live {
            self.newest_turn
        } else {
            self.playhead
        };
        let mut flags = 0;
        if self.live {
            flags |= STATUS_LIVE;
        }
        if self.paused {
            flags |= STATUS_PAUSED;
        }
        let mut out = vec![POST_STATUS];
        out.extend_from_slice(&self.live_scene.first_turn.to_le_bytes());
        out.extend_from_slice(&self.newest_turn.to_le_bytes());
        out.extend_from_slice(&playhead.to_le_bytes());
        out.push(flags);
        out.extend_from_slice(&self.speed.to_le_bytes());
        out.extend_from_slice(&self.live_bytes.to_le_bytes());
        out.extend_from_slice(&self.segment_bytes.to_le_bytes());
        self.post(&out);
    }

    fn post(&self, bytes: &[u8]) {
        let array = js_sys::Uint8Array::from(bytes);
        let transfer = js_sys::Array::of1(&array.buffer());
        let _ = self.scope.post_message_with_transfer(&array, &transfer);
    }
}

#[wasm_bindgen]
pub fn worker_main() {
    crate::set_panic_hook();
    if let Err(error) = start() {
        web_sys::console::error_1(&error.into());
    }
}

fn start() -> Result<(), String> {
    let scope: DedicatedWorkerGlobalScope = js_sys::global()
        .dyn_into()
        .map_err(|_| "not a dedicated worker")?;
    let location = scope.location();
    let scheme = match location.protocol().as_str() {
        "https:" => "wss",
        _ => "ws",
    };
    let url = format!("{scheme}://{}/ws", location.host());
    let socket = WebSocket::new(&url).map_err(|_| format!("cannot open {url}"))?;
    socket.set_binary_type(BinaryType::Arraybuffer);

    let playback = Rc::new_cyclic(|this: &Weak<RefCell<Playback>>| {
        let target = this.clone();
        let wake = Closure::<dyn FnMut()>::new(move || {
            if let Some(playback) = target.upgrade() {
                playback.borrow_mut().resume();
            }
        });
        let function = wake.as_ref().unchecked_ref::<js_sys::Function>().clone();
        // The timer target lives as long as the worker.
        wake.forget();
        RefCell::new(Playback {
            this: this.clone(),
            scope: scope.clone(),
            socket: socket.clone(),
            wake: function,
            live_scene: Scene::default(),
            newest_turn: 0,
            recent: VecDeque::new(),
            segments: Vec::new(),
            fetching: None,
            live: true,
            paused: false,
            scrubbing: false,
            scrub_index: None,
            speed: 1.0,
            playhead: 0,
            next_due: 0.0,
            pending_seek: None,
            timer: None,
            live_bytes: 0,
            segment_bytes: 0,
        })
    });

    let receiver = Rc::clone(&playback);
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() else {
            return;
        };
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        let mut playback = receiver.borrow_mut();
        playback.live_bytes += bytes.len() as u64;
        playback.on_live(bytes);
    });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    // The socket lives as long as the worker, so its handler does too.
    on_message.forget();

    let on_close = Closure::<dyn FnMut()>::new(|| crate::log("stream closed"));
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();

    let on_command = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let bytes = js_sys::Uint8Array::new(&event.data()).to_vec();
        let Some((&command, rest)) = bytes.split_first() else {
            return;
        };
        let Some(value) = rest.first_chunk::<8>() else {
            return;
        };
        playback
            .borrow_mut()
            .on_command(command, f64::from_le_bytes(*value));
    });
    scope.set_onmessage(Some(on_command.as_ref().unchecked_ref()));
    on_command.forget();
    Ok(())
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let scope: WorkerGlobalScope = js_sys::global().unchecked_into();
    let response: Response = JsFuture::from(scope.fetch_with_str(url))
        .await
        .map_err(|_| "request failed")?
        .dyn_into()
        .map_err(|_| "not a response")?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let body = response.array_buffer().map_err(|_| "no body")?;
    let buffer = JsFuture::from(body).await.map_err(|_| "body failed")?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}
