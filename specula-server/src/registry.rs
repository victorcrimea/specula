//! The matches the server runs: each hosted match has its own `Hub`, its
//! own `Play`, its own feed socket and the UDP port of its slot, and is
//! addressed by a UUID (`/g/<uuid>`). The broadcast hub of `--observe` and
//! `--slots 0` is one long-lived hub that many matches pass through; its
//! UUID is its live view, and each match it encodes gets a UUID of its own.
//! A match's UUID outlives its hub: once only the history holds it,
//! `/g/<uuid>` replays it from there (`Registry::resolve`).
//!
//! A match opens as a setup (D13): its seats, hub, play and `SetupRoom` exist
//! from `open_setup` on, the visitors join over the room's websocket, and
//! the host's launch (`launch_setup`) starts the engine. A setup closes on
//! its own (`tick_setups`, D15, D16) or when it is replaced or the server
//! stops; the ids of closed ones are kept (`closed`) so their page says why.
//!
//! Lock order: registry `inner`, then the `SetupRoom`, then `Play`'s inner,
//! then `Seats`, then `Hub` state.
//! Nothing called from inside a `SetupRoom`, `Play`, `Seats` or `Hub` lock
//! calls the registry; `Play::start`'s `on_exit` runs after its lock is
//! released. `launch_setup` resolves the settings under the room's lock and
//! releases it before it takes the registry's.
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
//! 5 and 1 minute warnings the pages show from `limits` (D20). A reached
//! limit first has the players scored and the losers resigned (M30), then
//! stops the engine. It also stops a decided game's engine, which stays in
//! its session otherwise, and drops the ended matches.

use std::collections::HashMap;
use std::collections::VecDeque;
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

use serde_json::Value;
use serde_json::json;

use crate::catalog::Catalog;
use crate::chat::Chat;
use crate::config::Config;
use crate::history::History;
use crate::nickname::Nickname;
use crate::observer;
use crate::play::Play;
use crate::seats::Seats;
use crate::settings::Settings;
use crate::setup::CLOSED_KEEP;
use crate::setup::CloseReason;
use crate::setup::LEAVE_GRACE;
use crate::setup::Request;
use crate::setup::SetupRoom;
use crate::setup::catalog_json;
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
/// The lobby refreshes thumbnails every 15 s, so a drawing this old is
/// current enough for every visitor.
const THUMB_TTL: Duration = Duration::from_secs(10);

/// `DEFAULT_TURN_LENGTH` (`TurnManager.h`), which a hosted game runs at.
pub(crate) const TURN_LENGTH: Duration = Duration::from_millis(200);

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
    /// Open for visitors to join, before the host's launch (D13).
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
    /// From the setup's opening on (D13); `play` shares it.
    pub seats: Arc<Seats>,
    pub room: Arc<SetupRoom>,
    /// From the setup's opening on (D50), so setup chat carries into the
    /// game; a rematch is a new match with an empty chat.
    pub chat: Arc<Chat>,
    state: Mutex<MatchState>,
    /// The engine's runner thread, handed to a rematch in the same slot,
    /// which joins it first: the UDP port is the slot's.
    runner: Mutex<Option<JoinHandle<()>>>,
    /// The runner of the game this setup replaced, which the launch joins
    /// first for the same reason.
    previous: Mutex<Option<JoinHandle<()>>>,
    /// Taken as the engine's start for L1: the runner may first wait a
    /// moment for the slot's previous engine, which only makes it stricter.
    launched: Mutex<Option<Instant>>,
    /// The lobby's thumbnail as last drawn (M41), so a lobby full of
    /// visitors draws it once per `THUMB_TTL`.
    thumb: Mutex<Option<(Instant, Arc<Vec<u8>>)>>,
    /// How many of `LIMIT_WARNINGS` were logged.
    warned: Mutex<usize>,
    /// When the watcher first saw the game decided.
    decided: Mutex<Option<Instant>>,
    /// The setup a rematch opened for this match (M35), once its host
    /// started one.
    rematch: Mutex<Option<MatchId>>,
}

impl Match {
    pub fn state(&self) -> MatchState {
        *self.state.lock().unwrap()
    }

    /// The thumbnail as WebP, at most `THUMB_TTL` old; None while the
    /// match is not running or has shown nothing yet. Blocks while it
    /// draws, so call it off the async threads.
    pub fn thumb_webp(&self) -> Option<Arc<Vec<u8>>> {
        // M36's "none" observer mode returns None here, which the lobby
        // shows as the map's stock preview (D61).
        if self.state() != MatchState::Running {
            return None;
        }
        let mut cached = self.thumb.lock().unwrap();
        if let Some((drawn, bytes)) = &*cached
            && drawn.elapsed() < THUMB_TTL
        {
            return Some(Arc::clone(bytes));
        }
        let (thumb, terrain) = self.hub.thumb()?;
        let bytes = Arc::new(crate::thumb::render(&thumb, &terrain)?);
        *cached = Some((Instant::now(), Arc::clone(&bytes)));
        Some(bytes)
    }

    /// L2 counts this. Without save and restore it is the game clock; a
    /// restored match will add what it played before the save (D19).
    fn game_time(&self) -> Duration {
        TURN_LENGTH * self.hub.turn()
    }

    /// The wall time and the game time left before a limit stops it.
    fn limit_left(&self, config: &Config) -> (Duration, Duration) {
        (
            config.wall_limit.saturating_sub(
                self.launched
                    .lock()
                    .unwrap()
                    .unwrap_or_else(Instant::now)
                    .elapsed(),
            ),
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

    /// The summary of the match the UUID names, for a route that does not
    /// carry its stream id. None for the broadcast's own UUID.
    pub fn own_summary(&self, turn: Option<u32>) -> Option<SummaryBody> {
        self.summary(self.match_id?, turn)
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
    /// Hosting only.
    catalog: Option<Catalog>,
    /// What every setup socket is sent first.
    catalog_json: Option<String>,
    inner: Mutex<Inner>,
}

/// The seated players a rematch holds slots for (M35).
pub type Followers = Vec<(String, Nickname, u8)>;

/// What an ended match leaves once it is dropped.
struct Ended {
    owner: String,
    settings: Settings,
    nickname: Nickname,
    /// The owner's player number, which a rematch keeps if it is still a
    /// slot.
    seat: Option<u8>,
    /// The seated members at the drop, whose slots the rematch reserves.
    players: Followers,
    /// The setup a rematch opened for this match (M35).
    rematch: Option<MatchId>,
}

struct Inner {
    matches: HashMap<MatchId, Arc<Match>>,
    ended: HashMap<MatchId, Ended>,
    /// Launch order, oldest first.
    order: Vec<MatchId>,
    /// `config.slots` entries; the match whose engine holds the slot's port.
    slots: Vec<Option<MatchId>>,
    generation: u64,
    /// Here rather than in `Registry`, so `launch_setup` and `stop` agree on
    /// it.
    stopping: bool,
    /// The newest closed setups, oldest first.
    closed: VecDeque<(MatchId, CloseReason)>,
    /// A closed setup's predecessor engine, by slot, which the next game of
    /// the slot joins first.
    pending: HashMap<u16, JoinHandle<()>>,
}

/// One handle that joins both.
fn chain(first: Option<JoinHandle<()>>, second: Option<JoinHandle<()>>) -> Option<JoinHandle<()>> {
    match (first, second) {
        (Some(first), Some(second)) => Some(std::thread::spawn(move || {
            let _ = first.join();
            let _ = second.join();
        })),
        (first, second) => first.or(second),
    }
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
        catalog: Option<Catalog>,
    ) -> Self {
        let slots = vec![None; usize::from(config.slots)];
        let catalog_json = catalog.as_ref().map(catalog_json);
        Registry {
            config,
            socket_base,
            delay,
            history,
            broadcast: broadcast.map(|hub| (MatchId::new_v4(), hub)),
            catalog,
            catalog_json,
            inner: Mutex::new(Inner {
                matches: HashMap::new(),
                ended: HashMap::new(),
                order: Vec::new(),
                slots,
                generation: 0,
                stopping: false,
                closed: VecDeque::new(),
                pending: HashMap::new(),
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

    /// The newest setup or running match `visitor` hosts.
    pub fn own_hosted(&self, visitor: &str) -> Option<MatchId> {
        let inner = self.inner.lock().unwrap();
        inner.order.iter().rev().copied().find(|id| {
            inner.matches.get(id).is_some_and(|found| {
                found.state() != MatchState::Ended && found.seats.is_host(visitor)
            })
        })
    }

    /// The newest match `owner` hosts whose engine still holds its slot,
    /// running or not: after End the engine takes a moment to die, and a
    /// rematch started meanwhile must wait for it rather than be refused.
    fn own_slot_holder_in(inner: &Inner, owner: &str) -> Option<(MatchId, Arc<Match>)> {
        inner.order.iter().rev().find_map(|id| {
            let found = inner.matches.get(id)?;
            (inner.slots[usize::from(found.slot)] == Some(*id) && found.seats.is_host(owner))
                .then(|| (*id, Arc::clone(found)))
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

    /// `/lobby.json` (M40, D59): the setups, the running matches and the
    /// newest `replays` stored ones.
    pub fn lobby_json(&self, replays: usize) -> String {
        let hosted: Vec<(MatchId, Arc<Match>)> = {
            let inner = self.inner.lock().unwrap();
            inner
                .order
                .iter()
                .rev()
                .filter_map(|id| Some((*id, Arc::clone(inner.matches.get(id)?))))
                .collect()
        };
        let color = |color: u32| format!("#{color:06x}");
        let host_of = |found: &Match| {
            found
                .seats
                .host()
                .and_then(|host| found.seats.nickname_of(&host))
                .map(|nickname| nickname.as_str().to_string())
                .unwrap_or_default()
        };
        let mut setups = Vec::new();
        let mut running = Vec::new();
        // Off the registry lock, as `watch_matches`: each read takes the
        // match's own locks.
        for (id, found) in &hosted {
            match found.state() {
                MatchState::Setup => {
                    let Some((map, max)) = found.room.lobby_info() else {
                        continue;
                    };
                    setups.push(json!({
                        "id": id.to_string(),
                        "host": host_of(found),
                        "map": map.name,
                        "preview": map.preview,
                        "seated": found.seats.seated_names().len(),
                        "max": max,
                    }));
                }
                MatchState::Running => {
                    let map = found.play.lobby_info();
                    let mut players: Vec<Value> = found
                        .hub
                        .lobby_players()
                        .into_iter()
                        .map(|(name, civ, rgb)| {
                            json!({ "name": name, "civ": civ, "color": color(rgb) })
                        })
                        .collect();
                    // Before the first turn the feed has no players yet.
                    if players.is_empty() {
                        players = found
                            .seats
                            .seated_names()
                            .into_iter()
                            .map(|(_, name)| json!({ "name": name.as_str(), "civ": "", "color": null }))
                            .collect();
                    }
                    running.push(json!({
                        "id": id.to_string(),
                        "host": host_of(found),
                        "map": map.as_ref().and_then(|map| map.name.clone()),
                        "preview": map.and_then(|map| map.preview),
                        "players": players,
                        "elapsed": found.game_time().as_secs(),
                        "observers": found.hub.observer_count(),
                    }));
                }
                MatchState::Ended => {}
            }
        }
        let (stored, more) = match &self.history {
            Some(history) => history.lock().unwrap().list(replays),
            None => (Vec::new(), false),
        };
        let stored: Vec<Value> = stored
            .into_iter()
            .map(|info| {
                let meta = info.meta.as_ref();
                json!({
                    "id": info.uuid.to_string(),
                    "map": meta.and_then(|meta| meta.map_name.clone()),
                    "host": meta.map(|meta| meta.host.clone()),
                    "players": meta.map_or_else(Vec::new, |meta| {
                        meta.players
                            .iter()
                            .map(|(name, civ, rgb)| {
                                json!({ "name": name, "civ": civ, "color": color(*rgb) })
                            })
                            .collect()
                    }),
                    "duration": info.duration_secs,
                    "started": info.started_ms,
                })
            })
            .collect();
        json!({
            "setups": setups,
            "running": running,
            "replays": stored,
            "more": more,
        })
        .to_string()
    }

    pub fn slot_free(&self) -> bool {
        self.inner.lock().unwrap().slots.iter().any(Option::is_none)
    }

    /// Opens a setup for `owner`, its host, seated as a player (vanilla's
    /// `onClientJoin` gives the host the first free slot). `from` is a
    /// finished game's settings, the host's seat in it and the other seated
    /// players, for a rematch; their slots are reserved for them (M35).
    /// `batch` is the host's stored settings (D34), used only when `from` is
    /// `None`. A new setup replaces the game the visitor hosts (D14), in
    /// whose slot it then runs.
    pub fn open_setup(
        self: &Arc<Self>,
        owner: &str,
        nickname: Nickname,
        from: Option<(Settings, Option<u8>, Followers)>,
        batch: Option<&Value>,
    ) -> Result<MatchId, &'static str> {
        let catalog = self.catalog.as_ref().ok_or("not hosting")?;
        let id = MatchId::new_v4();
        let socket = format!("{}.{id}", self.socket_base);
        if socket.len() >= SOCKET_PATH_LIMIT {
            eprintln!(
                "feed socket path too long ({} bytes): {socket}",
                socket.len()
            );
            return Err(FEED_SOCKET_ERROR);
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.stopping {
            return Err(STOPPING_ERROR);
        }
        let old = Self::own_slot_holder_in(&inner, owner);
        // A running game is replaced only if its host may end it (D14, D45);
        // one whose engine is already stopped is waited for, as before.
        if let Some((_, old)) = &old
            && old.state() == MatchState::Running
            && old.play.game_running()
            && !old.play.may_end(owner)
        {
            return Err("your game is still running");
        }
        let slot = match &old {
            Some((_, old)) => old.slot,
            None => inner
                .slots
                .iter()
                .position(Option::is_none)
                .ok_or("every match slot is in use")? as u16,
        };
        let seats = Arc::new(Seats::new());
        let nickname = seats.join(owner, nickname);
        let seat = from
            .as_ref()
            .and_then(|(settings, seat, _)| {
                seat.filter(|seat| usize::from(*seat) <= settings.player_count.players)
            })
            .unwrap_or(1);
        let (settings, followers) = match from {
            Some((mut settings, _, players)) => {
                settings.on_seated(catalog, &[seat]);
                (settings, players)
            }
            None => (
                Settings::open(catalog, batch, &[seat]).map_err(|error| {
                    eprintln!("cannot open a setup: {error}");
                    "cannot open a setup"
                })?,
                Vec::new(),
            ),
        };
        let (reserved, dropped): (Followers, Followers) =
            followers.into_iter().partition(|(_, _, player)| {
                let slot = usize::from(*player) - 1;
                *player != seat
                    && slot < settings.player_count.players
                    && settings.player_ai.get(slot).is_some_and(Option::is_none)
                    && settings.player_removed.get(slot) != Some(&true)
            });
        // Rare (the host took the seat, or it is no slot any more), and
        // otherwise invisible: the player just finds no slot held.
        for (_, name, player) in &dropped {
            println!(
                "match {id}: no slot held for {} (player {player}), it is not open",
                name.as_str()
            );
        }
        seats.set_player(owner, Some(seat))?;
        seats.force_host_stay();
        // Only now: nothing above can fail after the old game was ended.
        if let Some((old_id, old)) = &old {
            match old.state() {
                MatchState::Setup => Self::close_in(&mut inner, *old_id, CloseReason::Replaced),
                MatchState::Running => old.play.end_replaced(),
                MatchState::Ended => {}
            }
        }
        let mut previous = inner.pending.remove(&slot);
        if let Some((_, old)) = &old {
            previous = chain(old.runner.lock().unwrap().take(), previous);
        }
        let hub = spawn_hub(self.delay, self.history.clone(), Some(id));
        let dirs = observer::EngineDirs::new(&self.config.engine_dir, id, &format!("slot-{slot}"))
            .with_settings();
        let play = Arc::new(Play::new(
            socket.clone(),
            self.config.port(slot),
            Arc::clone(&hub),
            dirs,
            Arc::clone(&seats),
        ));
        inner.matches.insert(
            id,
            Arc::new(Match {
                slot,
                socket,
                hub,
                play,
                seats,
                room: Arc::new(SetupRoom::new(settings, reserved)),
                chat: Arc::new(Chat::new(self.delay)),
                state: Mutex::new(MatchState::Setup),
                runner: Mutex::new(None),
                previous: Mutex::new(previous),
                launched: Mutex::new(None),
                thumb: Mutex::new(None),
                warned: Mutex::new(0),
                decided: Mutex::new(None),
                rematch: Mutex::new(None),
            }),
        );
        inner.order.push(id);
        inner.slots[usize::from(slot)] = Some(id);
        println!(
            "match {id} setup opened by {} in slot {slot}",
            nickname.as_str()
        );
        Ok(id)
    }

    /// The host's Start (M16): the engine starts with the resolved
    /// settings and the match runs.
    pub fn launch_setup(self: &Arc<Self>, id: MatchId, cookie: &str) -> Result<(), &'static str> {
        let catalog = self.catalog.as_ref().ok_or("not hosting")?;
        let found = self
            .hosted(id)
            .filter(|found| found.state() == MatchState::Setup)
            .ok_or("no such setup")?;
        let launch = found
            .room
            .begin_launch(catalog, &found.seats, cookie, &id.to_string())?;
        let started = self.start_engine(id, &found, launch);
        found.room.finish_launch(started.is_ok());
        started
    }

    fn start_engine(
        self: &Arc<Self>,
        id: MatchId,
        found: &Arc<Match>,
        launch: crate::settings::Launch,
    ) -> Result<(), &'static str> {
        let mut inner = self.inner.lock().unwrap();
        // A stop or a replacement may have closed it since the room's check.
        if inner.stopping {
            return Err(STOPPING_ERROR);
        }
        if !inner.matches.contains_key(&id) {
            return Err("the setup is closed");
        }
        let slot = found.slot;
        // Stored only once the launch succeeds, so the log numbers games.
        let generation = inner.generation + 1;
        let socket = found.socket.clone();
        let listener = crate::remove_stale_socket(&socket)
            .and_then(|()| UnixListener::bind(&socket))
            .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
            .map_err(|error| {
                eprintln!("cannot open the feed socket {socket}: {error}");
                FEED_SOCKET_ERROR
            })?;
        found.hub.set_meta_source(
            found
                .seats
                .host()
                .and_then(|host| found.seats.nickname_of(&host))
                .map(|nickname| nickname.as_str().to_string())
                .unwrap_or_default(),
            launch.settings.map_name.clone(),
        );
        let engine_done = Arc::new(AtomicBool::new(false));
        {
            let (hub, play) = (Arc::clone(&found.hub), Arc::clone(&found.play));
            let (engine_done, socket) = (Arc::clone(&engine_done), socket.clone());
            std::thread::spawn(move || feed_match(listener, &hub, &play, &engine_done, &socket));
        }
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
        let runner = match found
            .play
            .start(launch, &found.previous, generation, on_exit)
        {
            Ok(runner) => runner,
            Err(error) => {
                engine_done.store(true, Ordering::SeqCst);
                return Err(error);
            }
        };
        inner.generation = generation;
        *found.runner.lock().unwrap() = Some(runner);
        *found.launched.lock().unwrap() = Some(Instant::now());
        *found.state.lock().unwrap() = MatchState::Running;
        println!(
            "match {id} (game {generation}) by {} in slot {slot}, port {}, feed {socket}",
            found
                .seats
                .host()
                .and_then(|host| found.seats.nickname_of(&host))
                .map(|nickname| nickname.as_str().to_string())
                .unwrap_or_default(),
            self.config.port(slot)
        );
        let live: Vec<MatchId> = inner
            .matches
            .iter()
            .filter(|(_, found)| *found.state.lock().unwrap() == MatchState::Running)
            .map(|(id, _)| *id)
            .collect();
        let root = self.config.engine_dir.clone();
        // Off the registry lock: removing a log dir can take a while.
        std::thread::spawn(move || observer::prune_logs(&root, &live));
        Ok(())
    }

    /// A new setup from the settings of the game `owner` hosts in `id`,
    /// holding the other seated players' slots for them (M35).
    pub fn rematch(self: &Arc<Self>, owner: &str, id: MatchId) -> Result<MatchId, &'static str> {
        let (settings, seat, nickname, followers) = {
            let inner = self.inner.lock().unwrap();
            match inner.matches.get(&id) {
                Some(found) => found.play.rematch_settings_of(owner).and_then(|settings| {
                    Some((
                        settings,
                        found.seats.player_of(owner),
                        found.seats.nickname_of(owner)?,
                        found
                            .seats
                            .seated_members()
                            .into_iter()
                            .filter(|(cookie, _, _)| cookie != owner)
                            .collect(),
                    ))
                }),
                None => inner
                    .ended
                    .get(&id)
                    .filter(|ended| ended.owner == owner)
                    .map(|ended| {
                        (
                            ended.settings.clone(),
                            ended.seat,
                            ended.nickname.clone(),
                            ended.players.clone(),
                        )
                    }),
            }
        }
        .ok_or("no game to repeat")?;
        // D62: a rematch repeats the old game, not the host's stored copy.
        let new = self.open_setup(owner, nickname, Some((settings, seat, followers)), None)?;
        let inner = &mut *self.inner.lock().unwrap();
        if let Some(found) = inner.matches.get(&id) {
            *found.rematch.lock().unwrap() = Some(new);
        } else if let Some(ended) = inner.ended.get_mut(&id) {
            ended.rematch = Some(new);
        }
        Ok(new)
    }

    /// The setup `visitor` follows `id` into (M35), while it is still open:
    /// an old player's, not the host's, and not an observer's. Also after
    /// the match was dropped.
    pub fn rematch_of(&self, id: MatchId, visitor: &str) -> Option<MatchId> {
        let inner = self.inner.lock().unwrap();
        let next = match inner.matches.get(&id) {
            Some(found) => {
                if found.seats.player_of(visitor).is_none() || found.seats.is_host(visitor) {
                    return None;
                }
                *found.rematch.lock().unwrap()
            }
            None => {
                let ended = inner.ended.get(&id)?;
                if !ended.players.iter().any(|(cookie, _, _)| cookie == visitor) {
                    return None;
                }
                ended.rematch
            }
        }?;
        (inner.matches.get(&next)?.state() == MatchState::Setup).then_some(next)
    }

    /// Whether `id` is an open setup, or one that closed (its page says why).
    pub fn setup_page(&self, id: MatchId) -> bool {
        let inner = self.inner.lock().unwrap();
        inner
            .matches
            .get(&id)
            .is_some_and(|found| found.state() == MatchState::Setup)
            || inner.closed.iter().any(|(closed, _)| *closed == id)
    }

    pub fn closed_reason(&self, id: MatchId) -> Option<CloseReason> {
        let inner = self.inner.lock().unwrap();
        inner
            .closed
            .iter()
            .find(|(closed, _)| *closed == id)
            .map(|(_, reason)| *reason)
    }

    /// What a setup socket is sent first, and the catalog the room's rules
    /// read.
    pub fn catalog(&self) -> Option<(&Catalog, &str)> {
        Some((self.catalog.as_ref()?, self.catalog_json.as_deref()?))
    }

    /// A visitor's request in a setup; the reason is for the visitor only
    /// when it is a launch.
    pub fn setup_request(
        self: &Arc<Self>,
        id: MatchId,
        cookie: &str,
        request: Request,
    ) -> Result<(), &'static str> {
        if matches!(request, Request::Launch) {
            return self.launch_setup(id, cookie);
        }
        let catalog = self.catalog.as_ref().ok_or("not hosting")?;
        let found = self.hosted(id).ok_or("no such setup")?;
        found.room.handle(catalog, &found.seats, cookie, request)
    }

    /// Removes a setup and tells its sockets why (D15, D16, D22).
    fn close_in(inner: &mut Inner, id: MatchId, reason: CloseReason) {
        let Some(found) = inner.matches.remove(&id) else {
            return;
        };
        inner.order.retain(|known| *known != id);
        if inner.slots[usize::from(found.slot)] == Some(id) {
            inner.slots[usize::from(found.slot)] = None;
        }
        *found.state.lock().unwrap() = MatchState::Ended;
        found.room.close(reason);
        found.chat.close();
        found.hub.retire();
        // Its predecessor's engine may still hold the slot's port.
        if let Some(previous) = found.previous.lock().unwrap().take() {
            let waiting = inner.pending.remove(&found.slot);
            if let Some(joined) = chain(waiting, Some(previous)) {
                inner.pending.insert(found.slot, joined);
            }
        }
        inner.closed.push_back((id, reason));
        while inner.closed.len() > CLOSED_KEEP {
            inner.closed.pop_front();
        }
        println!("match {id} setup closed: {}", reason.name());
    }

    /// Members whose last setup socket is gone leave (U2), and the setups
    /// nobody started or nobody is in close (D15, D16).
    fn tick_setups(&self) {
        let Some(catalog) = &self.catalog else {
            return;
        };
        let setups: Vec<(MatchId, Arc<Match>)> = {
            let inner = self.inner.lock().unwrap();
            inner
                .matches
                .iter()
                .filter(|(_, found)| found.state() == MatchState::Setup)
                .map(|(id, found)| (*id, Arc::clone(found)))
                .collect()
        };
        for (id, found) in &setups {
            for cookie in found.seats.due_to_leave(LEAVE_GRACE) {
                let was_seated = found.seats.leave_setup(&cookie);
                found.room.after_leave(catalog, &found.seats, was_seated);
            }
            found.room.expire_reservations();
            if let Some(reason) = found.room.due() {
                let inner = &mut *self.inner.lock().unwrap();
                if found.state() == MatchState::Setup {
                    Self::close_in(inner, *id, reason);
                }
            }
        }
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
            if let Some((owner, settings)) = found.play.owner_settings()
                && let Some(nickname) = found.seats.nickname_of(&owner)
            {
                let seat = found.seats.player_of(&owner);
                let players = found
                    .seats
                    .seated_members()
                    .into_iter()
                    .filter(|(cookie, _, _)| *cookie != owner)
                    .collect();
                let rematch = *found.rematch.lock().unwrap();
                inner.ended.insert(
                    id,
                    Ended {
                        owner,
                        settings,
                        nickname,
                        seat,
                        players,
                        rematch,
                    },
                );
            }
            found.chat.close();
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
                found.play.tick_pause();
                found.play.tick_control();
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
                if left.is_zero() && found.play.tick_limit(limit) && found.play.end_at_limit() {
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
            self.tick_setups();
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
        let mut setups = Vec::new();
        for (id, found) in &inner.matches {
            match found.state() {
                MatchState::Setup => setups.push(*id),
                MatchState::Running => running += 1,
                MatchState::Ended => {}
            }
        }
        for id in setups {
            Self::close_in(inner, id, CloseReason::Stopping);
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
