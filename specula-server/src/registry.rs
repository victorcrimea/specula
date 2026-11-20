//! The matches the server runs: each hosted match has its own `Hub`, its
//! own `Play`, its own feed socket and the UDP port of its slot, and is
//! addressed by a UUID (`/g/<uuid>`). The broadcast hub of `--observe` and
//! `--slots 0` is one long-lived hub that many matches pass through, under
//! a UUID of its own for the server's run.
//!
//! Lock order: registry `inner`, then `Play`'s inner, then `Hub` state.
//! Nothing called from inside a `Play` or `Hub` lock calls the registry;
//! `Play::start`'s `on_exit` runs after its lock is released.
//!
//! An ended match stays here, with its hub and publisher thread, until the
//! server exits; only its slot is freed.

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
use std::time::SystemTime;

use crate::config::Config;
use crate::history::History;
use crate::observer;
use crate::play::Play;
use crate::play::Setup;
use crate::view::Hub;

/// `sockaddr_un.sun_path`, terminator included.
const SOCKET_PATH_LIMIT: usize = 108;
const ACCEPT_POLL: Duration = Duration::from_millis(100);
const FEED_SOCKET_ERROR: &str = "cannot open the feed socket";

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
}

impl Match {
    pub fn state(&self) -> MatchState {
        *self.state.lock().unwrap()
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

struct Inner {
    matches: HashMap<MatchId, Arc<Match>>,
    /// Launch order, oldest first.
    order: Vec<MatchId>,
    /// `config.slots` entries; the match whose engine holds the slot's port.
    slots: Vec<Option<MatchId>>,
    generation: u64,
}

/// The publisher thread is never stopped; see the module doc.
pub fn spawn_hub(delay: Duration, history: Option<Arc<Mutex<History>>>) -> Arc<Hub> {
    let hub = Arc::new(Hub::new(delay, history));
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
                order: Vec::new(),
                slots,
                generation: 0,
            }),
        }
    }

    pub fn broadcast(&self) -> Option<MatchId> {
        self.broadcast.as_ref().map(|(id, _)| *id)
    }

    /// The hub of the broadcast or of a hosted match, for the observers'
    /// routes.
    pub fn hub(&self, id: MatchId) -> Option<Arc<Hub>> {
        if let Some((broadcast, hub)) = &self.broadcast
            && *broadcast == id
        {
            return Some(Arc::clone(hub));
        }
        self.hosted(id).map(|found| Arc::clone(&found.hub))
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
        let hub = spawn_hub(self.delay, self.history.clone());
        let play = Arc::new(Play::new(
            socket.clone(),
            self.config.port(slot),
            Arc::clone(&hub),
            generation,
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
            "match {id} (game {generation}) in slot {slot}, port {}, feed {socket}",
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
            }),
        );
        inner.order.push(id);
        inner.slots[usize::from(slot)] = Some(id);
        Ok(id)
    }

    /// A new match with the setup of the one `owner` started in `id`.
    pub fn rematch(self: &Arc<Self>, owner: &str, id: MatchId) -> Result<MatchId, &'static str> {
        let setup = self
            .hosted(id)
            .and_then(|found| found.play.setup_of(owner))
            .ok_or("no game to repeat")?;
        self.launch(owner, &setup)
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
