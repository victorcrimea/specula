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
//! u64 most live bytes in `PEAK_WINDOW_S` seconds, then for the detail
//! stream u64 bytes of live messages and u64 bytes of segments received for
//! the shown match, f64 ms it has been followed since the counts began and
//! u8 1 while it is followed; or u8 `POST_DETAIL`,
//! then a detail stream message (`detail.rs`), none to clear the details;
//! or u8 `POST_STARTS`, u32 ms until the server shows the match's first
//! turn, while it holds them all back.
//!
//! A page opened with `?t=<seconds>` (a copied link) names a moment of the
//! clock of the match the hub shows, and the page passes it on in the
//! worker's own URL, with `base=/g/<uuid>`. If `<base>/info` says that
//! match is the one being shown, it is followed live as usual and seeks to
//! the moment once joined.
//! Otherwise it is a stored match: there is no WebSocket, its last segment
//! is applied as live messages would be, which gives the scene its end, and
//! playback seeks to the moment and pauses at the end. `STATUS_ARCHIVE`
//! tells the page.
//!
//! While the page has something selected (`COMMAND_DETAIL` 1) and for
//! `DETAIL_LINGER_MS` after (`COMMAND_DETAIL` 0), the worker also follows
//! the detail stream: live over `/detail`, and from its segments for turns
//! the live deltas do not reach. The page is handed the detail state of the
//! turn it shows, never of a later one, so the details step with the
//! visuals: deltas when they follow on, else a keyframe.

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
use crate::detail::Details;
use crate::stream;
use crate::stream::FrameDecoder;
use crate::stream::Ground;
use crate::stream::Scene;

pub const POST_SCENE: u8 = 1;
pub const POST_STATUS: u8 = 2;
pub const POST_TERRAIN: u8 = 3;
pub const POST_DETAIL: u8 = 4;
pub const POST_STARTS: u8 = 5;
pub const STATUS_LIVE: u8 = 1;
pub const STATUS_PAUSED: u8 = 2;
/// The shown match has ended.
pub const STATUS_ENDED: u8 = 4;
/// The next match has started and can be switched to.
pub const STATUS_NEXT_READY: u8 = 8;
/// The shown match is a stored one, played from its segments alone.
pub const STATUS_ARCHIVE: u8 = 16;
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
/// 1 while the page has something selected, else 0.
pub const COMMAND_DETAIL: u8 = 8;

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
/// How long the detail stream is followed after the selection is cleared,
/// so that clicking around does not fetch it again each time.
const DETAIL_LINGER_MS: f64 = 10_000.0;
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
    /// The match's `/g/<uuid>`, which its terrain and info are under.
    base: String,
    /// Before every stream URL: `base` plus "/play" for the player's page,
    /// which follows the live stream, else `base`.
    prefix: String,
    /// The player's page, whose next game is the one just started.
    seated: bool,
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
    /// The detail stream's, likewise, apart from the visual stream's.
    detail_live_bytes: u64,
    detail_segment_bytes: u64,
    /// How long it was followed since the counts began, up to
    /// `detail_since`, and when it was last taken up, while it is followed.
    detail_ms: f64,
    detail_since: Option<f64>,
    /// Fetched terrains, oldest first: the heightmap and the resource.
    terrains: Vec<(Rc<Ground>, Rc<[u8]>)>,
    /// Live messages waiting, by WebSocket message, from the first
    /// keyframe whose terrain is being fetched on.
    held: Vec<Vec<Vec<u8>>>,
    /// The terrain the page was last handed, and for a seat's the reveals
    /// it holds, those of the deltas it was handed since included.
    page_terrain: Cell<(u64, u32)>,
    /// The turn of the last scene message the page was handed.
    page_turn: Cell<u32>,
    /// The page has something selected, or had until less than
    /// `DETAIL_LINGER_MS` ago.
    detail_wanted: bool,
    detail_linger: Option<i32>,
    /// Calls `detail_linger_due`, for the linger timer.
    detail_linger_wake: js_sys::Function,
    detail_socket: Option<WebSocket>,
    detail_handlers: Option<SocketHandlers>,
    /// Goes up with every detail socket, so that an old one's close does
    /// not reconnect.
    detail_generation: u32,
    detail_frame: Option<FrameDecoder>,
    detail_reconnect_ms: f64,
    /// Every live detail message applied; it may belong to the match after
    /// the shown one.
    live_details: Details,
    /// The last live detail keyframe and its turn, and the live deltas
    /// since, without a gap.
    live_detail_keyframe: (u32, Vec<u8>),
    recent_details: VecDeque<(u32, Vec<u8>)>,
    /// The last live control (kind 3) since the live keyframe. Controls are
    /// not in `recent_details`, so a page that is caught up from segments
    /// or deltas gets this one again after them, to be current.
    live_control: Option<Vec<u8>>,
    /// Least recently used first, all of the shown match.
    detail_segments: Vec<Segment>,
    detail_fetching: Option<u32>,
    /// The match and turn of the detail state the page holds.
    page_details: Option<(u64, u32)>,
    /// A stored match, played from its segments without a WebSocket.
    archived: bool,
    /// The match and turn a link asked for, sought once that match is in
    /// `live_scene`.
    initial_seek: Option<(u64, u32)>,
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

    /// A fetched terrain of `id` holding at least `revision` reveals (0: a
    /// whole one).
    fn ground(&self, id: u64, revision: u32) -> Option<Rc<Ground>> {
        self.terrains
            .iter()
            .find(|(ground, _)| ground.id == id && ground.revision >= revision)
            .map(|(ground, _)| Rc::clone(ground))
    }

    /// Gives `scene` the heightmap a keyframe names before it is applied:
    /// its own or the live scene's when they hold enough of a seat's, as
    /// reveals only add.
    fn prepare(&self, scene: &mut Scene, message: &[u8]) -> Result<(), &'static str> {
        if stream::is_keyframe(message) {
            let (id, revision) = stream::keyframe_terrain(message)?;
            let enough = |ground: &Option<Rc<Ground>>| {
                ground
                    .as_ref()
                    .is_some_and(|ground| ground.id == id && ground.revision >= revision)
            };
            if enough(&scene.ground) {
                return Ok(());
            }
            scene.ground = if enough(&self.live_scene.ground) {
                self.live_scene.ground.clone()
            } else {
                self.ground(id, revision)
            };
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
        self.detail_live_bytes = 0;
        self.detail_segment_bytes = 0;
        self.detail_ms = 0.0;
        if self.detail_since.is_some() {
            self.detail_since = Some(self.counted_since);
        }
    }

    fn read_socket(&mut self, bytes: Vec<u8>) {
        let Some((&envelope, body)) = bytes.split_first() else {
            self.drop_stream("empty message");
            return;
        };
        if envelope == stream::ENVELOPE_STARTS {
            match stream::read_starts(body) {
                Ok(wait_ms) => {
                    let mut out = vec![POST_STARTS];
                    out.extend_from_slice(&wait_ms.to_le_bytes());
                    self.post(&out);
                }
                Err(error) => self.drop_stream(error),
            }
            return;
        }
        let messages = match envelope {
            stream::ENVELOPE_JOIN => stream::read_join(body, |_| false).map(|(frame, messages)| {
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
                // A seat's terrain is fetched again when a join's keyframe
                // has more reveals than any heightmap held; the deltas
                // after it bring their own.
                Ok((id, revision))
                    if id != 0
                        && self.ground(id, revision).is_none()
                        && !self.live_scene.ground.as_ref().is_some_and(|ground| {
                            ground.id == id && ground.revision >= revision
                        }) =>
                {
                    self.held.push(messages);
                    self.request_terrain(id, revision);
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

    fn request_terrain(&mut self, id: u64, revision: u32) {
        // A seat's own, as revealed so far.
        let url = if revision > 0 {
            format!("{}/terrain/{id:016x}", self.prefix)
        } else {
            format!("{}/terrain/{id:016x}", self.base)
        };
        let this = self.this.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = fetch_bytes(&url).await.and_then(|bytes| {
                let ground = Ground::from_resource(id, &bytes)?;
                Ok((Rc::new(ground), Rc::from(bytes)))
            });
            if let Some(playback) = this.upgrade() {
                let mut playback = playback.borrow_mut();
                playback.terrain_arrived(id, result);
                playback.sync_details();
            }
        });
    }

    fn terrain_arrived(&mut self, id: u64, result: Result<(Rc<Ground>, Rc<[u8]>), String>) {
        let held = std::mem::take(&mut self.held);
        match result {
            Ok(terrain) => {
                // A seat's newer one replaces its older.
                self.terrains.retain(|(ground, _)| ground.id != id);
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
        if let Some((match_id, turn)) = self.initial_seek
            && match_id == self.live_scene.match_id
        {
            self.initial_seek = None;
            self.seek(turn, true);
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

    /// Plays a stored match: its last segment, applied as a join would be,
    /// gives the match's end, then `initial_seek` moves to the moment.
    fn open_archive(&mut self, match_id: u64, segments: u32) {
        self.archived = true;
        let url = format!("{}/match/{match_id:016x}/seg/{}", self.prefix, segments - 1);
        let this = self.this.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let fetched = fetch_bytes(&url).await;
            let Some(playback) = this.upgrade() else {
                return;
            };
            let mut playback = playback.borrow_mut();
            let messages = fetched.and_then(|bytes| {
                playback.segment_bytes += bytes.len() as u64;
                stream::read_segment(&bytes).map_err(String::from)
            });
            let messages = messages.and_then(|messages| {
                let keyframe_turn = stream::message_turn(messages.first().ok_or("empty segment")?)?;
                // The delta into the keyframe's turn is for playback
                // crossing into the segment; the keyframe already holds it.
                let mut kept = Vec::with_capacity(messages.len());
                for (position, message) in messages.into_iter().enumerate() {
                    if position == 0 || stream::message_turn(&message)? > keyframe_turn {
                        kept.push(message);
                    }
                }
                Ok::<_, String>(kept)
            });
            match messages {
                Ok(messages) => playback.on_live(messages),
                Err(error) => {
                    web_sys::console::error_1(&format!("match {match_id:016x}: {error}").into());
                }
            }
            playback.sync_details();
        });
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
        // Not a turn: it has no place in the playback's turn order, so it
        // only reaches a viewer who is live, and the keyframes of the others
        // carry the state.
        if stream::is_pause(&message) {
            if let Err(error) = self.live_scene.apply(&message, 1.0) {
                self.drop_stream(error);
                return Applied::Failed;
            }
            return Applied::Live(message);
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
                self.detail_segments.clear();
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
        // The player's next game is the one they just started.
        let delay = if !self.seated { SWITCH_DELAY_MS } else { 0.0 };
        self.ended_at.map(|at| at + delay)
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
        self.detail_segments.clear();
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
            COMMAND_LIVE if self.archived => self.seek(self.newest_turn, true),
            COMMAND_LIVE => self.go_live(true),
            COMMAND_CURRENT_MATCH => self.switch_match(),
            COMMAND_DETAIL => self.set_detail_wanted(value != 0.0),
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
        if !was_live && self.has_match() {
            if jump {
                self.post_scene(0.0, &self.live_scene.encode_keyframe());
            } else {
                // The deltas played up to here skipped any pause or resume.
                self.post_scene(0.0, &self.live_scene.encode_pause());
            }
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
        if exact && turn >= self.newest_turn && !self.archived {
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
                if self.archived {
                    // A stored match has nothing after its end.
                    if moved {
                        self.post_status();
                    }
                    self.pause();
                    return;
                }
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
        let url = format!("{}/match/{match_id:016x}/seg/{index}", self.prefix);
        let this = self.this.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let fetched = fetch_bytes(&url).await;
            let size = fetched.as_ref().map_or(0, |bytes| bytes.len() as u64);
            let result = fetched
                .and_then(|bytes| stream::read_segment(&bytes).map_err(String::from))
                .and_then(|messages| {
                    messages
                        .into_iter()
                        .filter(|message| !stream::is_pause(message))
                        .map(|message| Ok((stream::message_turn(&message)?, message)))
                        .collect::<Result<Vec<_>, &'static str>>()
                        .map_err(String::from)
                });
            if let Some(playback) = this.upgrade() {
                let mut playback = playback.borrow_mut();
                playback.segment_bytes += size;
                playback.segment_arrived(match_id, index, result);
                playback.sync_details();
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
        let (page_id, page_revision) = self.page_terrain.get();
        if stream::is_keyframe(message)
            && let Ok((id, revision)) = stream::keyframe_terrain(message)
            && id != 0
            && (id != page_id || page_revision < revision)
            && let Some((ground, bytes)) = self
                .terrains
                .iter()
                .find(|(ground, _)| ground.id == id && ground.revision >= revision)
        {
            let mut out = Vec::with_capacity(9 + bytes.len());
            out.push(POST_TERRAIN);
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(bytes);
            self.post(&out);
            self.page_terrain.set((id, ground.revision));
        } else if !stream::is_keyframe(message) && page_id == self.live_scene.terrain_id {
            // The page applies the delta's reveals itself.
            self.page_terrain
                .set((page_id, page_revision.max(self.live_scene.terrain_revision)));
        }
        if !stream::is_pause(message)
            && let Ok(turn) = stream::message_turn(message)
        {
            self.page_turn.set(turn);
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
        if self.archived {
            flags |= STATUS_ARCHIVE;
        } else if self.live {
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
        out.extend_from_slice(&self.detail_live_bytes.to_le_bytes());
        out.extend_from_slice(&self.detail_segment_bytes.to_le_bytes());
        let detail_ms = self.detail_ms + self.detail_since.map_or(0.0, |since| self.now() - since);
        out.extend_from_slice(&detail_ms.to_le_bytes());
        out.push(u8::from(self.detail_since.is_some()));
        self.post(&out);
    }

    fn post(&self, bytes: &[u8]) {
        let array = js_sys::Uint8Array::from(bytes);
        let transfer = js_sys::Array::of1(&array.buffer());
        let _ = self.scope.post_message_with_transfer(&array, &transfer);
    }

    fn post_detail(&self, message: &[u8]) {
        let mut out = Vec::with_capacity(1 + message.len());
        out.push(POST_DETAIL);
        out.extend_from_slice(message);
        self.post(&out);
    }

    fn set_detail_wanted(&mut self, wanted: bool) {
        if wanted {
            if let Some(timer) = self.detail_linger.take() {
                self.scope.clear_timeout_with_handle(timer);
            }
            if !self.detail_wanted {
                self.detail_wanted = true;
                self.detail_since = Some(self.now());
                self.detail_reconnect_ms = RECONNECT_MIN_MS;
                self.connect_detail();
                self.post_status();
            }
        } else if self.detail_wanted && self.detail_linger.is_none() {
            self.detail_linger = self
                .scope
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    &self.detail_linger_wake,
                    DETAIL_LINGER_MS as i32,
                )
                .ok();
        }
    }

    fn detail_linger_due(&mut self) {
        self.detail_linger = None;
        self.detail_wanted = false;
        if let Some(since) = self.detail_since.take() {
            self.detail_ms += self.now() - since;
        }
        self.detail_generation += 1;
        if let Some(socket) = self.detail_socket.take() {
            let _ = socket.close();
        }
        self.detail_handlers = None;
        self.detail_frame = None;
        self.live_details = Details::default();
        self.recent_details.clear();
        self.live_control = None;
        self.detail_segments.clear();
        self.page_details = None;
        self.post_detail(&[]);
        self.post_status();
    }

    /// Opens the detail socket; a closed one is opened again after a
    /// growing pause, as the visual one is, while the details are wanted.
    fn connect_detail(&mut self) {
        // A stored match's details come from its segments.
        if self.archived {
            return;
        }
        self.detail_generation += 1;
        let generation = self.detail_generation;
        let location = self.scope.location();
        let scheme = match location.protocol().as_str() {
            "https:" => "wss",
            _ => "ws",
        };
        let url = format!("{scheme}://{}{}/detail", location.host(), self.prefix);
        let Ok(socket) = WebSocket::new(&url) else {
            web_sys::console::error_1(&format!("cannot open {url}").into());
            return;
        };
        socket.set_binary_type(BinaryType::Arraybuffer);

        let receiver = self.this.clone();
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() else {
                return;
            };
            let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
            if let Some(playback) = receiver.upgrade() {
                let mut playback = playback.borrow_mut();
                if playback.detail_generation == generation {
                    playback.on_detail_socket(bytes);
                    playback.sync_details();
                }
            }
        });
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        let target = self.this.clone();
        let on_close = Closure::<dyn FnMut()>::new(move || {
            let Some(strong) = target.upgrade() else {
                return;
            };
            let (scope, delay) = {
                let mut playback = strong.borrow_mut();
                if playback.detail_generation != generation || !playback.detail_wanted {
                    return;
                }
                let delay = playback.detail_reconnect_ms;
                playback.detail_reconnect_ms = (delay * 2.0).min(RECONNECT_MAX_MS);
                (playback.scope.clone(), delay)
            };
            crate::log(&format!("detail stream closed, reconnecting in {delay} ms"));
            let again = target.clone();
            let retry = Closure::once_into_js(move || {
                if let Some(playback) = again.upgrade() {
                    let mut playback = playback.borrow_mut();
                    if playback.detail_generation == generation && playback.detail_wanted {
                        playback.connect_detail();
                    }
                }
            });
            let _ = scope.set_timeout_with_callback_and_timeout_and_arguments_0(
                retry.unchecked_ref(),
                delay as i32,
            );
        });
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

        self.detail_frame = None;
        self.detail_socket = Some(socket);
        self.detail_handlers = Some(SocketHandlers {
            _on_message: on_message,
            _on_close: on_close,
        });
    }

    fn on_detail_socket(&mut self, bytes: Vec<u8>) {
        let size = bytes.len() as u64;
        let Some((&envelope, body)) = bytes.split_first() else {
            self.drop_detail_stream("empty message");
            return;
        };
        let messages = match envelope {
            stream::ENVELOPE_JOIN => {
                stream::read_join(body, crate::detail::is_control).map(|(frame, messages)| {
                    self.detail_frame = Some(frame);
                    messages
                })
            }
            stream::ENVELOPE_FRAME => {
                let frame = self.detail_frame.insert(FrameDecoder::new());
                frame.feed(body)
            }
            stream::ENVELOPE_MORE => match &mut self.detail_frame {
                Some(frame) => frame.feed(body),
                None => Err("frame continued before it started"),
            },
            _ => Err("unknown envelope"),
        };
        let applied = messages.and_then(|messages| {
            for message in messages {
                self.apply_live_detail(message)?;
            }
            Ok(())
        });
        match applied {
            Ok(()) => self.detail_reconnect_ms = RECONNECT_MIN_MS,
            Err(error) => self.drop_detail_stream(error),
        }
        self.detail_live_bytes += size;
    }

    fn apply_live_detail(&mut self, message: Vec<u8>) -> Result<(), &'static str> {
        if stream::is_keyframe(&message) {
            let mut details = Details::default();
            details.apply(&message)?;
            self.live_detail_keyframe = (details.turn, message);
            self.live_details = details;
            self.recent_details.clear();
            // The keyframe carries the control as of its turn, so an
            // earlier live one must not be sent after it.
            self.live_control = None;
            return Ok(());
        }
        if crate::detail::is_control(&message) {
            // Sent at once, even while paused when no turn moves; it is not
            // a turn, so it stays out of `recent_details`.
            self.live_details.apply(&message)?;
            self.post_detail(&message);
            self.live_control = Some(message);
            return Ok(());
        }
        self.live_details.apply(&message)?;
        if self.recent_details.len() == RECENT_DELTAS {
            self.recent_details.pop_front();
        }
        self.recent_details
            .push_back((self.live_details.turn, message));
        Ok(())
    }

    /// The reconnect starts again from a keyframe.
    fn drop_detail_stream(&mut self, error: &str) {
        web_sys::console::error_1(&format!("bad detail stream message: {error}").into());
        self.detail_frame = None;
        self.live_details = Details::default();
        self.recent_details.clear();
        self.live_control = None;
        if let Some(socket) = &self.detail_socket {
            let _ = socket.close();
        }
    }

    /// Hands the page the detail state of the turn it shows, if it is
    /// here; otherwise asks for what is missing, whose arrival calls this
    /// again.
    fn sync_details(&mut self) {
        if !self.detail_wanted || !self.has_match() {
            return;
        }
        let match_id = self.live_scene.match_id;
        let target = self.page_turn.get();
        let live = self.live_details.known && self.live_details.match_id == match_id;
        let at = self
            .page_details
            .filter(|(shown, _)| *shown == match_id)
            .map(|(_, turn)| turn);
        if at == Some(target) {
            return;
        }
        if let Some(at) = at.filter(|at| *at < target) {
            let reaches = live
                && at >= self.live_detail_keyframe.0
                && self
                    .recent_details
                    .front()
                    .is_none_or(|(front, _)| *front <= at + 1);
            let deltas = if reaches {
                // Up to the newest the socket brought, which may be short of
                // the page's turn; the rest follows as it arrives.
                Some(
                    self.recent_details
                        .iter()
                        .filter(|(turn, _)| *turn > at && *turn <= target)
                        .cloned()
                        .collect(),
                )
            } else if self.index_of(at + 1) == self.index_of(target) {
                self.segment_deltas(at, target)
            } else {
                // A jump past a whole segment starts again from a keyframe.
                None
            };
            if let Some(deltas) = deltas {
                let mut reached = at;
                for (turn, message) in deltas {
                    self.post_detail(&message);
                    reached = turn;
                }
                self.post_live_control();
                self.page_details = Some((match_id, reached));
                return;
            }
            if reaches || self.index_of(at + 1) == self.index_of(target) {
                return;
            }
        }
        let keyframe = if live && target >= self.live_details.turn {
            self.live_details.encode_keyframe()
        } else {
            let from_live = if live && target >= self.live_detail_keyframe.0 {
                self.live_details_at(target)
            } else {
                None
            };
            let details = if from_live.is_some() {
                from_live
            } else if self.live && self.detail_socket.is_some() && !live {
                // The socket's join is on its way and is newer than any
                // segment.
                return;
            } else {
                self.segment_details(target)
            };
            match details {
                Some(details) => details.encode_keyframe(),
                None => return,
            }
        };
        if let Ok(turn) = stream::message_turn(&keyframe) {
            self.post_detail(&keyframe);
            self.post_live_control();
            self.page_details = Some((match_id, turn));
        }
    }

    /// Sends the page the live control again, after a keyframe or deltas
    /// that may carry an older one.
    fn post_live_control(&self) {
        if let Some(control) = &self.live_control {
            self.post_detail(control);
        }
    }

    /// The live detail state at `turn`, from the last live keyframe and
    /// the deltas since; `None` when they do not reach it.
    fn live_details_at(&self, turn: u32) -> Option<Details> {
        let (keyframe_turn, keyframe) = &self.live_detail_keyframe;
        if self
            .recent_details
            .front()
            .is_some_and(|(front, _)| *front > keyframe_turn + 1)
        {
            return None;
        }
        let mut details = Details::default();
        details.apply(keyframe).ok()?;
        for (message_turn, message) in &self.recent_details {
            if *message_turn > turn {
                break;
            }
            details.apply(message).ok()?;
        }
        Some(details)
    }

    /// The detail deltas after turn `after` up to `upto`, from the
    /// segments; `None` while a segment is fetched.
    fn segment_deltas(&mut self, after: u32, upto: u32) -> Option<Vec<(u32, Vec<u8>)>> {
        let newest_index = self.index_of(self.newest_turn);
        let mut deltas = Vec::new();
        let mut reached = after;
        for index in self.index_of(after + 1)..=self.index_of(upto) {
            let Some(position) = self.cached_detail(index) else {
                self.request_detail_segment(index);
                return None;
            };
            // Skipping the keyframe leaves the delta into its turn, as in
            // `next_message`.
            for (turn, message) in &self.detail_segments[position].messages[1..] {
                // A control of the turn already reached is set after the
                // delta of that turn, so it still goes out.
                let control = crate::detail::is_control(message);
                if (*turn > reached || (control && *turn == reached)) && *turn <= upto {
                    deltas.push((*turn, message.clone()));
                    reached = *turn;
                }
            }
            if index == newest_index && reached < upto {
                // The open segment has grown since it was fetched.
                self.detail_segments.remove(position);
                self.request_detail_segment(index);
                return None;
            }
        }
        Some(deltas)
    }

    /// The detail state at `turn` from its segment, then the live deltas if
    /// the segment ends before it, as `rebuild`; `None` while the segment is
    /// fetched.
    fn segment_details(&mut self, turn: u32) -> Option<Details> {
        let index = self.index_of(turn);
        let Some(position) = self.cached_detail(index) else {
            self.request_detail_segment(index);
            return None;
        };
        let mut details = Details::default();
        let applied = (|| {
            let messages = &self.detail_segments[position].messages;
            let keyframe_turn = messages[0].0;
            details.apply(&messages[0].1)?;
            for (message_turn, message) in &messages[1..] {
                // A control of the keyframe's turn is set after the keyframe.
                if *message_turn == keyframe_turn && !crate::detail::is_control(message) {
                    continue;
                }
                if *message_turn > turn {
                    break;
                }
                details.apply(message)?;
            }
            if self.live_details.match_id == details.match_id
                && self
                    .recent_details
                    .front()
                    .is_some_and(|(front, _)| *front <= details.turn + 1)
            {
                for (message_turn, message) in &self.recent_details {
                    if *message_turn > turn {
                        break;
                    }
                    if *message_turn > details.turn {
                        details.apply(message)?;
                    }
                }
            }
            Ok::<(), &'static str>(())
        })();
        if let Err(error) = applied {
            web_sys::console::error_1(&format!("bad detail segment {index}: {error}").into());
            self.detail_segments.remove(position);
            return None;
        }
        if details.turn < turn && index == self.index_of(self.newest_turn) {
            self.detail_segments.remove(position);
            self.request_detail_segment(index);
            return None;
        }
        Some(details)
    }

    fn cached_detail(&mut self, index: u32) -> Option<usize> {
        let position = self
            .detail_segments
            .iter()
            .position(|segment| segment.index == index)?;
        let segment = self.detail_segments.remove(position);
        self.detail_segments.push(segment);
        Some(self.detail_segments.len() - 1)
    }

    /// One fetch at a time, as `request_segment`.
    fn request_detail_segment(&mut self, index: u32) {
        if self.detail_fetching.is_some() {
            return;
        }
        self.detail_fetching = Some(index);
        let match_id = self.live_scene.match_id;
        let url = format!("{}/match/{match_id:016x}/detail/{index}", self.prefix);
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
                playback.detail_segment_bytes += size;
                playback.detail_segment_arrived(match_id, index, result);
                playback.post_status();
                playback.sync_details();
            }
        });
    }

    fn detail_segment_arrived(
        &mut self,
        match_id: u64,
        index: u32,
        result: Result<Vec<(u32, Vec<u8>)>, String>,
    ) {
        self.detail_fetching = None;
        if match_id != self.live_scene.match_id || !self.detail_wanted {
            return;
        }
        match result {
            Ok(messages) => {
                if self.detail_segments.len() == CACHED_SEGMENTS {
                    self.detail_segments.remove(0);
                }
                self.detail_segments.push(Segment { index, messages });
            }
            // The next turn the page is handed asks again.
            Err(error) => {
                web_sys::console::error_1(&format!("detail segment {index}: {error}").into());
            }
        }
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
                let mut playback = playback.borrow_mut();
                playback.resume();
                playback.sync_details();
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
                playback.sync_details();
            }
        });
        let switch_function = switch_wake
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone();
        switch_wake.forget();
        let target = this.clone();
        let detail_linger_wake = Closure::<dyn FnMut()>::new(move || {
            if let Some(playback) = target.upgrade() {
                playback.borrow_mut().detail_linger_due();
            }
        });
        let detail_linger_function = detail_linger_wake
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone();
        detail_linger_wake.forget();
        let search = scope.location().search();
        let seated = crate::query_param(&search, "play").is_some();
        let base = crate::query_param(&search, "base")
            .and_then(|base| js_sys::decode_uri_component(&base).ok())
            .and_then(|base| base.as_string())
            .unwrap_or_default();
        RefCell::new(Playback {
            this: this.clone(),
            scope: scope.clone(),
            seated,
            prefix: if seated {
                format!("{base}/play")
            } else {
                base.clone()
            },
            base,
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
            detail_live_bytes: 0,
            detail_segment_bytes: 0,
            detail_ms: 0.0,
            detail_since: None,
            terrains: Vec::new(),
            held: Vec::new(),
            page_terrain: Cell::new((0, 0)),
            page_turn: Cell::new(0),
            detail_wanted: false,
            detail_linger: None,
            detail_linger_wake: detail_linger_function,
            detail_socket: None,
            detail_handlers: None,
            detail_generation: 0,
            detail_frame: None,
            detail_reconnect_ms: RECONNECT_MIN_MS,
            live_details: Details::default(),
            live_detail_keyframe: (0, Vec::new()),
            recent_details: VecDeque::new(),
            live_control: None,
            detail_segments: Vec::new(),
            detail_fetching: None,
            page_details: None,
            archived: false,
            initial_seek: None,
        })
    });
    open(&Rc::downgrade(&playback))?;

    let on_command = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let bytes = js_sys::Uint8Array::new(&event.data()).to_vec();
        let Some((&command, rest)) = bytes.split_first() else {
            return;
        };
        let Some(value) = rest.first_chunk::<8>() else {
            return;
        };
        let mut playback = playback.borrow_mut();
        playback.on_command(command, f64::from_le_bytes(*value));
        playback.sync_details();
    });
    scope.set_onmessage(Some(on_command.as_ref().unchecked_ref()));
    on_command.forget();
    Ok(())
}

/// Follows the live stream, unless the worker's URL names a moment (from
/// the page's link), which `<base>/info` says how to open.
fn open(playback: &Weak<RefCell<Playback>>) -> Result<(), String> {
    let Some(strong) = playback.upgrade() else {
        return Ok(());
    };
    let search = strong.borrow().scope.location().search();
    let Some(seconds) = crate::query_param(&search, "t")
        .and_then(|t| t.parse::<f64>().ok())
        .filter(|t| t.is_finite() && *t >= 0.0)
    else {
        return connect(playback);
    };
    let base = strong.borrow().base.clone();
    let turn = (seconds * 1000.0 / TURN_MS) as u32;
    let playback = playback.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let info = fetch_bytes(&format!("{base}/info"))
            .await
            .and_then(|bytes| {
                let text = String::from_utf8(bytes).map_err(|_| "not text")?;
                let mut fields = text.split_whitespace();
                let (Some(id), Some(segments), Some(live)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    return Err("bad info".to_string());
                };
                let id = u64::from_str_radix(id, 16).map_err(|_| "bad match id")?;
                let segments = segments.parse::<u32>().map_err(|_| "bad segment count")?;
                Ok::<_, String>((id, segments, live == "1"))
            });
        let Some(strong) = playback.upgrade() else {
            return;
        };
        match info {
            Ok((match_id, segments, false)) if segments > 0 => {
                let mut strong = strong.borrow_mut();
                strong.initial_seek = Some((match_id, turn));
                strong.open_archive(match_id, segments);
            }
            Ok((match_id, _, _)) => {
                strong.borrow_mut().initial_seek = Some((match_id, turn));
                if let Err(error) = connect(&playback) {
                    web_sys::console::error_1(&error.into());
                }
            }
            // Gone from the history, or never stored: the live stream
            // rather than nothing.
            Err(error) => {
                web_sys::console::error_1(&format!("match info: {error}").into());
                if let Err(error) = connect(&playback) {
                    web_sys::console::error_1(&error.into());
                }
            }
        }
    });
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
    let prefix = strong.borrow().prefix.clone();
    let url = format!("{scheme}://{}{prefix}/ws", location.host());
    let socket = WebSocket::new(&url).map_err(|_| format!("cannot open {url}"))?;
    socket.set_binary_type(BinaryType::Arraybuffer);

    let receiver = playback.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() else {
            return;
        };
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        if let Some(playback) = receiver.upgrade() {
            let mut playback = playback.borrow_mut();
            playback.on_socket(bytes);
            playback.sync_details();
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
