//! The stream Worker (init.md 2.4): the WebSocket, the match's segments and
//! the playback clock. The page is only handed wire messages to apply, so
//! its scene moves forward one turn at a time, or is replaced by a keyframe
//! on a seek.
//!
//! When the match ends, its viewers keep it, history and all, while the
//! next match is followed in the background; they move to it when they ask
//! or `SWITCH_DELAY_MS` after the end, whichever is first, or on its first
//! turn if that comes later.
//!
//! The worker alone fetches a match's terrain, since a scene cannot apply
//! the match's messages without its heightmap: live messages wait while it
//! is fetched, and the page is handed it before the first keyframe that
//! names it.
//!
//! Page to worker: u8 command, f64 argument. Worker to page: u8 `POST_SCENE`,
//! f64 real milliseconds per turn (0: live, measure the pace), then a wire
//! message; or u8 `POST_TERRAIN`, u64 terrain id, then the terrain
//! resource; or u8 `POST_STATUS`, u32 first turn, u32 newest turn, u32
//! playhead turn, u8 `STATUS_*` flags, f32 speed, u64 bytes of live
//! messages and u64 bytes of segments received for the shown match, u32 ms
//! until the move to the next match (0 once due; meaningful with
//! `STATUS_ENDED`), u64 bytes of terrain, f64 ms since the counts began and
//! u64 most live bytes in `PEAK_WINDOW_S` seconds.

use std::cell::Cell;
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
use crate::stream::FrameDecoder;
use crate::stream::Ground;
use crate::stream::Scene;

pub const POST_SCENE: u8 = 1;
pub const POST_STATUS: u8 = 2;
pub const POST_TERRAIN: u8 = 3;
pub const STATUS_LIVE: u8 = 1;
pub const STATUS_PAUSED: u8 = 2;
/// The shown match has ended.
pub const STATUS_ENDED: u8 = 4;
/// The next match has started and can be switched to.
pub const STATUS_NEXT_READY: u8 = 8;
/// Shows the keyframe at or before the turn; for dragging the timeline.
pub const COMMAND_SCRUB: u8 = 1;
/// Shows exactly the turn.
pub const COMMAND_SEEK: u8 = 2;
pub const COMMAND_PLAY: u8 = 3;
pub const COMMAND_PAUSE: u8 = 4;
/// In game time: 1 is one game second per second.
pub const COMMAND_SPEED: u8 = 5;
pub const COMMAND_LIVE: u8 = 6;
/// Leaves an ended match for the one being played now.
pub const COMMAND_CURRENT_MATCH: u8 = 7;

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
/// How long viewers stay on a finished match before being moved on.
const SWITCH_DELAY_MS: f64 = 5.0 * 60.0 * 1000.0;
const RECONNECT_MIN_MS: f64 = 1000.0;
const RECONNECT_MAX_MS: f64 = 10_000.0;
/// The live match's, the next one's and one spare.
const KEPT_TERRAINS: usize = 3;
/// Seconds of live bytes the peak rate is taken over, in 1 s steps, so a
/// burst across a window edge still counts whole.
pub const PEAK_WINDOW_S: u64 = 5;

struct Segment {
    index: u32,
    /// (turn, message); the first is the keyframe.
    messages: Vec<(u32, Vec<u8>)>,
}

/// What applying one live message did.
enum Applied {
    /// It went into the live scene, so the page should see it.
    Live(Vec<u8>),
    /// It went elsewhere: the next match, or the end of this one.
    Other,
    /// The stream was dropped.
    Failed,
}

enum Next {
    Message(u32, Vec<u8>),
    /// A segment is being fetched; its arrival resumes playback.
    Waiting,
}

struct Playback {
    this: Weak<RefCell<Playback>>,
    scope: DedicatedWorkerGlobalScope,
    socket: Option<WebSocket>,
    /// The current socket's handlers, dropped with it.
    socket_handlers: Option<SocketHandlers>,
    /// Decodes the frame the stream is in; a join or a new frame replaces
    /// it.
    frame: Option<FrameDecoder>,
    reconnect_ms: f64,
    /// Calls `resume`, for timers.
    wake: js_sys::Function,
    /// Calls `switch_due`, for the switch timer.
    switch_wake: js_sys::Function,
    /// Every live message applied: the newest state of the match.
    live_scene: Scene,
    /// When `live_scene`'s match ended, on the performance clock.
    ended_at: Option<f64>,
    /// The match after an ended one, kept up to date until the viewer
    /// moves to it, and when it ended in turn.
    next_scene: Option<Scene>,
    next_ended_at: Option<f64>,
    switch_timer: Option<i32>,
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
    /// Received for the shown match, for the page's statistics; the next
    /// match's live bytes before the switch are not counted.
    live_bytes: u64,
    segment_bytes: u64,
    terrain_bytes: u64,
    /// When the counts began, on the performance clock.
    counted_since: f64,
    /// Live bytes per whole second since `counted_since`, the last
    /// `PEAK_WINDOW_S` seconds only.
    live_window: VecDeque<(u64, u64)>,
    peak_window_bytes: u64,
    /// Fetched terrains, oldest first: the heightmap and the resource.
    terrains: Vec<(Rc<Ground>, Rc<[u8]>)>,
    /// Live messages waiting, by WebSocket message, from the first
    /// keyframe whose terrain is being fetched on.
    held: Vec<Vec<Vec<u8>>>,
    /// The terrain the page was last handed.
    page_terrain: Cell<u64>,
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

    /// Every later delta would build on a half-patched scene, so the
    /// stream starts again from the keyframe a new connection gets.
    fn drop_stream(&mut self, error: &str) {
        web_sys::console::error_1(&format!("bad stream message: {error}").into());
        if let Some(socket) = &self.socket {
            let _ = socket.close();
        }
    }

    fn ground(&self, id: u64) -> Option<Rc<Ground>> {
        self.terrains
            .iter()
            .find(|(ground, _)| ground.id == id)
            .map(|(ground, _)| Rc::clone(ground))
    }

    /// Gives `scene` the heightmap a keyframe names before it is applied.
    fn prepare(&self, scene: &mut Scene, message: &[u8]) -> Result<(), &'static str> {
        if stream::is_keyframe(message) {
            let id = stream::keyframe_terrain(message)?;
            scene.ground = self.ground(id);
        }
        Ok(())
    }

    fn on_socket(&mut self, bytes: Vec<u8>) {
        let size = bytes.len() as u64;
        self.read_socket(bytes);
        // After the message, so a keyframe that moves to a new match is
        // counted for that match.
        self.count_live(size);
    }

    fn count_live(&mut self, size: u64) {
        self.live_bytes += size;
        let second = ((self.now() - self.counted_since) / 1000.0).max(0.0) as u64;
        match self.live_window.back_mut() {
            Some((at, bytes)) if *at == second => *bytes += size,
            _ => self.live_window.push_back((second, size)),
        }
        while self
            .live_window
            .front()
            .is_some_and(|(at, _)| at + PEAK_WINDOW_S <= second)
        {
            self.live_window.pop_front();
        }
        let window: u64 = self.live_window.iter().map(|(_, bytes)| bytes).sum();
        self.peak_window_bytes = self.peak_window_bytes.max(window);
    }

    /// The statistics are per match.
    fn reset_counts(&mut self) {
        self.live_bytes = 0;
        self.segment_bytes = 0;
        // Fetched before the switch, but for this match.
        self.terrain_bytes = self
            .live_scene
            .ground
            .as_ref()
            .and_then(|ground| self.terrains.iter().find(|(kept, _)| kept.id == ground.id))
            .map_or(0, |(_, bytes)| bytes.len() as u64);
        self.counted_since = self.now();
        self.live_window.clear();
        self.peak_window_bytes = 0;
    }

    fn read_socket(&mut self, bytes: Vec<u8>) {
        let Some((&envelope, body)) = bytes.split_first() else {
            self.drop_stream("empty message");
            return;
        };
        let messages = match envelope {
            stream::ENVELOPE_JOIN => stream::read_join(body).map(|(frame, messages)| {
                self.frame = Some(frame);
                messages
            }),
            stream::ENVELOPE_FRAME => {
                let frame = self.frame.insert(FrameDecoder::new());
                frame.feed(body)
            }
            stream::ENVELOPE_MORE => match &mut self.frame {
                Some(frame) => frame.feed(body),
                None => Err("frame continued before it started"),
            },
            stream::ENVELOPE_ENDED => Ok(vec![body.to_vec()]),
            _ => Err("unknown envelope"),
        };
        match messages {
            Ok(messages) => self.on_live(messages),
            Err(error) => {
                self.frame = None;
                self.drop_stream(error);
            }
        }
    }

    /// `messages` came in one WebSocket message; only a join's keyframe
    /// can name a terrain.
    fn on_live(&mut self, messages: Vec<Vec<u8>>) {
        self.reconnect_ms = RECONNECT_MIN_MS;
        if !self.held.is_empty() {
            self.held.push(messages);
            return;
        }
        if let Some(first) = messages.first()
            && stream::is_keyframe(first)
        {
            match stream::keyframe_terrain(first) {
                Ok(id) if id != 0 && self.ground(id).is_none() => {
                    self.held.push(messages);
                    self.request_terrain(id);
                    return;
                }
                Ok(_) => {}
                Err(error) => {
                    self.drop_stream(error);
                    return;
                }
            }
        }
        self.apply_live(messages);
    }

    fn request_terrain(&mut self, id: u64) {
        let url = format!("/terrain/{id:016x}");
        let this = self.this.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = fetch_bytes(&url).await.and_then(|bytes| {
                let ground = Ground::from_resource(id, &bytes)?;
                Ok((Rc::new(ground), Rc::from(bytes)))
            });
            if let Some(playback) = this.upgrade() {
                playback.borrow_mut().terrain_arrived(id, result);
            }
        });
    }

    fn terrain_arrived(&mut self, id: u64, result: Result<(Rc<Ground>, Rc<[u8]>), String>) {
        let held = std::mem::take(&mut self.held);
        match result {
            Ok(terrain) => {
                if self.terrains.len() == KEPT_TERRAINS {
                    self.terrains.remove(0);
                }
                self.terrain_bytes += terrain.1.len() as u64;
                self.terrains.push(terrain);
                for messages in held {
                    self.on_live(messages);
                }
            }
            // The reconnect's keyframe asks for it again, after the
            // reconnect's growing pause.
            Err(error) => {
                web_sys::console::error_1(&format!("terrain {id:016x}: {error}").into());
                self.drop_stream("no terrain");
            }
        }
    }

    /// A join replays up to a segment of deltas at once; the page gets
    /// the result as one keyframe rather than all of them in one go.
    fn apply_live(&mut self, messages: Vec<Vec<u8>>) {
        let mut shown = Vec::new();
        for message in messages {
            match self.apply_message(message) {
                Applied::Live(message) => shown.push(message),
                Applied::Other => {}
                Applied::Failed => return,
            }
        }
        if shown.is_empty() {
            return;
        }
        if self.live {
            if let [message] = &shown[..] {
                self.post_scene(0.0, message);
            } else {
                self.post_scene(0.0, &self.live_scene.encode_keyframe());
            }
        }
        self.post_status();
    }

    fn apply_message(&mut self, message: Vec<u8>) -> Applied {
        if stream::is_ended(&message) {
            match stream::read_ended(&message) {
                Ok((match_id, since_ms)) => {
                    self.on_ended(match_id, since_ms);
                    return Applied::Other;
                }
                Err(error) => {
                    self.drop_stream(error);
                    return Applied::Failed;
                }
            }
        }
        if self.ended_at.is_some() {
            if let Some(mut next) = self.next_scene.take() {
                let applied = self
                    .prepare(&mut next, &message)
                    .and_then(|()| next.apply(&message, 1.0));
                self.next_scene = Some(next);
                if let Err(error) = applied {
                    self.next_scene = None;
                    self.drop_stream(error);
                    return Applied::Failed;
                }
                return Applied::Other;
            }
            if stream::is_keyframe(&message) {
                let mut scene = Scene::default();
                if let Err(error) = self
                    .prepare(&mut scene, &message)
                    .and_then(|()| scene.apply(&message, 1.0))
                {
                    self.drop_stream(error);
                    return Applied::Failed;
                }
                // A reconnect resends the ended match itself; that replaces
                // it as any keyframe does, below.
                if scene.match_id != self.live_scene.match_id {
                    self.next_scene = Some(scene);
                    self.switch_due();
                    return Applied::Other;
                }
            }
        }
        let previous_match = self.live_scene.match_id;
        let mut scene = std::mem::take(&mut self.live_scene);
        let applied = self
            .prepare(&mut scene, &message)
            .and_then(|()| scene.apply(&message, 1.0));
        self.live_scene = scene;
        if let Err(error) = applied {
            self.drop_stream(error);
            return Applied::Failed;
        }
        let turn = self.live_scene.turn;
        self.newest_turn = turn;
        if stream::is_keyframe(&message) {
            self.recent.clear();
            if self.live_scene.match_id != previous_match {
                // Nothing cached or shown belongs to the new match.
                self.segments.clear();
                // The first join has nothing to reset, and its bytes are
                // only counted after it.
                if previous_match != 0 {
                    self.reset_counts();
                }
                self.go_live(false);
            }
        } else {
            if self.recent.len() == RECENT_DELTAS {
                self.recent.pop_front();
            }
            self.recent.push_back((turn, message.clone()));
        }
        Applied::Live(message)
    }

    fn on_ended(&mut self, match_id: u64, since_ms: u64) {
        let at = self.now() - since_ms as f64;
        if match_id == self.live_scene.match_id {
            self.ended_at = Some(at);
            self.schedule_switch();
            self.post_status();
        } else if self
            .next_scene
            .as_ref()
            .is_some_and(|next| next.match_id == match_id)
        {
            self.next_ended_at = Some(at);
        }
    }

    fn switch_deadline(&self) -> Option<f64> {
        self.ended_at.map(|at| at + SWITCH_DELAY_MS)
    }

    fn schedule_switch(&mut self) {
        if let Some(timer) = self.switch_timer.take() {
            self.scope.clear_timeout_with_handle(timer);
        }
        let Some(deadline) = self.switch_deadline() else {
            return;
        };
        self.switch_timer = self
            .scope
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                &self.switch_wake,
                (deadline - self.now()).max(0.0).ceil() as i32,
            )
            .ok();
    }

    /// Moves on if the deadline has passed and the next match is there;
    /// else only tells the page how things stand.
    fn switch_due(&mut self) {
        if self
            .switch_deadline()
            .is_some_and(|deadline| self.now() >= deadline)
        {
            self.switch_match();
        } else {
            self.post_status();
        }
    }

    fn switch_match(&mut self) {
        let Some(next) = self.next_scene.take() else {
            self.post_status();
            return;
        };
        self.live_scene = next;
        self.newest_turn = self.live_scene.turn;
        self.ended_at = self.next_ended_at.take();
        // Nothing cached belongs to the new match; a fetch still in flight
        // is dropped on arrival.
        self.recent.clear();
        self.segments.clear();
        self.reset_counts();
        self.live = false;
        self.go_live(true);
        self.schedule_switch();
    }

    fn on_command(&mut self, command: u8, value: f64) {
        match command {
            COMMAND_SCRUB => self.seek(value as u32, false),
            COMMAND_SEEK => self.seek(value as u32, true),
            COMMAND_PLAY => self.play(),
            COMMAND_PAUSE => self.pause(),
            COMMAND_SPEED => self.set_speed(value as f32),
            COMMAND_LIVE => self.go_live(true),
            COMMAND_CURRENT_MATCH => self.switch_match(),
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
        scene.ground = self.live_scene.ground.clone();
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
        if stream::is_keyframe(message)
            && let Ok(id) = stream::keyframe_terrain(message)
            && id != 0
            && id != self.page_terrain.get()
            && let Some((_, bytes)) = self.terrains.iter().find(|(ground, _)| ground.id == id)
        {
            let mut out = Vec::with_capacity(9 + bytes.len());
            out.push(POST_TERRAIN);
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(bytes);
            self.post(&out);
            self.page_terrain.set(id);
        }
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
        if self.ended_at.is_some() {
            flags |= STATUS_ENDED;
        }
        if self.next_scene.is_some() {
            flags |= STATUS_NEXT_READY;
        }
        let switch_in = self
            .switch_deadline()
            .map_or(0.0, |deadline| (deadline - self.now()).max(0.0));
        let mut out = vec![POST_STATUS];
        out.extend_from_slice(&self.live_scene.first_turn.to_le_bytes());
        out.extend_from_slice(&self.newest_turn.to_le_bytes());
        out.extend_from_slice(&playhead.to_le_bytes());
        out.push(flags);
        out.extend_from_slice(&self.speed.to_le_bytes());
        out.extend_from_slice(&self.live_bytes.to_le_bytes());
        out.extend_from_slice(&self.segment_bytes.to_le_bytes());
        out.extend_from_slice(&(switch_in.ceil() as u32).to_le_bytes());
        out.extend_from_slice(&self.terrain_bytes.to_le_bytes());
        out.extend_from_slice(&(self.now() - self.counted_since).to_le_bytes());
        out.extend_from_slice(&self.peak_window_bytes.to_le_bytes());
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

    let playback = Rc::new_cyclic(|this: &Weak<RefCell<Playback>>| {
        // The timer targets live as long as the worker.
        let target = this.clone();
        let wake = Closure::<dyn FnMut()>::new(move || {
            if let Some(playback) = target.upgrade() {
                playback.borrow_mut().resume();
            }
        });
        let function = wake.as_ref().unchecked_ref::<js_sys::Function>().clone();
        wake.forget();
        let target = this.clone();
        let switch_wake = Closure::<dyn FnMut()>::new(move || {
            if let Some(playback) = target.upgrade() {
                let mut playback = playback.borrow_mut();
                playback.switch_timer = None;
                playback.switch_due();
            }
        });
        let switch_function = switch_wake
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone();
        switch_wake.forget();
        RefCell::new(Playback {
            this: this.clone(),
            scope: scope.clone(),
            socket: None,
            socket_handlers: None,
            frame: None,
            reconnect_ms: RECONNECT_MIN_MS,
            wake: function,
            switch_wake: switch_function,
            live_scene: Scene::default(),
            ended_at: None,
            next_scene: None,
            next_ended_at: None,
            switch_timer: None,
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
            terrain_bytes: 0,
            counted_since: scope
                .performance()
                .map_or_else(js_sys::Date::now, |performance| performance.now()),
            live_window: VecDeque::new(),
            peak_window_bytes: 0,
            terrains: Vec::new(),
            held: Vec::new(),
            page_terrain: Cell::new(0),
        })
    });
    connect(&Rc::downgrade(&playback))?;

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

struct SocketHandlers {
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut()>,
}

/// Opens the stream. A closed one is opened again after a growing pause, as
/// the server may be restarting; its first message is a keyframe, which
/// replaces whatever the old connection left.
fn connect(playback: &Weak<RefCell<Playback>>) -> Result<(), String> {
    let Some(strong) = playback.upgrade() else {
        return Ok(());
    };
    let location = strong.borrow().scope.location();
    let scheme = match location.protocol().as_str() {
        "https:" => "wss",
        _ => "ws",
    };
    let url = format!("{scheme}://{}/ws", location.host());
    let socket = WebSocket::new(&url).map_err(|_| format!("cannot open {url}"))?;
    socket.set_binary_type(BinaryType::Arraybuffer);

    let receiver = playback.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() else {
            return;
        };
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        if let Some(playback) = receiver.upgrade() {
            playback.borrow_mut().on_socket(bytes);
        }
    });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

    let target = playback.clone();
    let on_close = Closure::<dyn FnMut()>::new(move || {
        let Some(strong) = target.upgrade() else {
            return;
        };
        let (scope, delay) = {
            let mut playback = strong.borrow_mut();
            let delay = playback.reconnect_ms;
            playback.reconnect_ms = (delay * 2.0).min(RECONNECT_MAX_MS);
            (playback.scope.clone(), delay)
        };
        crate::log(&format!("stream closed, reconnecting in {delay} ms"));
        let again = target.clone();
        let retry = Closure::once_into_js(move || {
            if let Err(error) = connect(&again) {
                web_sys::console::error_1(&error.into());
            }
        });
        let _ = scope.set_timeout_with_callback_and_timeout_and_arguments_0(
            retry.unchecked_ref(),
            delay as i32,
        );
    });
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

    let mut strong = strong.borrow_mut();
    strong.socket = Some(socket);
    strong.socket_handlers = Some(SocketHandlers {
        _on_message: on_message,
        _on_close: on_close,
    });
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
