//! The matches the server runs: each hosted match has its own `Hub`, its
//! own `Play`, its own feed socket and the UDP port of its slot, and is
//! addressed by a UUID (`/g/<uuid>`). The broadcast hub of `--observe` and
//! `--slots 0` is one long-lived hub that many matches pass through; its
//! UUID is its live view, and each match it encodes gets a UUID of its own.
//! A match's UUID outlives its hub: once only the history holds it,
//! `/g/<uuid>` replays it from there (`Registry::resolve`).
//!
//! Lock order: registry `inner`, then `Play`'s inner, then `Hub` state.
//! Nothing called from inside a `Play` or `Hub` lock calls the registry;
//! `Play::start`'s `on_exit` runs after its lock is released.
//!
//! An ended match frees its slot when its engine exits, and leaves once its
//! hub has released the end (M8): from then on `/g/<uuid>` is the history's,
//! or gone with the history off. Only who started it and its setup stay, for
//! a rematch.
//!
//! A stop (D22) waits until every hosted match has ended and its hub has
//! released the end, so observers see it and the history holds all of it.
//!
//! `watch_matches` stops an engine at L1 or L2 (D18, D19), and logs the 15,
//! 5 and 1 minute warnings the pages show from `limits` (D20). It also stops
//! a decided game's engine, which stays in its session otherwise, and drops
//! the ended matches.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::fs::File;
use std::hash::BuildHasher;
use std::io::ErrorKind;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use crate::config::Config;
use crate::history::History;
use crate::observer;
use crate::play::Play;
use crate::play::Setup;
use crate::view::Hub;
use crate::view::Message;
use crate::view::SummaryBody;
use crate::view::TerrainFile;
use crate::view::View;
use crate::view::stored_data;
use crate::view::stored_segment;
use crate::view::stored_summary;
use crate::view::stored_terrain;

/// `sockaddr_un.sun_path`, terminator included.
const SOCKET_PATH_LIMIT: usize = 108;
const ACCEPT_POLL: Duration = Duration::from_millis(100);
const FEED_SOCKET_ERROR: &str = "cannot open the feed socket";
const STOPPING_ERROR: &str = "the server is stopping";
const STOP_POLL: Duration = Duration::from_secs(1);
const WATCH_POLL: Duration = Duration::from_secs(1);
/// How long a decided game's engine runs on, so its final summary comes in
/// and the seat's Continue watching shows a while of the rest.
const DECIDED_GRACE: Duration = Duration::from_secs(60);
/// The time left at which everyone is warned (D20), longest first; the
/// pages keep the same list.
const LIMIT_WARNINGS: [Duration; 3] = [
    Duration::from_secs(15 * 60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(60),
];
/// `DEFAULT_TURN_LENGTH` (`TurnManager.h`), which a hosted game runs at.
const TURN_LENGTH: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MatchId([u8; 16]);

impl MatchId {
    pub fn new_v4() -> Self {
        let mut bytes = [0; 16];
        let random = File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
        if random.is_err() {
            let state = RandomState::new();
            let now = SystemTime::now();
            bytes[..8].copy_from_slice(&state.hash_one((now, 0u8)).to_le_bytes());
            bytes[8..].copy_from_slice(&state.hash_one((now, 1u8)).to_le_bytes());
        }
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        MatchId(bytes)
    }

    pub fn parse(text: &str) -> Option<MatchId> {
        let text = text.as_bytes();
        if text.len() != 36 {
            return None;
        }
        let mut bytes = [0; 16];
        let mut nibbles = 0;
        for (index, &byte) in text.iter().enumerate() {
            if [8, 13, 18, 23].contains(&index) {
                if byte != b'-' {
                    return None;
                }
                continue;
            }
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                _ => return None,
            };
            bytes[nibbles / 2] |= digit << (4 * (1 - nibbles % 2));
            nibbles += 1;
        }
        Some(MatchId(bytes))
    }
}

impl std::fmt::Display for MatchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if [4, 6, 8, 10].contains(&index) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl<'r> rocket::request::FromParam<'r> for MatchId {
    type Error = &'r str;

    fn from_param(param: &'r str) -> Result<Self, Self::Error> {
        MatchId::parse(param).ok_or(param)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum MatchState {
    /// Setup opens before the launch (D13, M15); nothing enters it yet.
    #[allow(dead_code)]
    Setup,
    Running,
    Ended,
}

pub struct Match {
    pub slot: u16,
    /// `<SOCKET>.<uuid>`; its pid file is `<socket>.observer`.
    pub socket: String,
    pub hub: Arc<Hub>,
    pub play: Arc<Play>,
    state: Mutex<MatchState>,
    /// The engine's runner thread, handed to a rematch in the same slot,
    /// which joins it first: the UDP port is the slot's.
    runner: Mutex<Option<JoinHandle<()>>>,
    /// Taken as the engine's start for L1: the runner may first wait a
    /// moment for the slot's previous engine, which only makes it stricter.
    launched: Instant,
    /// How many of `LIMIT_WARNINGS` were logged.
    warned: Mutex<usize>,
    /// When the watcher first saw the game decided.
    decided: Mutex<Option<Instant>>,
}

impl Match {
    pub fn state(&self) -> MatchState {
        *self.state.lock().unwrap()
    }

    /// L2 counts this. Without save and restore it is the game clock; a
    /// restored match will add what it played before the save (D19).
    fn game_time(&self) -> Duration {
        TURN_LENGTH * self.hub.turn()
    }

    /// The wall time and the game time left before a limit stops it.
    fn limit_left(&self, config: &Config) -> (Duration, Duration) {
        (
            config.wall_limit.saturating_sub(self.launched.elapsed()),
            config.game_limit.saturating_sub(self.game_time()),
        )
    }
}

/// What a `/g/<uuid>` request reads.
pub struct Source {
    /// The hub still holding the match; None once only the history has it.
    pub hub: Option<Arc<Hub>>,
    /// The stream id of the match the UUID names; None for the broadcast's
    /// own UUID, which follows whatever match its hub shows. 0 for a hosted
    /// match before its first turn.
    match_id: Option<u64>,
    history: Option<Arc<Mutex<History>>>,
}

impl Source {
    pub fn info(&self) -> Option<(u64, usize, bool)> {
        match (self.match_id, &self.hub) {
            (None, hub) => hub.as_ref()?.shown_info(),
            (Some(id), Some(hub)) => hub
                .match_info(id)
                .map(|(segments, live)| (id, segments, live)),
            (Some(id), None) => {
                let count = self.history.as_ref()?.lock().unwrap().segment_count(id)?;
                Some((id, count, false))
            }
        }
    }

    /// A match's URL never serves another match.
    fn names(&self, mid: u64) -> bool {
        self.match_id.is_none_or(|id| id == mid)
    }

    pub fn segment(&self, mid: u64, index: usize, detail: bool) -> Option<(Message, bool)> {
        if !self.names(mid) {
            return None;
        }
        match &self.hub {
            Some(hub) if detail => hub.detail_segment(mid, index, View::Observer),
            Some(hub) => hub.segment(mid, index, View::Observer),
            None => stored_segment(self.history.as_deref()?, mid, detail, index),
        }
    }

    pub fn summary(&self, mid: u64, turn: Option<u32>) -> Option<SummaryBody> {
        if !self.names(mid) {
            return None;
        }
        match &self.hub {
            Some(hub) => hub.summary(mid, turn),
            None => stored_summary(self.history.as_deref()?, mid, turn),
        }
    }

    /// Content-addressed, so any match's URL may serve it.
    pub fn terrain(&self, id: u64) -> Option<TerrainFile> {
        match &self.hub {
            Some(hub) => hub.terrain(id),
            None => stored_terrain(self.history.as_deref()?, id),
        }
    }

    pub fn data(&self, hash: u64) -> Option<Arc<[u8]>> {
        match &self.hub {
            Some(hub) => hub.data(hash, View::Observer),
            None => stored_data(self.history.as_deref()?, hash),
        }
    }

    /// Whether observers have already reached the match's end.
    pub fn replay_only(&self) -> bool {
        match (self.match_id, &self.hub) {
            (None, _) => false,
            (Some(id), Some(hub)) => !hub.observers_running(id),
            (Some(_), None) => true,
        }
    }

    pub fn uuid_of(&self, mid: u64, requested: MatchId) -> Option<MatchId> {
        match self.match_id {
            Some(id) => (mid == id).then_some(requested),
            None => self
                .hub
                .as_ref()
                .and_then(|hub| hub.uuid_of(mid))
                .or_else(|| self.history.as_ref()?.lock().unwrap().uuid_of(mid)),
        }
    }
}

pub struct Registry {
    config: Arc<Config>,
    /// The SOCKET argument; match sockets are `<base>.<uuid>`.
    socket_base: String,
    delay: Duration,
    history: Option<Arc<Mutex<History>>>,
    /// `--observe` or `--slots 0`: the one long-lived hub.
    broadcast: Option<(MatchId, Arc<Hub>)>,
    inner: Mutex<Inner>,
}

/// What an ended match leaves once it is dropped.
struct Ended {
    owner: String,
    setup: Setup,
}

struct Inner {
    matches: HashMap<MatchId, Arc<Match>>,
    ended: HashMap<MatchId, Ended>,
    /// Launch order, oldest first.
    order: Vec<MatchId>,
    /// `config.slots` entries; the match whose engine holds the slot's port.
    slots: Vec<Option<MatchId>>,
    generation: u64,
    /// Here rather than in `Registry`, so `launch` and `stop` agree on it.
    stopping: bool,
}

/// The publisher thread runs until `Hub::retire`, which only a dropped
/// match's hub gets.
pub fn spawn_hub(
    delay: Duration,
    history: Option<Arc<Mutex<History>>>,
    uuid: Option<MatchId>,
) -> Arc<Hub> {
    let hub = Arc::new(Hub::new(delay, history, uuid));
    let publisher = Arc::clone(&hub);
    std::thread::spawn(move || publisher.run_publisher());
    hub
}

impl Registry {
    pub fn new(
        config: Arc<Config>,
        socket_base: String,
        delay: Duration,
        history: Option<Arc<Mutex<History>>>,
        broadcast: Option<Arc<Hub>>,
    ) -> Self {
        let slots = vec![None; usize::from(config.slots)];
        Registry {
            config,
            socket_base,
            delay,
            history,
            broadcast: broadcast.map(|hub| (MatchId::new_v4(), hub)),
            inner: Mutex::new(Inner {
                matches: HashMap::new(),
                ended: HashMap::new(),
                order: Vec::new(),
                slots,
                generation: 0,
                stopping: false,
            }),
        }
    }

    pub fn broadcast(&self) -> Option<MatchId> {
        self.broadcast.as_ref().map(|(id, _)| *id)
    }

    /// What `/g/<id>` reads: a hub still holding the match, or else the
    /// history.
    pub fn resolve(&self, id: MatchId) -> Option<Source> {
        let source = |hub: Option<Arc<Hub>>, match_id: Option<u64>| Source {
            hub,
            match_id,
            history: self.history.clone(),
        };
        if let Some((broadcast, hub)) = &self.broadcast
            && *broadcast == id
        {
            return Some(source(Some(Arc::clone(hub)), None));
        }
        if let Some(found) = self.hosted(id) {
            let match_id = found.hub.match_of(id).unwrap_or(0);
            return Some(source(Some(Arc::clone(&found.hub)), Some(match_id)));
        }
        if let Some((_, hub)) = &self.broadcast
            && let Some(match_id) = hub.match_of(id)
        {
            return Some(source(Some(Arc::clone(hub)), Some(match_id)));
        }
        // After `hosted` released `inner`: no registry lock is taken under
        // the history's.
        let match_id = self.history.as_ref()?.lock().unwrap().match_of(id)?;
        Some(source(None, Some(match_id)))
    }

    /// Hosted matches only, for the seats' routes.
    pub fn hosted(&self, id: MatchId) -> Option<Arc<Match>> {
        self.inner.lock().unwrap().matches.get(&id).cloned()
    }

    /// The newest running match `visitor` holds the seat of.
    pub fn own_running(&self, visitor: &str) -> Option<MatchId> {
        let inner = self.inner.lock().unwrap();
        Self::own_running_in(&inner, visitor)
    }

    fn own_running_in(inner: &Inner, visitor: &str) -> Option<MatchId> {
        inner.order.iter().rev().copied().find(|id| {
            inner.matches.get(id).is_some_and(|found| {
                found.state() == MatchState::Running && found.play.status(Some(visitor)).mine
            })
        })
    }

    /// The newest match of `owner` whose engine still holds its slot,
    /// running or not: after End the engine takes a moment to die, and a
    /// rematch started meanwhile must wait for it rather than be refused.
    fn own_slot_holder_in(inner: &Inner, owner: &str) -> Option<Arc<Match>> {
        inner.order.iter().rev().find_map(|id| {
            let found = inner.matches.get(id)?;
            (inner.slots[usize::from(found.slot)] == Some(*id)
                && found.play.seat(Some(owner)).is_some())
            .then(|| Arc::clone(found))
        })
    }

    pub fn latest_running(&self) -> Option<MatchId> {
        let inner = self.inner.lock().unwrap();
        inner.order.iter().rev().copied().find(|id| {
            inner
                .matches
                .get(id)
                .is_some_and(|found| found.state() == MatchState::Running)
        })
    }

    pub fn slot_free(&self) -> bool {
        self.inner.lock().unwrap().slots.iter().any(Option::is_none)
    }

    /// Starts a match for `owner`. A new game ends the visitor's running
    /// one (D14), in whose slot it then runs.
    pub fn launch(self: &Arc<Self>, owner: &str, setup: &Setup) -> Result<MatchId, &'static str> {
        // A bad form costs nothing.
        Play::check(setup)?;
        let mut inner = self.inner.lock().unwrap();
        if inner.stopping {
            return Err(STOPPING_ERROR);
        }
        let old = Self::own_slot_holder_in(&inner, owner);
        let slot = match &old {
            Some(old) => old.slot,
            None => inner
                .slots
                .iter()
                .position(Option::is_none)
                .ok_or("a game is in progress")? as u16,
        };
        let id = MatchId::new_v4();
        // Stored only once the launch succeeds, so the log numbers games.
        let generation = inner.generation + 1;
        let socket = format!("{}.{id}", self.socket_base);
        if socket.len() >= SOCKET_PATH_LIMIT {
            eprintln!(
                "feed socket path too long ({} bytes): {socket}",
                socket.len()
            );
            return Err(FEED_SOCKET_ERROR);
        }
        let listener = crate::remove_stale_socket(&socket)
            .and_then(|()| UnixListener::bind(&socket))
            .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
            .map_err(|error| {
                eprintln!("cannot open the feed socket {socket}: {error}");
                FEED_SOCKET_ERROR
            })?;
        let hub = spawn_hub(self.delay, self.history.clone(), Some(id));
        let dirs = observer::EngineDirs::new(&self.config.engine_dir, id, &format!("slot-{slot}"));
        let play = Arc::new(Play::new(
            socket.clone(),
            self.config.port(slot),
            Arc::clone(&hub),
            generation,
            dirs,
        ));
        let engine_done = Arc::new(AtomicBool::new(false));
        {
            let (hub, play) = (Arc::clone(&hub), Arc::clone(&play));
            let (engine_done, socket) = (Arc::clone(&engine_done), socket.clone());
            std::thread::spawn(move || feed_match(listener, &hub, &play, &engine_done, &socket));
        }
        // Only now: nothing above can fail the launch after the old game
        // was ended.
        let previous = old.as_ref().and_then(|old| {
            let _ = old.play.end(owner);
            old.runner.lock().unwrap().take()
        });
        let on_exit = {
            let registry: Weak<Registry> = Arc::downgrade(self);
            let engine_done = Arc::clone(&engine_done);
            move || {
                engine_done.store(true, Ordering::SeqCst);
                if let Some(registry) = registry.upgrade() {
                    registry.finished(id);
                }
            }
        };
        let runner = match play.start(owner, setup, previous, on_exit) {
            Ok(runner) => runner,
            Err(error) => {
                engine_done.store(true, Ordering::SeqCst);
                return Err(error);
            }
        };
        inner.generation = generation;
        println!(
            "match {id} (game {generation}) by {} in slot {slot}, port {}, feed {socket}",
            setup.nickname.as_str(),
            self.config.port(slot)
        );
        inner.matches.insert(
            id,
            Arc::new(Match {
                slot,
                socket,
                hub,
                play,
                state: Mutex::new(MatchState::Running),
                runner: Mutex::new(Some(runner)),
                launched: Instant::now(),
                warned: Mutex::new(0),
                decided: Mutex::new(None),
            }),
        );
        inner.order.push(id);
        inner.slots[usize::from(slot)] = Some(id);
        let live: Vec<MatchId> = inner
            .matches
            .iter()
            .filter(|(_, found)| *found.state.lock().unwrap() == MatchState::Running)
            .map(|(id, _)| *id)
            .collect();
        let root = self.config.engine_dir.clone();
        // Off the registry lock: removing a log dir can take a while.
        std::thread::spawn(move || observer::prune_logs(&root, &live));
        Ok(id)
    }

    /// A new match with the setup of the one `owner` started in `id`.
    pub fn rematch(self: &Arc<Self>, owner: &str, id: MatchId) -> Result<MatchId, &'static str> {
        let setup = {
            let inner = self.inner.lock().unwrap();
            match inner.matches.get(&id) {
                Some(found) => found.play.setup_of(owner),
                None => inner
                    .ended
                    .get(&id)
                    .filter(|ended| ended.owner == owner)
                    .map(|ended| ended.setup.clone()),
            }
        }
        .ok_or("no game to repeat")?;
        self.launch(owner, &setup)
    }

    /// Whether `visitor` started the dropped match `id`; None if it is not
    /// one.
    pub fn ended_owned(&self, id: MatchId, visitor: Option<&str>) -> Option<bool> {
        let inner = self.inner.lock().unwrap();
        let ended = inner.ended.get(&id)?;
        Some(visitor == Some(ended.owner.as_str()))
    }

    /// Drops the ended matches whose hub has released the end, so the
    /// history serves them and their hub is freed.
    fn drop_settled(&self) {
        let inner = &mut *self.inner.lock().unwrap();
        let settled: Vec<MatchId> = inner
            .matches
            .iter()
            .filter(|(_, found)| found.state() == MatchState::Ended && found.hub.settled())
            .map(|(id, _)| *id)
            .collect();
        for id in settled {
            let Some(found) = inner.matches.remove(&id) else {
                continue;
            };
            inner.order.retain(|known| *known != id);
            if let Some((owner, setup)) = found.play.owner_setup() {
                inner.ended.insert(id, Ended { owner, setup });
            }
            found.hub.retire();
            println!("match {id} dropped from memory");
        }
    }

    fn finished(&self, id: MatchId) {
        let mut inner = self.inner.lock().unwrap();
        let Some(found) = inner.matches.get(&id).cloned() else {
            return;
        };
        *found.state.lock().unwrap() = MatchState::Ended;
        // A rematch may already own the slot.
        if inner.slots[usize::from(found.slot)] == Some(id) {
            inner.slots[usize::from(found.slot)] = None;
        }
    }

    /// A running hosted match's wall time left and its game limit as a
    /// game time, which a page turns into its own countdown: the wall time
    /// runs on while the game is paused, the game time follows the turns
    /// the page is shown.
    pub fn limits(&self, id: MatchId) -> Option<(Duration, Duration)> {
        let found = self.hosted(id)?;
        if found.state() != MatchState::Running {
            return None;
        }
        let (wall, _) = found.limit_left(&self.config);
        Some((wall, self.config.game_limit))
    }

    /// Never returns; one thread for every hosted match.
    pub fn watch_matches(&self) {
        loop {
            let running: Vec<(MatchId, Arc<Match>)> = {
                let inner = self.inner.lock().unwrap();
                inner
                    .matches
                    .iter()
                    .filter(|(_, found)| found.state() == MatchState::Running)
                    .map(|(id, found)| (*id, Arc::clone(found)))
                    .collect()
            };
            // Off the registry lock: stopping an engine runs `kill`.
            for (id, found) in &running {
                let (wall, game) = found.limit_left(&self.config);
                let (left, limit) = if wall <= game {
                    (wall, "wall")
                } else {
                    (game, "game")
                };
                {
                    // A limit shorter than a warning passes it at once; one
                    // line says the same.
                    let crossed = LIMIT_WARNINGS.iter().filter(|&&at| left <= at).count();
                    let mut warned = found.warned.lock().unwrap();
                    if crossed > *warned {
                        *warned = crossed;
                        println!(
                            "match {id}: {limit} time limit in {} min",
                            left.as_secs().div_ceil(60)
                        );
                    }
                }
                if left.is_zero() && found.play.end_at_limit() {
                    // D20's scoring goes before this stop (M30); until then
                    // the match ends undecided.
                    println!("match {id} reached the {limit} time limit, stopping its engine");
                }
                if found.play.decided() {
                    let since = *found
                        .decided
                        .lock()
                        .unwrap()
                        .get_or_insert_with(Instant::now);
                    if since.elapsed() >= DECIDED_GRACE {
                        found.play.end_decided();
                    }
                }
            }
            self.drop_settled();
            std::thread::sleep(WATCH_POLL);
        }
    }

    pub fn stopping(&self) -> bool {
        self.inner.lock().unwrap().stopping
    }

    /// Refuses new games and closes the open setups (D22); answers how many
    /// matches are still running.
    pub fn stop(&self) -> usize {
        let inner = &mut *self.inner.lock().unwrap();
        inner.stopping = true;
        let mut running = 0;
        for (id, found) in &inner.matches {
            let mut state = found.state.lock().unwrap();
            match *state {
                MatchState::Setup => {
                    *state = MatchState::Ended;
                    if inner.slots[usize::from(found.slot)] == Some(*id) {
                        inner.slots[usize::from(found.slot)] = None;
                    }
                }
                MatchState::Running => running += 1,
                MatchState::Ended => {}
            }
        }
        running
    }

    /// Blocks until every hosted match has ended and been released.
    pub fn wait_stopped(&self) {
        loop {
            let running: Vec<Arc<Match>> = {
                let inner = self.inner.lock().unwrap();
                inner
                    .matches
                    .values()
                    .filter(|found| found.state() == MatchState::Running)
                    .cloned()
                    .collect()
            };
            // Off the registry lock: stopping an engine runs `kill`.
            for found in &running {
                found.play.end_decided();
            }
            if self.idle() {
                return;
            }
            std::thread::sleep(STOP_POLL);
        }
    }

    fn idle(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner
            .matches
            .values()
            .all(|found| found.state() == MatchState::Ended && found.hub.settled())
    }

    /// At exit: no engine may outlive the server.
    pub fn kill_all(&self) {
        let inner = self.inner.lock().unwrap();
        for found in inner.matches.values() {
            if found.state() == MatchState::Running {
                observer::kill_recorded(&found.socket);
            }
        }
    }
}

/// Serves the one connection of a match's engine, then removes the socket.
fn feed_match(
    listener: UnixListener,
    hub: &Arc<Hub>,
    play: &Arc<Play>,
    engine_done: &AtomicBool,
    socket: &str,
) {
    // Polled: an engine that dies before connecting would leave a blocking
    // accept waiting for good.
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break Some(stream),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if engine_done.load(Ordering::SeqCst) {
                    break None;
                }
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(error) => {
                eprintln!("accept failed on {socket}: {error}");
                break None;
            }
        }
    };
    if let Some(stream) = stream {
        let _ = stream.set_nonblocking(false);
        println!("match {socket} observer connected");
        hub.start_feed();
        play.set_uplink(stream.try_clone().ok());
        let served = crate::serve(stream, hub, Some(play));
        play.set_uplink(None);
        match served {
            Ok(()) => println!("match {socket} observer disconnected"),
            Err(error) => eprintln!("match {socket} observer feed failed: {error}"),
        }
        // Whether the host quit, the game was won or the observer died,
        // this match gets no more turns.
        hub.end_match();
    }
    let _ = crate::remove_stale_socket(socket);
}
