//! Hosting: the server hosts a game for a web player instead of observing
//! someone else's. A headless engine (`scripts/play.sh`) hosts it as an
//! observer, with the player's slot left unassigned and without AI; the
//! feed socket carries the match out as usual and the player's commands
//! back (`SpeculaFeed.h`, the uplink), which the engine posts for that
//! slot as the game's controller.
//!
//! One game per `Play`, and one `Play` per match (`registry.rs`).
//! `/g/<uuid>` is the player's page on the live stream for a visitor who
//! holds a seat, and the observers' page with the delay for anyone else;
//! this split is a convenience, not a guard. Who holds which seat, and who
//! hosts, is the match's seat table (`seats.rs`), by the visitor's `sv`
//! cookie.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value;

use specula_assets::command::ALLOW_CAPTURE;
use specula_assets::command::Command as Order;
use specula_assets::command::PUSH_FRONT;
use specula_assets::command::QUEUED;
use specula_assets::command::Query;
use specula_assets::command::QueryKind;

use rocket::tokio::sync::broadcast;

use crate::detail::CONTROL_ABSENT;
use crate::detail::CONTROL_BANNED;
use crate::detail::CONTROL_CLAIM;
use crate::detail::CONTROL_END_ALLOWED;
use crate::detail::CONTROL_HOST;
use crate::detail::CONTROL_KICKED;
use crate::feed::Summary;
use crate::observer;
use crate::observer::EngineDirs;
use crate::pause;
use crate::registry::MatchId;
use crate::registry::TURN_LENGTH;
use crate::score;
use crate::seats::Phase;
use crate::seats::Seats;
use crate::settings::Choice;
use crate::settings::Launch;
use crate::settings::Settings;
use crate::summary_envelope;
use crate::view::Hub;
use crate::view::SummaryBody;
use crate::view::spawn_brotli;
use crate::wire::Writer;

const PLAY_SCRIPT: &str = "scripts/play.sh";
/// Where the engine finds the settings file (`EngineDirs::settings_file`).
const SETTINGS_VFS_PATH: &str = "config/matchsettings.mp.json";
const UPLINK_COMMAND: u8 = 1;
const UPLINK_QUERY: u8 = 2;
const UPLINK_PAUSE: u8 = 3;
const UPLINK_SUMMARY: u8 = 4;
/// `SpeculaFeed.h`'s query methods.
const QUERY_BUILDING_PREVIEW: u8 = 1;
const QUERY_FOUNDATION_SNAP: u8 = 2;
const QUERY_WALL_PREVIEW: u8 = 3;
/// Replies not yet sent to the pages; a placement asks at most a couple at
/// a time.
const REPLY_BACKLOG: usize = 64;
/// How many unanswered queries are remembered: the engine never answers one
/// whose uplink dropped mid-query, and they must not pile up.
const ASKERS_KEPT: usize = 256;
/// What every order the GUI gives but walk names, as
/// `g_AutoFormation.getNull()` with `formationwalkonly`
/// (`globalscripts/Formation.js`).
const NULL_FORMATION: &str = "special/formations/null";
/// `feed::Player::state`.
const PLAYER_ACTIVE: u8 = 0;
const PLAYER_DEFEATED: u8 = 1;
/// D44: away this long with no pause budget left, a seat has left.
const ABSENT_AFTER: Duration = Duration::from_secs(5 * 60);
/// D45: the host may end the game freely before this much game time.
const END_FREE: Duration = Duration::from_secs(60);
/// D17: with no human seat connected for this long the match ends.
const NOBODY_CONNECTED_END: Duration = Duration::from_secs(5 * 60);
/// How long a time limit waits for the capture its summary request asks for
/// before scoring the newest it already holds (M30).
const LIMIT_SUMMARY_WAIT: Duration = Duration::from_secs(5);
/// How long the engine runs on after the resigns so that it processes them
/// and writes the final summary (M30).
const LIMIT_RESIGN_WAIT: Duration = Duration::from_secs(10);
/// The final summary is written after the turn that saw the last resign;
/// one poll later is enough to have it.
const LIMIT_RESIGN_SEEN: Duration = Duration::from_secs(1);
/// The resign the engine takes as the seat's own (`Commands.js` `resign`).
const RESIGN_JSON: &str = r#"{"type":"resign"}"#;

/// A query's answer, for the one command connection that asked.
#[derive(Clone)]
pub struct Reply {
    pub connection: u64,
    pub seat: u8,
    /// `{"id":<the page's id>,"result":<JSON>}`, as the page reads it.
    pub text: String,
}

/// Who asked a query the uplink sent under `uplink_id`.
struct Asker {
    uplink_id: u32,
    connection: u64,
    seat: u8,
    id: u32,
}

/// What the player's page reads: whether a game is running and is the
/// visitor's, whether its seat is defeated and whether the visitor's last
/// game can be repeated.
pub fn setup_json(status: &Status) -> String {
    let mut json = Json::new();
    json.boolean("running", status.running);
    json.boolean("mine", status.mine);
    json.boolean("defeated", status.defeated);
    json.boolean("watch", status.watch);
    json.boolean("rematch", status.rematch);
    json.text(
        "next",
        &status.next.map(|id| id.to_string()).unwrap_or_default(),
    );
    json.raw("seat", &status.seat.unwrap_or(0).to_string());
    if let Some((wall_left, game_limit)) = status.limits {
        json.raw("wall_left_ms", &wall_left.as_millis().to_string());
        json.raw("game_limit_ms", &game_limit.as_millis().to_string());
    }
    json.finish()
}

/// What the new game form reads (`GET /new`): whether every slot is taken,
/// whether the visitor hosts a game and which, the newest running one. The
/// ids are UUIDs, empty for none.
pub fn form_json(
    running: bool,
    own: Option<MatchId>,
    latest: Option<MatchId>,
    stopping: bool,
) -> String {
    let mut json = Json::new();
    json.boolean("running", running);
    json.boolean("mine", own.is_some());
    json.boolean("stopping", stopping);
    json.text("match", &own.map(|id| id.to_string()).unwrap_or_default());
    json.text(
        "latest",
        &latest.map(|id| id.to_string()).unwrap_or_default(),
    );
    json.finish()
}

/// The visitor's view of the game, for `setup_json`.
pub struct Status {
    pub running: bool,
    pub mine: bool,
    /// The visitor's seat is not active any more: they resigned or were
    /// beaten while the others play on.
    pub defeated: bool,
    /// The visitor's seat is defeated with no mutual ally left in the game:
    /// its page shows the observers' view (D47).
    pub watch: bool,
    /// The visitor hosts the last game, which a rematch repeats.
    pub rematch: bool,
    /// The visitor's seat while the game is theirs, running or not, which
    /// the summary reads after the engine has exited.
    pub seat: Option<u8>,
    /// A running match's wall time left and game limit (`Registry::limits`),
    /// for the pages' warnings; the registry fills it in.
    pub limits: Option<(Duration, Duration)>,
    /// The setup the visitor follows the same game into (M35), while it is
    /// still open; the registry fills it in.
    pub next: Option<MatchId>,
}

pub struct Play {
    socket: String,
    port: u16,
    dirs: EngineDirs,
    /// Server-wide launch counter, for the log line only; set at the start.
    generation: AtomicU64,
    hub: Arc<Hub>,
    seats: Arc<Seats>,
    inner: Mutex<Inner>,
    /// One id per command socket.
    next_connection: AtomicU64,
    /// The id the next query goes to the engine under.
    next_query: AtomicU32,
    /// Who asked each query the engine has not answered yet: a leaf lock,
    /// never take `inner` while holding it.
    askers: Mutex<VecDeque<Asker>>,
    /// The engine's answers to the queries, each for the connection that
    /// asked, as the text message `{"id":..,"result":..}` the page reads.
    replies: broadcast::Sender<Reply>,
    /// Held for a whole pause update, so the engine and the pages get the
    /// ledger's decisions in the order the ledger made them. After the
    /// registry lock (`start` runs under it), before every lock of this,
    /// `Seats` and `Hub`, and never while one of those is held.
    pause_order: Mutex<()>,
    /// The pause ledger (D41 to D43): a leaf lock, never take `seats`,
    /// `hub` or `inner` while holding it.
    ledger: Mutex<pause::Ledger>,
    /// The human seats at launch, ascending.
    humans: Mutex<Vec<u8>>,
    /// The in-game control (D17, D44 to D46, D54): a leaf lock, never take
    /// `seats`, `hub`, `ledger` or `inner` while holding it.
    control: Mutex<ControlState>,
}

/// What the host and the rules of leaving have decided so far in this game.
#[derive(Default)]
struct ControlState {
    /// Kicked by the host (V77); the seat is gone from the seat table.
    kicked: BTreeSet<u8>,
    /// The cookie of each kicked seat, since the seat table has forgotten
    /// it, so a later ban can still reach it (D54).
    kicked_cookie: BTreeMap<u8, String>,
    /// Banned by a kick with `ban` (D54).
    banned: BTreeSet<u8>,
    /// Resigned by the server for a claim or by D17, so that a resign is
    /// sent once.
    server_resigned: BTreeSet<u8>,
    /// When no human seat has been connected, from the first poll that saw
    /// it.
    nobody_since: Option<Instant>,
    /// The time limit's scoring, once it is reached (M30).
    limit: Option<LimitPhase>,
}

/// How far a time limit has settled the match (M30).
#[derive(Clone)]
enum LimitPhase {
    /// Pauses lifted, a summary asked for after `turn`.
    Asked { since: Instant, turn: u32 },
    /// The losers were resigned.
    Resigned { since: Instant, losers: Vec<u8> },
}

/// What the control rules read, taken once (`Play::facts`).
struct Facts {
    /// The human seats at launch, ascending.
    humans: Vec<u8>,
    host: Option<u8>,
    /// Active in the engine.
    active: BTreeSet<u8>,
    /// Not disconnected for `DISCONNECT_GRACE` or longer.
    connected: BTreeSet<u8>,
    kicked: BTreeSet<u8>,
    banned: BTreeSet<u8>,
    /// No pause time left and no `play/command` socket for `ABSENT_AFTER`.
    absent: BTreeSet<u8>,
    game_time: Duration,
}

impl Facts {
    /// Kicked or absent: the seats that have left the game (D44).
    fn has_left(&self, player: u8) -> bool {
        self.kicked.contains(&player) || self.absent.contains(&player)
    }

    /// Active and not left: still in the game.
    fn in_game(&self, player: u8) -> bool {
        self.active.contains(&player) && !self.has_left(player)
    }

    /// The active human seats that have left, kicked ones included.
    fn left_active(&self) -> Vec<u8> {
        self.humans
            .iter()
            .copied()
            .filter(|&player| self.active.contains(&player) && self.has_left(player))
            .collect()
    }

    /// Whether `seat` may claim victory now (the user's decision 4).
    fn may_claim(&self, seat: u8, allies: bool) -> bool {
        self.humans.contains(&seat)
            && self.active.contains(&seat)
            && self.connected.contains(&seat)
            && !self.has_left(seat)
            && !self.left_active().is_empty()
            && allies
    }

    /// Whether `seat` may end the game (D45).
    fn may_end(&self, seat: u8) -> bool {
        self.host == Some(seat)
            && (self.game_time < END_FREE
                || self
                    .humans
                    .iter()
                    .all(|&human| human == seat || !self.in_game(human)))
    }
}

#[derive(Default)]
struct Inner {
    game: Option<Game>,
    /// The engine's end of the feed socket while it is connected.
    uplink: Option<UnixStream>,
}

struct Game {
    running: bool,
    /// What the engine started with: the resolved settings and the attributes
    /// it was given. Kept to compare with the engine's report (M21) and for
    /// the rematch's replay of the draw (M35).
    launch: Launch,
    /// Every statistics capture of each human seat, by player number, kept
    /// after the engine exits.
    summaries: HashMap<u8, Vec<Arc<SeatCapture>>>,
    /// The newest full capture and its turn, for the time limit's scoring.
    last_full: Option<(u32, Arc<[u8]>)>,
    /// The newest full capture, every player, served to the seats once the
    /// match is over (the decision or the engine's exit).
    full: Option<Arc<SeatCapture>>,
    /// The engine sent its final capture: the game is decided.
    decided: bool,
}

/// One capture of a seat, and its envelope once compressed.
struct SeatCapture {
    turn: u32,
    inner: Mutex<SeatBody>,
}

struct SeatBody {
    raw: Option<Arc<[u8]>>,
    brotli: Option<Arc<[u8]>>,
}

impl Play {
    pub fn new(
        socket: String,
        port: u16,
        hub: Arc<Hub>,
        dirs: EngineDirs,
        seats: Arc<Seats>,
    ) -> Self {
        Play {
            socket,
            port,
            dirs,
            generation: AtomicU64::new(0),
            hub,
            seats,
            inner: Mutex::new(Inner::default()),
            next_connection: AtomicU64::new(0),
            next_query: AtomicU32::new(1),
            askers: Mutex::new(VecDeque::new()),
            replies: broadcast::channel(REPLY_BACKLOG).0,
            pause_order: Mutex::new(()),
            ledger: Mutex::new(pause::Ledger::new(&[])),
            humans: Mutex::new(Vec::new()),
            control: Mutex::new(ControlState::default()),
        }
    }

    /// Starts the game for the seats the table holds and gives the thread
    /// that runs its engine. `previous` holds the runner of the game this one
    /// replaces in the slot, joined first: the UDP port is the slot's. It is
    /// taken only once nothing here can fail, so a refused start leaves it
    /// for the next try.
    /// `on_exit` runs once the engine is gone and no lock of this is held.
    pub fn start(
        self: &Arc<Self>,
        launch: Launch,
        previous: &Mutex<Option<JoinHandle<()>>>,
        generation: u64,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> Result<JoinHandle<()>, &'static str> {
        // `main.cpp` wants a non-empty `-autostart=` with
        // `-autostart-nonvisual`; the script reads the settings file and
        // ignores it.
        let map = match &launch.settings.map.map {
            Some(Choice::Fixed(path)) => path.strip_prefix("maps/").unwrap_or(path).to_string(),
            _ => return Err("the settings name no map"),
        };
        let file = self
            .dirs
            .settings_file()
            .ok_or("cannot write the settings file")?;
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&file, launch.attributes.to_string()));
        if let Err(error) = written {
            eprintln!("cannot write the settings file {}: {error}", file.display());
            return Err("cannot write the settings file");
        }
        let args = [
            format!("-autostart={map}"),
            format!("-autostart-specula-settings={SETTINGS_VFS_PATH}"),
        ];
        let previous = previous.lock().unwrap().take();
        self.generation.store(generation, Ordering::SeqCst);
        self.inner.lock().unwrap().game = Some(Game {
            running: true,
            launch,
            summaries: HashMap::new(),
            last_full: None,
            full: None,
            decided: false,
        });
        let host = self.seats.host().unwrap_or_default();
        println!("game {generation} for {host}: {}", args.join(" "));
        // The engine connects the feed after this, so its first turn
        // already has the seats' streams.
        let player_list = self.seats.players();
        let players = player_list.iter().copied().map(u32::from).collect();
        let mut humans = player_list.clone();
        humans.sort_unstable();
        *self.humans.lock().unwrap() = humans;
        self.hub.set_seats(players);
        self.seats.start_presence();
        {
            // A tick in progress must not finish against the fresh ledger.
            let _order = self.pause_order.lock().unwrap();
            *self.ledger.lock().unwrap() = pause::Ledger::new(&player_list);
            *self.control.lock().unwrap() = ControlState::default();
        }
        let play = Arc::clone(self);
        Ok(std::thread::spawn(move || {
            if let Some(previous) = previous {
                let _ = previous.join();
            }
            let mut command = Command::new(PLAY_SCRIPT);
            command
                .arg(&play.socket)
                .arg(play.port.to_string())
                .args(&args);
            match observer::run_engine(
                command,
                "hosting a game",
                &play.socket,
                &play.dirs,
                &play.hub,
            ) {
                Ok(outcome) => println!("play host {outcome}"),
                Err(error) => eprintln!("cannot run the play host: {error}"),
            }
            {
                let mut inner = play.inner.lock().unwrap();
                if let Some(game) = inner.game.as_mut() {
                    game.running = false;
                }
            }
            on_exit();
        }))
    }

    /// Compares what the engine reports it launched with to what was sent,
    /// and logs every difference (D29). It never changes the game.
    pub fn check_attributes(&self, json: &[u8]) {
        let launched: Value = match serde_json::from_slice(json) {
            Ok(value) => value,
            Err(error) => {
                let generation = self.generation.load(Ordering::SeqCst);
                eprintln!("game {generation}: the engine's attributes are not JSON: {error}");
                return;
            }
        };
        let sent = {
            let inner = self.inner.lock().unwrap();
            match inner.game.as_ref() {
                Some(game) => game.launch.attributes.clone(),
                None => return,
            }
        };
        let generation = self.generation.load(Ordering::SeqCst);
        let lines = crate::settings::differences(&sent, &launched);
        if lines.is_empty() {
            println!("game {generation}: the engine launched with the attributes sent");
            return;
        }
        eprintln!("game {generation}: the engine launched with other attributes than sent (D29):");
        for line in lines {
            eprintln!("  {line}");
        }
    }

    /// The player number `visitor` commands, if they hold a seat.
    pub fn seat(&self, visitor: Option<&str>) -> Option<u8> {
        self.seats.player_of(visitor?)
    }

    /// Where the map puts `seat`'s camera at the start (x, z), as the engine
    /// hands it to `SetStartingCamera`; none when the map gives no such
    /// camera, so the page falls back on the seat's base as vanilla does.
    pub fn starting_camera(&self, seat: u8) -> Option<[f32; 2]> {
        let index = usize::from(seat.checked_sub(1)?);
        let inner = self.inner.lock().unwrap();
        let attributes = &inner.game.as_ref()?.launch.attributes;
        let position = &attributes["settings"]["PlayerData"][index]["StartingCamera"]["Position"];
        Some([
            position["x"].as_f64()? as f32,
            position["z"].as_f64()? as f32,
        ])
    }

    /// An id for one command socket.
    pub fn connection(&self) -> u64 {
        self.next_connection.fetch_add(1, Ordering::Relaxed)
    }

    /// Counts a seated page's `play/command` socket while the guard lives;
    /// a returning seat lifts its automatic pause at once (M25).
    pub fn present(self: &Arc<Self>, cookie: &str) -> Presence {
        self.seats.connect_play(cookie);
        self.tick_pause();
        Presence {
            play: Arc::clone(self),
            cookie: cookie.to_string(),
        }
    }

    /// Whether the engine reported `PLAYER_ACTIVE` for `player`. A kicked
    /// seat is out of the seat table, so it is told as away and inactive
    /// here: its holds and its veil go (D54).
    fn pause_inputs(&self) -> Vec<pause::Input> {
        let kicked: Vec<u8> = self
            .control
            .lock()
            .unwrap()
            .kicked
            .iter()
            .copied()
            .collect();
        let mut inputs: Vec<pause::Input> = self
            .seats
            .presence(crate::seats::DISCONNECT_GRACE)
            .into_iter()
            .map(|(player, connected)| pause::Input {
                player,
                connected,
                active: self.hub.player_state(u32::from(player)) == Some(PLAYER_ACTIVE),
            })
            .collect();
        inputs.extend(kicked.into_iter().map(|player| pause::Input {
            player,
            connected: false,
            active: false,
        }));
        inputs
    }

    /// The `inner.game.running` test of `status`, `running_seat` and
    /// `decided`.
    pub(crate) fn game_running(&self) -> bool {
        self.inner
            .lock()
            .unwrap()
            .game
            .as_ref()
            .is_some_and(|game| game.running)
    }

    /// Charges the ledger, applies `change`, and tells the engine and the
    /// pages what it decided. `pause_order` is held for the whole update,
    /// so concurrent updates reach the engine and the pages in the order
    /// the ledger made them; the ledger lock is held only around the ledger
    /// calls. Callers hold no lock of `Play`, `Seats` or `Hub`.
    fn apply_pause(
        &self,
        change: impl FnOnce(&mut pause::Ledger) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let _order = self.pause_order.lock().unwrap();
        let inputs = self.pause_inputs();
        let now = Instant::now();
        let (events, result, changes, report) = {
            let mut ledger = self.ledger.lock().unwrap();
            let events = ledger.advance(now, &inputs);
            let result = change(&mut ledger);
            let changes = ledger.engine_changes();
            let report = ledger.report(now);
            (events, result, changes, report)
        };
        let generation = self.generation.load(Ordering::SeqCst);
        for event in events {
            match event {
                pause::Event::Disconnected(player) => {
                    println!("game {generation}: player {player} disconnected");
                }
                pause::Event::Reconnected(player) => {
                    println!("game {generation}: player {player} reconnected");
                }
                pause::Event::Spent(player) => {
                    println!("game {generation}: player {player} ran out of pause time");
                }
            }
        }
        for (player, on) in changes {
            let record = [UPLINK_PAUSE, 2, 0, 0, 0, player, u8::from(on)];
            self.write_uplink(&record);
        }
        if let Some(entries) = report {
            self.hub.set_ledger(&entries);
        }
        result
    }

    /// Advances the ledger once a poll while a game can be paused.
    pub fn tick_pause(&self) {
        if !self.game_running() || !self.hub.has_turn() {
            return;
        }
        let _ = self.apply_pause(|_| Ok(()));
    }

    pub fn status(&self, visitor: Option<&str>) -> Status {
        let running = self
            .inner
            .lock()
            .unwrap()
            .game
            .as_ref()
            .is_some_and(|game| game.running);
        let seat = self.seat(visitor);
        Status {
            running,
            mine: running && seat.is_some(),
            defeated: running && seat.is_some_and(|seat| self.seat_defeated(seat)),
            watch: running
                && seat.is_some_and(|seat| {
                    self.hub.player_state(u32::from(seat)) == Some(PLAYER_DEFEATED)
                        && self.hub.team_eyes(u32::from(seat)).is_none()
                }),
            rematch: visitor.is_some_and(|visitor| self.seats.is_host(visitor)),
            seat: seat.filter(|&seat| seat != 0),
            limits: None,
            next: None,
        }
    }

    /// Every capture is kept for the human seats' views. Only the newest
    /// full copy is kept, as a seat has no timeline, and it is served
    /// through the seats' route only once the match is over, as it holds the
    /// opponents' statistics.
    pub fn set_summary(&self, summary: Summary) {
        let players = self.seats.players();
        let mut compress = Vec::with_capacity(players.len());
        {
            let mut inner = self.inner.lock().unwrap();
            let Some(game) = inner.game.as_mut() else {
                return;
            };
            game.last_full = Some((summary.turn, Arc::clone(&summary.full)));
            if summary.is_final {
                game.decided = true;
            }
            if let Some(raw) = summary_envelope(summary.turn, summary.is_final, &summary.full) {
                let raw: Arc<[u8]> = raw.into();
                let capture = Arc::new(SeatCapture {
                    turn: summary.turn,
                    inner: Mutex::new(SeatBody {
                        raw: Some(Arc::clone(&raw)),
                        brotli: None,
                    }),
                });
                game.full = Some(Arc::clone(&capture));
                compress.push((capture, raw));
            }
            for player in players {
                let Some(view) = summary
                    .seats
                    .get(usize::from(player))
                    .and_then(|view| view.as_ref())
                else {
                    continue;
                };
                let Some(raw) = summary_envelope(summary.turn, summary.is_final, view) else {
                    continue;
                };
                let raw: Arc<[u8]> = raw.into();
                let capture = Arc::new(SeatCapture {
                    turn: summary.turn,
                    inner: Mutex::new(SeatBody {
                        raw: Some(Arc::clone(&raw)),
                        brotli: None,
                    }),
                });
                let captures = game.summaries.entry(player).or_default();
                // A final capture on the turn of a periodic one replaces it.
                if captures
                    .last()
                    .is_some_and(|last| last.turn == summary.turn)
                {
                    captures.pop();
                }
                captures.push(Arc::clone(&capture));
                compress.push((capture, raw));
            }
        }
        for (capture, raw) in compress {
            spawn_brotli(raw, move |brotli| {
                let mut inner = capture.inner.lock().unwrap();
                inner.brotli = Some(brotli);
                inner.raw = None;
            });
        }
    }

    /// The visitor's seat view at or before `turn`, or the last one, while
    /// the game goes on; every player once it is decided or the engine has
    /// stopped, as vanilla's end summary (D47 keeps a defeated seat on its
    /// own view until then).
    pub fn summary(&self, visitor: Option<&str>, turn: Option<u32>) -> Option<SummaryBody> {
        let player = self.seat(visitor)?;
        let inner = self.inner.lock().unwrap();
        let game = inner.game.as_ref()?;
        let over = game.decided || !game.running;
        let capture = match game
            .full
            .as_ref()
            .filter(|full| over && turn.is_none_or(|turn| full.turn <= turn))
        {
            Some(full) => full,
            None => game
                .summaries
                .get(&player)?
                .iter()
                .rev()
                .find(|capture| turn.is_none_or(|turn| capture.turn <= turn))?,
        };
        let body = capture.inner.lock().unwrap();
        if let Some(brotli) = &body.brotli {
            return Some(SummaryBody {
                bytes: Arc::clone(brotli),
                brotli: true,
            });
        }
        body.raw.as_ref().map(|raw| SummaryBody {
            bytes: Arc::clone(raw),
            brotli: false,
        })
    }

    /// Whether the feed reports `seat` as no longer active. Asked without
    /// `inner` held, so the two locks are never taken in both orders.
    fn seat_defeated(&self, seat: u8) -> bool {
        self.hub
            .player_state(u32::from(seat))
            .is_some_and(|state| state != PLAYER_ACTIVE)
    }

    /// The seat `owner` holds in the running game.
    fn running_seat(&self, owner: &str) -> Result<u8, &'static str> {
        let running = self
            .inner
            .lock()
            .unwrap()
            .game
            .as_ref()
            .is_some_and(|game| game.running);
        match self.seats.player_of(owner) {
            Some(seat) if running => Ok(seat),
            _ => Err("no game of yours is running"),
        }
    }

    /// Whether no human seat is still in the game (D17). Asked without
    /// `inner` held, as `seat_defeated` is.
    fn humans_out(&self) -> bool {
        self.seats
            .players()
            .into_iter()
            .all(|seat| self.seat_defeated(seat))
    }

    /// Resigns `owner`'s seat, as the session's Resign does
    /// (`ResignConfirmation.js`); the others play on.
    pub fn resign(&self, owner: &str) -> Result<(), &'static str> {
        let seat = self.running_seat(owner)?;
        self.send_json(seat, r#"{"type":"resign"}"#);
        Ok(())
    }

    /// A flare from `owner`'s seat, to its mutual allies and the observers
    /// (D51).
    pub fn flare(&self, visitor: &str, x: f32, z: f32) -> Result<(), &'static str> {
        let seat = self.running_seat(visitor)?;
        self.hub.flare_from_seat(u32::from(seat), x, z)
    }

    /// Pauses or resumes the game for `owner`'s seat through the ledger, as
    /// the session's PauseControl does. A seat out of pause time is refused.
    pub fn set_paused(&self, owner: &str, paused: bool) -> Result<(), &'static str> {
        let seat = self.running_seat(owner)?;
        if !self.hub.has_turn() {
            return Err("the game has not started");
        }
        self.apply_pause(|ledger| {
            if paused {
                ledger.hold(seat)
            } else {
                ledger.release(seat);
                Ok(())
            }
        })
    }

    /// Ends the game for everyone if `owner` may end it (`may_end`): the
    /// engine is stopped, and the runner thread and the feed's end take it
    /// from there.
    pub fn end(&self, owner: &str) -> Result<(), &'static str> {
        if !self.game_running() {
            return Err("no game of yours is running");
        }
        if !self.may_end(owner) {
            return Err(
                "the host may end the game only in its first minute, or once every other player has left or resigned",
            );
        }
        let mut inner = self.inner.lock().unwrap();
        match &mut inner.game {
            Some(game) if game.running => {
                self.stop_engine(game);
                Ok(())
            }
            _ => Err("no game of yours is running"),
        }
    }

    /// Whether `owner` may end the running game (D45): the host, and either
    /// in the first minute or once every other human has left or resigned.
    pub fn may_end(&self, owner: &str) -> bool {
        if !self.game_running() {
            return false;
        }
        let Some(seat) = self.seats.player_of(owner) else {
            return false;
        };
        self.facts().may_end(seat)
    }

    /// The control rules' inputs, read one lock at a time in lock order: the
    /// human seats, the seat table, the hub, then the two leaf locks.
    fn facts(&self) -> Facts {
        let humans = self.humans.lock().unwrap().clone();
        let away = self.seats.away();
        let host = self.seats.host_player();
        let active: BTreeSet<u8> = humans
            .iter()
            .copied()
            .filter(|&player| self.hub.player_state(u32::from(player)) == Some(PLAYER_ACTIVE))
            .collect();
        let (kicked, banned) = {
            let control = self.control.lock().unwrap();
            (control.kicked.clone(), control.banned.clone())
        };
        let connected: BTreeSet<u8> = away
            .iter()
            .filter(|(_, away)| away.is_none_or(|away| away < crate::seats::DISCONNECT_GRACE))
            .map(|(player, _)| *player)
            .collect();
        let absent: BTreeSet<u8> = {
            let ledger = self.ledger.lock().unwrap();
            away.iter()
                .filter(|(player, away)| {
                    !kicked.contains(player)
                        && away.is_some_and(|away| away >= ABSENT_AFTER)
                        && ledger
                            .budget(*player)
                            .is_some_and(|budget| budget.is_zero())
                })
                .map(|(player, _)| *player)
                .collect()
        };
        Facts {
            humans,
            host,
            active,
            connected,
            kicked,
            banned,
            absent,
            game_time: TURN_LENGTH * self.hub.turn(),
        }
    }

    /// Whether a claim against the left seats is allowed by the other
    /// players' alliances (the claim rule, decision 4).
    fn allies(&self, facts: &Facts) -> bool {
        let left: Vec<u32> = facts.left_active().into_iter().map(u32::from).collect();
        self.hub.claim_allies(&left)
    }

    /// The control a seat is sent: its own flags, then every human row
    /// (the detail stream's kind 3).
    fn control_body(facts: &Facts, seat: u8, allies: bool) -> Vec<u8> {
        let mut own = 0;
        if facts.may_end(seat) {
            own |= CONTROL_END_ALLOWED;
        }
        if facts.may_claim(seat, allies) {
            own |= CONTROL_CLAIM;
        }
        let mut body = Writer::default();
        body.u8(own);
        body.varint(facts.humans.len() as u64);
        for &human in &facts.humans {
            let mut flags = 0;
            if facts.host == Some(human) {
                flags |= CONTROL_HOST;
            }
            if facts.kicked.contains(&human) {
                flags |= CONTROL_KICKED;
            }
            if facts.banned.contains(&human) {
                flags |= CONTROL_BANNED;
            }
            if facts.absent.contains(&human) {
                flags |= CONTROL_ABSENT;
            }
            body.u8(human);
            body.u8(flags);
        }
        body.0
    }

    /// The seats among `seats` not yet resigned by the server, which are
    /// marked resigned so that a resign is sent once.
    fn mark_resigned(&self, seats: impl Iterator<Item = u8>) -> Vec<u8> {
        let mut control = self.control.lock().unwrap();
        seats
            .filter(|seat| control.server_resigned.insert(*seat))
            .collect()
    }

    /// Runs the rules of leaving and sends every human seat its control
    /// (`hub.set_control`). Called once a poll; the encoder sends only what
    /// changed.
    pub fn tick_control(&self) {
        if !self.game_running() || !self.hub.has_turn() {
            return;
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let mut facts = self.facts();
        // D46: the host powers pass on when the host is no longer in the game.
        // Only to someone still in it: at the decision nobody is, and the
        // host keeps the powers, so the summary's Rematch stays theirs (D63).
        if let Some(host) = facts.host
            && !facts.in_game(host)
            && facts
                .humans
                .iter()
                .any(|&human| human != host && facts.in_game(human))
            && let Some(cookie) = self.seats.host()
        {
            self.seats
                .leave(&cookie, Phase::Running, |player| facts.in_game(player));
            match self.seats.host_player() {
                Some(next) => println!("game {generation}: host powers pass to player {next}"),
                None => println!("game {generation}: host powers pass to nobody"),
            }
            facts = self.facts();
        }
        // D17: the game ends when nobody is connected for a while, or when
        // every active human has left.
        let nobody = !facts.humans.iter().any(|&human| {
            facts.active.contains(&human)
                && !facts.kicked.contains(&human)
                && facts.connected.contains(&human)
        });
        let now = Instant::now();
        let nobody_long = {
            let mut control = self.control.lock().unwrap();
            if nobody {
                now.duration_since(*control.nobody_since.get_or_insert(now)) >= NOBODY_CONNECTED_END
            } else {
                control.nobody_since = None;
                false
            }
        };
        let all_left = facts.humans.iter().all(|&human| !facts.in_game(human))
            && !facts.left_active().is_empty();
        let reason = if nobody_long {
            Some("nobody connected for 5 min, resigning every human seat")
        } else if all_left {
            Some("every player has left, resigning the seats left")
        } else {
            None
        };
        if let Some(reason) = reason {
            let active: Vec<u8> = facts
                .humans
                .iter()
                .copied()
                .filter(|human| facts.active.contains(human))
                .collect();
            let resign = self.mark_resigned(active.into_iter());
            if !resign.is_empty() {
                println!("game {generation}: {reason}");
                for player in resign {
                    self.send_json(player, RESIGN_JSON);
                }
            }
        }
        let allies = self.allies(&facts);
        let bodies: Vec<(u32, Vec<u8>)> = facts
            .humans
            .iter()
            .copied()
            .filter(|human| !facts.kicked.contains(human))
            .map(|human| (u32::from(human), Self::control_body(&facts, human, allies)))
            .collect();
        self.hub.set_control(&bodies);
    }

    /// The seat `owner` holds claims victory (decision 4): every active seat
    /// that has left is resigned, kicked ones included.
    pub fn claim(&self, owner: &str) -> Result<(), &'static str> {
        let seat = self.running_seat(owner)?;
        let facts = self.facts();
        let allies = self.allies(&facts);
        if !facts.may_claim(seat, allies) {
            return Err("there is no victory to claim");
        }
        let left = facts.left_active();
        for player in self.mark_resigned(left.iter().copied()) {
            self.send_json(player, RESIGN_JSON);
        }
        let names: Vec<String> = left.iter().map(u8::to_string).collect();
        println!(
            "game {}: player {seat} claimed victory, resigning {}",
            self.generation.load(Ordering::SeqCst),
            names.join(", ")
        );
        self.tick_control();
        Ok(())
    }

    /// The host kicks a human seat out of the running game (M29, V77), with
    /// a ban when asked (D54). The seat is taken for good; its civ stays
    /// idle.
    pub fn kick(&self, owner: &str, player: u8, ban: bool) -> Result<(), &'static str> {
        if !self.game_running() || !self.seats.is_host(owner) {
            return Err("only the host can kick");
        }
        let humans = self.humans.lock().unwrap().clone();
        if !humans.contains(&player) || self.seats.host_player() == Some(player) {
            return Err("the host cannot be kicked");
        }
        // Checked and marked under one hold of the leaf lock, so two bans
        // of a kicked seat cannot both pass; `seats` is taken after it.
        let kicked_cookie = {
            let mut control = self.control.lock().unwrap();
            if control.kicked.contains(&player) {
                if !ban {
                    return Err("already kicked");
                }
                if !control.banned.insert(player) {
                    return Err("already banned");
                }
                Some(control.kicked_cookie.get(&player).cloned())
            } else {
                None
            }
        };
        if let Some(cookie) = kicked_cookie {
            if let Some(cookie) = cookie {
                self.seats.ban(&cookie);
            }
            self.tick_control();
            println!(
                "game {}: the host banned kicked player {player}",
                self.generation.load(Ordering::SeqCst)
            );
            return Ok(());
        }
        let cookie = self
            .seats
            .cookie_of_player(player)
            .ok_or("not in this match")?;
        self.seats.set_player(&cookie, None)?;
        {
            let mut control = self.control.lock().unwrap();
            control.kicked.insert(player);
            control.kicked_cookie.insert(player, cookie.clone());
            if ban {
                control.banned.insert(player);
            }
        }
        if ban {
            self.seats.ban(&cookie);
        }
        // The kicked seat's page gets its control before its stream is
        // dropped, so it can say it was kicked.
        let facts = self.facts();
        let allies = self.allies(&facts);
        let body = Self::control_body(&facts, player, allies);
        self.hub.set_control(&[(u32::from(player), body)]);
        let kicked = self.control.lock().unwrap().kicked.clone();
        let remaining: Vec<u32> = humans
            .iter()
            .filter(|human| !kicked.contains(human))
            .map(|&human| u32::from(human))
            .collect();
        self.hub.set_seats(remaining);
        self.tick_pause();
        self.tick_control();
        println!(
            "game {}: the host {} player {player}",
            self.generation.load(Ordering::SeqCst),
            if ban { "banned" } else { "kicked" }
        );
        Ok(())
    }

    /// The host bans an unseated member, an observer, from the match's chat
    /// (D54); a holder of a seat goes through `kick` instead.
    pub fn ban_observer(&self, owner: &str, member: usize) -> Result<(), &'static str> {
        if !self.game_running() || !self.seats.is_host(owner) {
            return Err("only the host can ban");
        }
        let cookie = self.seats.cookie_at(member).ok_or("no such member")?;
        if self.seats.is_host(&cookie) {
            return Err("the host cannot be banned");
        }
        if self.seats.player_of(&cookie).is_some() {
            return Err("use Kick for a player");
        }
        if self.seats.is_banned(&cookie) {
            return Err("already banned");
        }
        self.seats.ban(&cookie);
        println!(
            "game {}: the host banned observer {member}",
            self.generation.load(Ordering::SeqCst)
        );
        Ok(())
    }

    /// Advances a reached time limit (M30, D20): lifts every pause, asks the
    /// engine for a fresh summary, scores the active players' mutual-ally
    /// groups at it and resigns every loser. Returns whether the engine
    /// should be stopped now.
    pub fn tick_limit(&self, limit: &str) -> bool {
        if !self.game_running() {
            return false;
        }
        let phase = self.control.lock().unwrap().limit.clone();
        match phase {
            None => {
                let _ = self.apply_pause(|ledger| {
                    ledger.end();
                    Ok(())
                });
                let turn = self.hub.turn();
                self.write_uplink(&[UPLINK_SUMMARY, 0, 0, 0, 0]);
                self.control.lock().unwrap().limit = Some(LimitPhase::Asked {
                    since: Instant::now(),
                    turn,
                });
                println!(
                    "game {}: {limit} time limit, scoring",
                    self.generation.load(Ordering::SeqCst)
                );
                false
            }
            Some(LimitPhase::Asked { since, turn }) => {
                let last = {
                    let inner = self.inner.lock().unwrap();
                    inner.game.as_ref().and_then(|game| game.last_full.clone())
                };
                let fresh = last.as_ref().is_some_and(|(at, _)| *at > turn);
                if !fresh && since.elapsed() < LIMIT_SUMMARY_WAIT {
                    return false;
                }
                let generation = self.generation.load(Ordering::SeqCst);
                let Some((_, full)) = last else {
                    println!("game {generation}: no summary, stopping undecided");
                    return true;
                };
                let Some(scores) = score::scores(&full) else {
                    println!("game {generation}: the summary cannot be read, stopping undecided");
                    return true;
                };
                let captured = serde_json::from_slice::<Value>(&full)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("playerStates")
                            .and_then(Value::as_array)
                            .map(|states| states.len().saturating_sub(1))
                    });
                let active = self.active_players(captured);
                // Kicked and absent seats have left the game (D44, D54), so
                // their idle civs never win on score: they only lose.
                let left = self.facts().left_active();
                let competing: Vec<u8> = active
                    .iter()
                    .copied()
                    .filter(|player| !left.contains(player))
                    .collect();
                let outcome = score::decide(&scores, &competing);
                let groups = score::groups(&scores, &competing);
                let losers: Vec<u8> = match &outcome {
                    score::Outcome::Winners(winners) => active
                        .iter()
                        .copied()
                        .filter(|player| !winners.contains(player))
                        .collect(),
                    score::Outcome::Draw => active.clone(),
                };
                let shown: Vec<String> = groups
                    .iter()
                    .map(|(members, total)| {
                        format!(
                            "{}={total}",
                            members
                                .iter()
                                .map(u8::to_string)
                                .collect::<Vec<_>>()
                                .join("+")
                        )
                    })
                    .collect();
                match &outcome {
                    score::Outcome::Winners(winners) => println!(
                        "game {generation}: {limit} time limit, winners {} ({})",
                        winners
                            .iter()
                            .map(u8::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                        shown.join(", ")
                    ),
                    score::Outcome::Draw => println!(
                        "game {generation}: {limit} time limit, draw ({})",
                        shown.join(", ")
                    ),
                }
                for player in self.mark_resigned(losers.iter().copied()) {
                    self.send_json(player, RESIGN_JSON);
                }
                self.control.lock().unwrap().limit = Some(LimitPhase::Resigned {
                    since: Instant::now(),
                    losers: losers.clone(),
                });
                losers.is_empty()
            }
            Some(LimitPhase::Resigned { since, losers }) => {
                let all_out = since.elapsed() >= LIMIT_RESIGN_SEEN
                    && losers.iter().all(|player| {
                        self.hub.player_state(u32::from(*player)) != Some(PLAYER_ACTIVE)
                    });
                all_out || since.elapsed() >= LIMIT_RESIGN_WAIT
            }
        }
    }

    /// The players still active in the engine when a time limit scores: the
    /// capture's own players, or 1 to 8 while it has none.
    fn active_players(&self, captured: Option<usize>) -> Vec<u8> {
        let count = captured.unwrap_or(8);
        (1..=count)
            .filter_map(|index| u8::try_from(index).ok())
            .filter(|player| self.hub.player_state(u32::from(*player)) == Some(PLAYER_ACTIVE))
            .collect()
    }

    fn stop_engine(&self, game: &mut Game) {
        observer::kill_recorded(&self.socket);
        // At once rather than once the runner has seen the engine exit, so
        // the page reloaded after this shows the end.
        game.running = false;
    }

    /// Stops the engine of the host's game that a new setup replaces (D14),
    /// once `may_end` allowed it: asking `may_end` again could refuse after
    /// minute 1 has passed since, and leave the engine in the new setup's
    /// slot.
    pub fn end_replaced(&self) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(game) = inner.game.as_mut().filter(|game| game.running) {
            self.stop_engine(game);
        }
    }

    /// Stops the engine at a time limit (D18, D19). Returns whether it was
    /// still running.
    pub fn end_at_limit(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match &mut inner.game {
            Some(game) if game.running => {
                self.stop_engine(game);
                true
            }
            _ => false,
        }
    }

    /// Stops the engine of a game nobody plays any more, as `end` does but
    /// for no host: a stopping server must not wait on a won game whose
    /// engine stays in its session. Returns whether it stopped one.
    pub fn end_decided(&self) -> bool {
        let out = self.humans_out();
        let mut inner = self.inner.lock().unwrap();
        match &mut inner.game {
            Some(game) if game.running && out => {
                println!(
                    "game {} is decided, stopping its engine",
                    self.generation.load(Ordering::SeqCst)
                );
                self.stop_engine(game);
                true
            }
            _ => false,
        }
    }

    /// Whether the game runs on with no seat left to play it (D17).
    pub fn decided(&self) -> bool {
        let running = self
            .inner
            .lock()
            .unwrap()
            .game
            .as_ref()
            .is_some_and(|game| game.running);
        running && self.humans_out()
    }

    /// Who hosts the game and the settings it was set up with, which
    /// outlive the `Play`. None once the host powers found nobody to pass
    /// to.
    pub fn owner_settings(&self) -> Option<(String, Settings)> {
        let host = self.seats.host()?;
        let inner = self.inner.lock().unwrap();
        let game = inner.game.as_ref()?;
        Some((host, rematch_settings(&game.launch)))
    }

    /// The settings of the game `owner` hosts, which a rematch repeats.
    pub fn rematch_settings_of(&self, owner: &str) -> Option<Settings> {
        if !self.seats.is_host(owner) {
            return None;
        }
        let inner = self.inner.lock().unwrap();
        Some(rematch_settings(&inner.game.as_ref()?.launch))
    }

    pub fn set_uplink(&self, stream: Option<UnixStream>) {
        self.inner.lock().unwrap().uplink = stream;
    }

    /// Posts `order` for `player`; dropped when no engine is connected.
    pub fn send(&self, player: u8, order: &Order) {
        self.send_json(player, &order_json(order));
    }

    fn send_json(&self, player: u8, json: &str) {
        let mut record = Vec::with_capacity(json.len() + 6);
        record.push(UPLINK_COMMAND);
        record.extend_from_slice(&(json.len() as u32 + 1).to_le_bytes());
        record.push(player);
        record.extend_from_slice(json.as_bytes());
        self.write_uplink(&record);
    }

    /// Asks the engine `query` as `player` would; the answer comes back as
    /// a feed record, to `reply`.
    pub fn query(&self, connection: u64, player: u8, query: &Query) {
        let uplink_id = self.next_query.fetch_add(1, Ordering::Relaxed);
        {
            let mut askers = self.askers.lock().unwrap();
            askers.push_back(Asker {
                uplink_id,
                connection,
                seat: player,
                id: query.id,
            });
            while askers.len() > ASKERS_KEPT {
                askers.pop_front();
            }
        }
        let (method, json) = query_json(&query.kind);
        let mut record = Vec::with_capacity(json.len() + 11);
        record.push(UPLINK_QUERY);
        record.extend_from_slice(&(json.len() as u32 + 6).to_le_bytes());
        record.extend_from_slice(&uplink_id.to_le_bytes());
        record.push(player);
        record.push(method);
        record.extend_from_slice(json.as_bytes());
        self.write_uplink(&record);
    }

    /// `id` is the uplink id the query was sent under.
    pub fn reply(&self, id: u32, json: &[u8]) {
        let Ok(json) = std::str::from_utf8(json) else {
            return;
        };
        let asker = {
            let mut askers = self.askers.lock().unwrap();
            let index = askers.iter().position(|asker| asker.uplink_id == id);
            index.and_then(|index| askers.remove(index))
        };
        let Some(asker) = asker else {
            return;
        };
        // No receiver is no page to tell.
        let _ = self.replies.send(Reply {
            connection: asker.connection,
            seat: asker.seat,
            text: format!(r#"{{"id":{},"result":{json}}}"#, asker.id),
        });
    }

    pub fn replies(&self) -> broadcast::Receiver<Reply> {
        self.replies.subscribe()
    }

    fn write_uplink(&self, record: &[u8]) {
        let mut inner = self.inner.lock().unwrap();
        let Some(uplink) = &mut inner.uplink else {
            return;
        };
        if let Err(error) = uplink.write_all(record) {
            eprintln!("cannot send a command to the engine: {error}");
            inner.uplink = None;
        }
    }
}

/// Counts a seated page's `play/command` socket while it lives (M25).
pub struct Presence {
    play: Arc<Play>,
    cookie: String,
}

impl Drop for Presence {
    fn drop(&mut self) {
        self.play.seats.disconnect_play(&self.cookie);
    }
}

/// The settings a rematch opens a setup from (M35): the same resolved draw,
/// but the AI seed is drawn again, as vanilla's rematch would.
fn rematch_settings(launch: &Launch) -> Settings {
    let mut settings = launch.settings.clone();
    settings.seeds.ai_seed = Choice::Random;
    settings
}

/// The engine's command JSON for `order`, field for field as the session
/// GUI posts it (`gui/session/unit_actions.js`, `input.js`,
/// `selection_panels_helpers.js`).
fn order_json(order: &Order) -> String {
    let mut out = Json::new();
    match order {
        Order::Walk {
            flags,
            entities,
            x,
            z,
            formation,
        } => {
            out.text("type", "walk");
            out.entities("entities", entities);
            out.number("x", *x);
            out.number("z", *z);
            out.order_flags(*flags);
            // `g_AutoFormation.getDefault()`, undefined when it is null.
            if !formation.is_empty() {
                out.text("formation", formation);
            }
        }
        Order::AttackWalk {
            flags,
            entities,
            x,
            z,
        } => {
            out.text("type", "attack-walk");
            out.entities("entities", entities);
            out.number("x", *x);
            out.number("z", *z);
            out.raw("targetClasses", r#"{"attack":["Unit","Structure"]}"#);
            out.order_flags(*flags);
            out.text("formation", NULL_FORMATION);
        }
        Order::Attack {
            flags,
            entities,
            target,
        } => {
            out.text("type", "attack");
            out.entities("entities", entities);
            out.raw("target", &target.to_string());
            out.boolean("allowCapture", flags & ALLOW_CAPTURE != 0);
            out.order_flags(*flags);
            out.text("formation", NULL_FORMATION);
        }
        Order::Gather {
            flags,
            entities,
            target,
        } => out.targeted("gather", *flags, entities, *target),
        Order::Repair {
            flags,
            entities,
            target,
        } => {
            out.targeted("repair", *flags, entities, *target);
            out.boolean("autocontinue", true);
        }
        Order::Garrison {
            flags,
            entities,
            target,
        } => out.targeted("garrison", *flags, entities, *target),
        Order::ReturnResource {
            flags,
            entities,
            target,
        } => out.targeted("returnresource", *flags, entities, *target),
        Order::UnloadAll { holders } => {
            out.text("type", "unload-all");
            out.entities("garrisonHolders", holders);
        }
        Order::Construct {
            flags,
            entities,
            template,
            x,
            z,
            angle,
            seed,
        } => {
            out.text("type", "construct");
            out.text("template", template);
            out.number("x", *x);
            out.number("z", *z);
            out.number("angle", *angle);
            out.raw("actorSeed", &seed.to_string());
            out.entities("entities", entities);
            out.boolean("autorepair", true);
            out.boolean("autocontinue", true);
            out.order_flags(*flags);
            out.text("formation", NULL_FORMATION);
        }
        Order::Train {
            entities,
            template,
            count,
        } => {
            out.text("type", "train");
            out.text("template", template);
            out.raw("count", &count.to_string());
            out.entities("entities", entities);
            out.boolean("pushFront", false);
        }
        Order::Research {
            flags,
            entity,
            template,
        } => {
            out.text("type", "research");
            out.raw("entity", &entity.to_string());
            out.text("template", template);
            out.boolean("pushFront", flags & PUSH_FRONT != 0);
        }
        Order::Stop { flags, entities } => {
            out.text("type", "stop");
            out.entities("entities", entities);
            out.boolean("queued", flags & QUEUED != 0);
        }
        Order::Delete { entities } => {
            out.text("type", "delete-entities");
            out.entities("entities", entities);
        }
        Order::Rally {
            flags,
            entities,
            x,
            z,
            order,
            target,
            resource_type,
            resource_template,
            source,
        } => {
            out.text("type", "set-rallypoint");
            out.entities("entities", entities);
            out.number("x", *x);
            out.number("z", *z);
            let mut data = Json::new();
            data.text("command", order);
            // `RallyPointCommands.js` sends an attack-walk's targetClasses
            // from the data, as the GUI's set-rallypoint puts them there.
            if order == "attack-walk" {
                data.raw("targetClasses", r#"{"attack":["Unit","Structure"]}"#);
            }
            if *target != 0 {
                data.raw("target", &target.to_string());
            }
            if *source != 0 {
                data.raw("source", &source.to_string());
            }
            // Without them, a unit sent to the rally point after its
            // resource ran out throws in `GatherNearPosition`, which aborts
            // the unload or the batch that sent it.
            if let Some((generic, specific)) = resource_type.split_once('.') {
                let mut kind = Json::new();
                kind.text("generic", generic);
                kind.text("specific", specific);
                data.raw("resourceType", &kind.finish());
                data.text("resourceTemplate", resource_template);
            }
            out.raw("data", &data.finish());
            out.boolean("queued", flags & QUEUED != 0);
        }
        Order::StopProduction { entity, id } => {
            out.text("type", "stop-production");
            out.raw("entity", &entity.to_string());
            out.raw("id", &id.to_string());
        }
        Order::Autoqueue { on, entities } => {
            out.text("type", if *on { "autoqueue-on" } else { "autoqueue-off" });
            out.entities("entities", entities);
        }
        Order::Stance { entities, name } => {
            out.text("type", "stance");
            out.entities("entities", entities);
            out.text("name", name);
        }
        Order::Formation { entities, name } => {
            out.text("type", "formation");
            out.entities("entities", entities);
            out.text("formation", name);
        }
        Order::ConstructWall {
            flags,
            entities,
            tower,
            pieces,
            start_snapped,
            end_snapped,
        } => {
            out.text("type", "construct-wall");
            out.boolean("autorepair", true);
            out.boolean("autocontinue", true);
            out.boolean("queued", flags & QUEUED != 0);
            out.entities("entities", entities);
            let mut templates = Json::new();
            templates.text("tower", tower);
            let mut wall_set = Json::new();
            wall_set.raw("templates", &templates.finish());
            out.raw("wallSet", &wall_set.finish());
            let list: Vec<String> = pieces
                .iter()
                .map(|piece| {
                    let mut item = Json::new();
                    item.text("template", &piece.template);
                    item.number("x", piece.x);
                    item.number("z", piece.z);
                    item.number("angle", piece.angle);
                    item.finish()
                })
                .collect();
            out.raw("pieces", &format!("[{}]", list.join(",")));
            if *start_snapped != 0 {
                out.raw("startSnappedEntity", &start_snapped.to_string());
            }
            if *end_snapped != 0 {
                out.raw("endSnappedEntity", &end_snapped.to_string());
            }
            out.text("formation", NULL_FORMATION);
        }
        Order::UnloadAllByOwner { holders } => {
            out.text("type", "unload-all-by-owner");
            out.entities("garrisonHolders", holders);
        }
        Order::UnloadTurrets { entities } => {
            out.text("type", "unload-turrets");
            out.entities("entities", entities);
        }
        Order::LeaveTurret { entities } => {
            out.text("type", "leave-turret");
            out.entities("entities", entities);
        }
        Order::BackToWork { entities } => {
            out.text("type", "back-to-work");
            out.entities("entities", entities);
        }
        Order::RemoveGuard { entities } => {
            out.text("type", "remove-guard");
            out.entities("entities", entities);
        }
        Order::AlertRaise { entities } => {
            out.text("type", "alert-raise");
            out.entities("entities", entities);
        }
        Order::AlertEnd { entities } => {
            out.text("type", "alert-end");
            out.entities("entities", entities);
        }
        Order::Upgrade { entities, template } => {
            out.text("type", "upgrade");
            out.entities("entities", entities);
            out.text("template", template);
            out.boolean("queued", false);
        }
        Order::LockGate { lock, entities } => {
            out.text("type", "lock-gate");
            out.entities("entities", entities);
            out.boolean("lock", *lock);
        }
        Order::Pack { pack, entities } => {
            out.text("type", "pack");
            out.entities("entities", entities);
            out.boolean("pack", *pack);
            out.boolean("queued", false);
        }
        Order::CancelPack { pack, entities } => {
            out.text("type", "cancel-pack");
            out.entities("entities", entities);
            out.boolean("pack", *pack);
            out.boolean("queued", false);
        }
        Order::UnloadTemplate {
            all,
            template,
            owner,
            holders,
        } => {
            out.text("type", "unload-template");
            out.boolean("all", *all);
            out.text("template", template);
            out.raw("owner", &owner.to_string());
            out.entities("garrisonHolders", holders);
        }
        Order::CancelUpgrade { entities } => {
            out.text("type", "cancel-upgrade");
            out.entities("entities", entities);
            out.boolean("queued", false);
        }
        Order::Barter { sell, buy, amount } => {
            out.text("type", "barter");
            out.text("sell", sell);
            out.text("buy", buy);
            out.raw("amount", &amount.to_string());
        }
        Order::SetTradingGoods { goods } => {
            out.text("type", "set-trading-goods");
            let mut shares = Json::new();
            for (code, percent) in goods {
                shares.raw(code, &percent.to_string());
            }
            out.raw("tradingGoods", &shares.finish());
        }
        Order::DropsiteSharing { shared, entities } => {
            out.text("type", "set-dropsite-sharing");
            out.entities("entities", entities);
            out.boolean("shared", *shared);
        }
        Order::Guard {
            flags,
            entities,
            target,
        } => out.targeted("guard", *flags, entities, *target),
        Order::OccupyTurret {
            flags,
            entities,
            target,
        } => out.targeted("occupy-turret", *flags, entities, *target),
        Order::SetupTradeRoute {
            flags,
            entities,
            target,
        } => {
            out.text("type", "setup-trade-route");
            out.entities("entities", entities);
            out.raw("target", &target.to_string());
            out.raw("source", "null");
            out.raw("route", "null");
            out.boolean("queued", flags & QUEUED != 0);
            out.text("formation", NULL_FORMATION);
        }
        Order::CancelTradeRoute {
            flags,
            entities,
            target,
        } => {
            out.text("type", "cancel-setup-trade-route");
            out.entities("entities", entities);
            out.raw("target", &target.to_string());
            out.boolean("queued", flags & QUEUED != 0);
        }
        Order::Patrol {
            flags,
            entities,
            x,
            z,
            target,
            formation,
        } => {
            out.text("type", "patrol");
            out.entities("entities", entities);
            out.number("x", *x);
            out.number("z", *z);
            // `action.target`, undefined over the ground.
            if *target != 0 {
                out.raw("target", &target.to_string());
            }
            out.raw("targetClasses", r#"{"attack":["Unit"]}"#);
            out.boolean("queued", flags & QUEUED != 0);
            out.boolean("allowCapture", flags & ALLOW_CAPTURE != 0);
            if !formation.is_empty() {
                out.text("formation", formation);
            }
        }
        Order::CallToArms {
            flags,
            entities,
            x,
            z,
        } => {
            out.text("type", "call-to-arms");
            out.entities("entities", entities);
            let mut position = Json::new();
            position.number("x", *x);
            position.number("z", *z);
            out.raw("position", &position.finish());
            out.raw("targetClasses", r#"{"attack":["Unit","Structure"]}"#);
            out.order_flags(*flags);
            out.boolean("allowCapture", flags & ALLOW_CAPTURE != 0);
            out.text("formation", NULL_FORMATION);
        }
    }
    out.finish()
}

/// The query method and its argument JSON, as `input.js` passes it to
/// `Engine.GuiInterfaceCall`.
fn query_json(kind: &QueryKind) -> (u8, String) {
    let mut out = Json::new();
    let method = match kind {
        QueryKind::Preview {
            template,
            x,
            z,
            angle,
            seed,
        } => {
            out.text("template", template);
            out.number("x", *x);
            out.number("z", *z);
            out.number("angle", *angle);
            out.raw("actorSeed", &seed.to_string());
            QUERY_BUILDING_PREVIEW
        }
        QueryKind::Snap {
            template,
            x,
            z,
            angle,
            edges,
        } => {
            out.text("template", template);
            out.number("x", *x);
            out.number("z", *z);
            out.number("angle", *angle);
            out.boolean("snapToEdges", *edges);
            QUERY_FOUNDATION_SNAP
        }
        QueryKind::Wall {
            wallset,
            start,
            end,
            snap,
        } => {
            out.text("wallSet", wallset);
            let point = |(x, z): (f32, f32)| {
                let mut point = Json::new();
                point.number("x", x);
                point.number("z", z);
                point.finish()
            };
            if let Some(start) = start {
                out.raw("start", &point(*start));
            }
            if let Some(end) = end {
                out.raw("end", &point(*end));
            }
            out.entities("snapEntities", snap);
            QUERY_WALL_PREVIEW
        }
    };
    (method, out.finish())
}

/// A JSON object written field by field.
struct Json(String);

impl Json {
    fn new() -> Self {
        Json(String::from("{"))
    }

    fn key(&mut self, key: &str) {
        if self.0.len() > 1 {
            self.0.push(',');
        }
        push_string(&mut self.0, key);
        self.0.push(':');
    }

    fn raw(&mut self, key: &str, value: &str) {
        self.key(key);
        self.0.push_str(value);
    }

    fn text(&mut self, key: &str, value: &str) {
        self.key(key);
        push_string(&mut self.0, value);
    }

    fn number(&mut self, key: &str, value: f32) {
        // The decoder refuses what is not finite.
        self.raw(key, &value.to_string());
    }

    fn boolean(&mut self, key: &str, value: bool) {
        self.raw(key, if value { "true" } else { "false" });
    }

    fn entities(&mut self, key: &str, entities: &[u32]) {
        let list: Vec<String> = entities.iter().map(u32::to_string).collect();
        self.raw(key, &format!("[{}]", list.join(",")));
    }

    fn order_flags(&mut self, flags: u8) {
        self.boolean("queued", flags & QUEUED != 0);
        self.boolean("pushFront", flags & PUSH_FRONT != 0);
    }

    fn targeted(&mut self, kind: &str, flags: u8, entities: &[u32], target: u32) {
        self.text("type", kind);
        self.entities("entities", entities);
        self.raw("target", &target.to_string());
        self.order_flags(flags);
        self.text("formation", NULL_FORMATION);
    }

    fn finish(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

pub fn push_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The entity ids a query reply names (`SetWallPlacementPreview`'s
/// `startSnappedEnt` and `endSnappedEnt`, `GetFoundationSnapData`'s
/// `ent`), as the seat's handles; one the seat does not hold reads as
/// none, `INVALID_ENTITY`.
pub fn map_reply_ids(reply: &str, handle: impl Fn(u32) -> Option<u32>) -> String {
    const KEYS: [&str; 3] = ["\"startSnappedEnt\":", "\"endSnappedEnt\":", "\"ent\":"];
    let mut out = String::with_capacity(reply.len());
    let mut rest = reply;
    while let Some((at, key)) = KEYS
        .iter()
        .filter_map(|key| Some((rest.find(key)?, *key)))
        .min_by_key(|(at, _)| *at)
    {
        let value_start = at + key.len();
        out.push_str(&rest[..value_start]);
        rest = &rest[value_start..];
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if let Ok(id) = rest[..digits].parse::<u32>() {
            let mapped = if id == 0 { 0 } else { handle(id).unwrap_or(0) };
            out.push_str(&mapped.to_string());
            rest = &rest[digits..];
        }
    }
    out.push_str(rest);
    out
}
