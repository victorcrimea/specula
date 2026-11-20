//! Hosting: the server hosts a game for a web player instead of observing
//! someone else's. A headless engine (`scripts/play.sh`) hosts it as an
//! observer, with the player's slot left unassigned and without AI; the
//! feed socket carries the match out as usual and the player's commands
//! back (`SpeculaFeed.h`, the uplink), which the engine posts for that
//! slot as the game's controller.
//!
//! One game per `Play`, and one `Play` per match (`registry.rs`).
//! `/g/<uuid>` is the player's page on the live stream for the visitor who
//! holds the seat, and the observers' page with the delay for anyone else;
//! this split is a convenience, not a guard. The seat is the visitor's `sv`
//! cookie.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::JoinHandle;

use specula_assets::command::ALLOW_CAPTURE;
use specula_assets::command::Command as Order;
use specula_assets::command::PUSH_FRONT;
use specula_assets::command::QUEUED;
use specula_assets::command::Query;
use specula_assets::command::QueryKind;

use rocket::tokio::sync::broadcast;

use crate::feed::Summary;
use crate::observer;
use crate::observer::EngineDirs;
use crate::registry::MatchId;
use crate::summary_envelope;
use crate::view::Hub;
use crate::view::SummaryBody;
use crate::view::spawn_brotli;

const PLAY_SCRIPT: &str = "scripts/play.sh";
const UPLINK_COMMAND: u8 = 1;
const UPLINK_QUERY: u8 = 2;
const UPLINK_PAUSE: u8 = 3;
/// `SpeculaFeed.h`'s query methods.
const QUERY_BUILDING_PREVIEW: u8 = 1;
const QUERY_FOUNDATION_SNAP: u8 = 2;
const QUERY_WALL_PREVIEW: u8 = 3;
/// Replies not yet sent to the pages; a placement asks at most a couple at
/// a time.
const REPLY_BACKLOG: usize = 64;
/// What every order the GUI gives but walk names, as
/// `g_AutoFormation.getNull()` with `formationwalkonly`
/// (`globalscripts/Formation.js`).
const NULL_FORMATION: &str = "special/formations/null";
const MAX_PLAYERS: usize = 8;
const MAX_DIFFICULTY: u8 = 5;
/// `simulation/data/settings/starting_resources.json`, which the setup
/// page offers.
const STARTING_RESOURCES: [u32; 6] = [100, 300, 500, 1000, 3000, 50000];
/// Its default, for a page from before the choice.
const DEFAULT_STARTING_RESOURCES: u32 = 300;
/// `feed::Player::state`.
const PLAYER_ACTIVE: u8 = 0;

/// The random maps the setup page offers: script name and the engine's
/// name for it (`settings.Name` of `maps/random/<name>.json`), which the
/// page translates.
pub const MAPS: &[(&str, &str)] = &[
    ("mainland", "Mainland"),
    ("alpine_lakes", "Alpine Lakes"),
    ("african_plains", "African Plains"),
    ("continent", "Continent"),
    ("corinthian_isthmus", "Corinthian Isthmus"),
    ("gulf_of_bothnia", "Gulf of Bothnia"),
    ("lake", "Lake"),
    ("lorraine_plain", "Lorraine Plain"),
    ("neareastern_badlands", "Neareastern Badlands"),
    ("oasis", "Oasis"),
    ("persian_highlands", "Persian Highlands"),
    ("wild_lake", "Wild Lake"),
];

/// `session-gui` writes it from the game data; the client embeds it too.
const CIVS_TXT: &str = include_str!("../../specula-web/civs.txt");

/// Civ code and the engine's name for it, which the page translates.
pub fn civs() -> impl Iterator<Item = (&'static str, &'static str)> {
    CIVS_TXT
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('\t'))
}

/// What the setup page and the player's page read: whether a game is
/// running and is the visitor's, whether its seat is defeated, whether the
/// visitor's last game can be repeated, and the maps and civs to choose
/// from.
pub fn setup_json(status: &Status) -> String {
    let mut json = Json::new();
    json.boolean("running", status.running);
    json.boolean("mine", status.mine);
    json.boolean("defeated", status.defeated);
    json.boolean("rematch", status.rematch);
    json.raw("seat", &status.seat.unwrap_or(0).to_string());
    json.raw("maps", &pairs(&mut MAPS.iter().copied()));
    json.raw("civs", &pairs(&mut civs()));
    json.finish()
}

fn pairs(items: &mut dyn Iterator<Item = (&str, &str)>) -> String {
    let mut out = String::from("[");
    for (index, (code, name)) in items.enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('[');
        push_string(&mut out, code);
        out.push(',');
        push_string(&mut out, name);
        out.push(']');
    }
    out.push(']');
    out
}

/// What the setup form reads (`GET /new`): whether every slot is taken,
/// whether the visitor has a game running and which, the newest running
/// one, and the maps and civs to choose from. The ids are UUIDs, empty for
/// none.
pub fn form_json(
    running: bool,
    mine: bool,
    own: Option<MatchId>,
    latest: Option<MatchId>,
    stopping: bool,
) -> String {
    let mut json = Json::new();
    json.boolean("running", running);
    json.boolean("mine", mine);
    json.boolean("stopping", stopping);
    json.text("match", &own.map(|id| id.to_string()).unwrap_or_default());
    json.text(
        "latest",
        &latest.map(|id| id.to_string()).unwrap_or_default(),
    );
    json.raw("maps", &pairs(&mut MAPS.iter().copied()));
    json.raw("civs", &pairs(&mut civs()));
    json.finish()
}

/// The visitor's view of the game, for `setup_json`.
pub struct Status {
    pub running: bool,
    pub mine: bool,
    /// The visitor's seat is not active any more: they resigned or were
    /// beaten while the others play on.
    pub defeated: bool,
    /// The visitor started the last game, which a rematch repeats.
    pub rematch: bool,
    /// The visitor's seat while the game is theirs, running or not, which
    /// the summary reads after the engine has exited.
    pub seat: Option<u8>,
}

/// What the setup page chose.
#[derive(Clone)]
pub struct Setup {
    pub map: String,
    /// One per player, in order.
    pub slots: Vec<Slot>,
    /// The game setup's Explored Map and Revealed Map; revealed implies
    /// explored, as `MapExploration.setRevealed` makes it.
    pub explored: bool,
    pub revealed: bool,
    /// The game setup's Starting Resources: each resource's amount.
    pub resources: u32,
}

impl Setup {
    /// From the setup page's form: `map`, `players`, `resources`,
    /// `explored` and `revealed` when checked, and for each player i from 1, `civ<i>`,
    /// `slot<i>` ("you", "petra" or "empty") and `difficulty<i>` for Petra.
    /// Checked when the game starts.
    pub fn from_form(form: &HashMap<String, String>) -> Result<Setup, &'static str> {
        let field = |name: &str| form.get(name).map(String::as_str).ok_or("field missing");
        let players: usize = field("players")?.parse().map_err(|_| "bad player count")?;
        if !(2..=MAX_PLAYERS).contains(&players) {
            return Err("2 to 8 players");
        }
        let mut slots = Vec::with_capacity(players);
        for player in 1..=players {
            let kind = match field(&format!("slot{player}"))? {
                "you" => SlotKind::You,
                "petra" => SlotKind::Petra(
                    field(&format!("difficulty{player}"))?
                        .parse()
                        .map_err(|_| "bad difficulty")?,
                ),
                "empty" => SlotKind::Empty,
                _ => return Err("bad slot"),
            };
            slots.push(Slot {
                civ: field(&format!("civ{player}"))?.to_string(),
                kind,
            });
        }
        let revealed = form.contains_key("revealed");
        let resources = match form.get("resources") {
            Some(resources) => resources.parse().map_err(|_| "bad starting resources")?,
            None => DEFAULT_STARTING_RESOURCES,
        };
        Ok(Setup {
            map: field("map")?.to_string(),
            slots,
            explored: revealed || form.contains_key("explored"),
            revealed,
            resources,
        })
    }
}

#[derive(Clone)]
pub struct Slot {
    /// A civ code, or "random".
    pub civ: String,
    pub kind: SlotKind,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SlotKind {
    You,
    /// Petra at this difficulty, 0 (sandbox) to 5 (very hard).
    Petra(u8),
    Empty,
}

pub struct Play {
    socket: String,
    port: u16,
    dirs: EngineDirs,
    /// Server-wide launch counter, for the log line only.
    generation: u64,
    hub: Arc<Hub>,
    inner: Mutex<Inner>,
    /// The engine's answers to the queries, as the text message
    /// `{"id":..,"result":..}` the pages read.
    replies: broadcast::Sender<String>,
}

#[derive(Default)]
struct Inner {
    game: Option<Game>,
    /// The engine's end of the feed socket while it is connected.
    uplink: Option<UnixStream>,
}

struct Game {
    /// The visitor who started it, who holds the seat.
    owner: String,
    /// Their player number.
    seat: u8,
    running: bool,
    /// What it was started with, for a rematch.
    setup: Setup,
    /// Every statistics capture of the owner's seat, kept after the engine
    /// exits.
    summaries: Vec<Arc<SeatCapture>>,
}

/// One capture of the owner's seat, and its envelope once compressed.
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
        generation: u64,
        dirs: EngineDirs,
    ) -> Self {
        Play {
            socket,
            port,
            dirs,
            generation,
            hub,
            inner: Mutex::new(Inner::default()),
            replies: broadcast::channel(REPLY_BACKLOG).0,
        }
    }

    /// The seat and the engine's arguments for `setup`, so a bad form is
    /// refused before anything is spent on it.
    pub fn check(setup: &Setup) -> Result<(u8, Vec<String>), &'static str> {
        let seat = setup
            .slots
            .iter()
            .position(|slot| slot.kind == SlotKind::You)
            .ok_or("no slot is yours")? as u8
            + 1;
        Ok((seat, engine_args(setup)?))
    }

    /// Starts the game for `owner` and gives the thread that runs its
    /// engine. `previous` is the runner of the game this one replaces in
    /// the slot, joined first: the UDP port is the slot's. `on_exit` runs
    /// once the engine is gone and no lock of this is held.
    pub fn start(
        self: &Arc<Self>,
        owner: &str,
        setup: &Setup,
        previous: Option<JoinHandle<()>>,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> Result<JoinHandle<()>, &'static str> {
        let (seat, args) = Self::check(setup)?;
        self.inner.lock().unwrap().game = Some(Game {
            owner: owner.to_string(),
            seat,
            running: true,
            setup: setup.clone(),
            summaries: Vec::new(),
        });
        println!("game {} for {owner}: {}", self.generation, args.join(" "));
        // The engine connects the feed after this, so its first turn
        // already has the seat's stream.
        self.hub.set_seats(vec![u32::from(seat)]);
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

    /// The player number `visitor` commands, if they hold the seat.
    pub fn seat(&self, visitor: Option<&str>) -> Option<u8> {
        let inner = self.inner.lock().unwrap();
        let game = inner.game.as_ref()?;
        (Some(game.owner.as_str()) == visitor).then_some(game.seat)
    }

    pub fn status(&self, visitor: Option<&str>) -> Status {
        let (running, owned, seat) = {
            let inner = self.inner.lock().unwrap();
            match &inner.game {
                Some(game) => (
                    game.running,
                    Some(game.owner.as_str()) == visitor,
                    game.seat,
                ),
                None => (false, false, 0),
            }
        };
        Status {
            running,
            mine: running && owned,
            defeated: running && owned && self.seat_defeated(seat),
            rematch: owned,
            seat: owned.then_some(seat).filter(|&seat| seat != 0),
        }
    }

    /// Every capture is kept, but only the owner's seat: the other copies
    /// are never served through the seat's routes.
    pub fn set_summary(&self, summary: Summary) {
        let (capture, raw) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(game) = inner.game.as_mut() else {
                return;
            };
            let Some(view) = summary
                .seats
                .get(usize::from(game.seat))
                .and_then(|view| view.as_ref())
            else {
                return;
            };
            let Some(raw) = summary_envelope(summary.turn, summary.is_final, view) else {
                return;
            };
            let raw: Arc<[u8]> = raw.into();
            let capture = Arc::new(SeatCapture {
                turn: summary.turn,
                inner: Mutex::new(SeatBody {
                    raw: Some(Arc::clone(&raw)),
                    brotli: None,
                }),
            });
            // A final capture on the turn of a periodic one replaces it.
            if game
                .summaries
                .last()
                .is_some_and(|last| last.turn == summary.turn)
            {
                game.summaries.pop();
            }
            game.summaries.push(Arc::clone(&capture));
            (capture, raw)
        };
        spawn_brotli(raw, move |brotli| {
            let mut inner = capture.inner.lock().unwrap();
            inner.brotli = Some(brotli);
            inner.raw = None;
        });
    }

    /// The owner's seat view at or before `turn`, or the last one. Never
    /// the full copy: it holds the opponents' statistics.
    pub fn summary(&self, visitor: Option<&str>, turn: Option<u32>) -> Option<SummaryBody> {
        let inner = self.inner.lock().unwrap();
        let game = inner.game.as_ref()?;
        if Some(game.owner.as_str()) != visitor {
            return None;
        }
        let capture = game
            .summaries
            .iter()
            .rev()
            .find(|capture| turn.is_none_or(|turn| capture.turn <= turn))?;
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

    /// The running game `owner` holds the seat of, as its seat number.
    fn running_seat(&self, owner: &str) -> Result<u8, &'static str> {
        let inner = self.inner.lock().unwrap();
        match &inner.game {
            Some(game) if game.running && game.owner == owner => Ok(game.seat),
            _ => Err("no game of yours is running"),
        }
    }

    /// Resigns `owner`'s seat, as the session's Resign does
    /// (`ResignConfirmation.js`); the others play on.
    pub fn resign(&self, owner: &str) -> Result<(), &'static str> {
        let seat = self.running_seat(owner)?;
        self.send_json(seat, r#"{"type":"resign"}"#);
        Ok(())
    }

    /// Pauses or resumes the game for `owner`'s seat, as the session's
    /// PauseControl does; the engine holds it paused while any seat has.
    pub fn set_paused(&self, owner: &str, paused: bool) -> Result<(), &'static str> {
        let seat = self.running_seat(owner)?;
        let record = [UPLINK_PAUSE, 2, 0, 0, 0, seat, u8::from(paused)];
        self.write_uplink(&record);
        Ok(())
    }

    /// Ends `owner`'s game for everyone: the engine is stopped, and the
    /// runner thread and the feed's end take it from there.
    pub fn end(&self, owner: &str) -> Result<(), &'static str> {
        let mut inner = self.inner.lock().unwrap();
        match &mut inner.game {
            Some(game) if game.running && game.owner == owner => {
                observer::kill_recorded(&self.socket);
                // At once rather than once the runner has seen the engine
                // exit, so the page reloaded after this shows the end.
                game.running = false;
                Ok(())
            }
            _ => Err("no game of yours is running"),
        }
    }

    /// Stops the engine of a game nobody plays any more, as `end` does but
    /// for no owner: a stopping server must not wait on a won game whose
    /// engine stays in its session. Returns whether it stopped one.
    pub fn end_decided(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match &mut inner.game {
            Some(game) if game.running && self.seat_defeated(game.seat) => {
                println!("game {} is decided, stopping its engine", self.generation);
                observer::kill_recorded(&self.socket);
                game.running = false;
                true
            }
            _ => false,
        }
    }

    /// The setup of the game `owner` started, which a rematch repeats.
    pub fn setup_of(&self, owner: &str) -> Option<Setup> {
        let inner = self.inner.lock().unwrap();
        let game = inner.game.as_ref()?;
        (game.owner == owner).then(|| game.setup.clone())
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
    pub fn query(&self, player: u8, query: &Query) {
        let (method, json) = query_json(&query.kind);
        let mut record = Vec::with_capacity(json.len() + 11);
        record.push(UPLINK_QUERY);
        record.extend_from_slice(&(json.len() as u32 + 6).to_le_bytes());
        record.extend_from_slice(&query.id.to_le_bytes());
        record.push(player);
        record.push(method);
        record.extend_from_slice(json.as_bytes());
        self.write_uplink(&record);
    }

    pub fn reply(&self, id: u32, json: &[u8]) {
        let Ok(json) = std::str::from_utf8(json) else {
            return;
        };
        // No receiver is no page to tell.
        let _ = self
            .replies
            .send(format!(r#"{{"id":{id},"result":{json}}}"#));
    }

    pub fn replies(&self) -> broadcast::Receiver<String> {
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

/// The `-autostart-*` options for `setup`, checked.
fn engine_args(setup: &Setup) -> Result<Vec<String>, &'static str> {
    if !MAPS.iter().any(|(name, _)| *name == setup.map) {
        return Err("unknown map");
    }
    if !(2..=MAX_PLAYERS).contains(&setup.slots.len()) {
        return Err("2 to 8 players");
    }
    if setup
        .slots
        .iter()
        .filter(|slot| slot.kind == SlotKind::You)
        .count()
        != 1
    {
        return Err("exactly one slot must be yours");
    }
    let mut args = vec![
        format!("-autostart=random/{}", setup.map),
        format!("-autostart-players={}", setup.slots.len()),
        "-autostart-seed=-1".to_string(),
        "-autostart-aiseed=-1".to_string(),
    ];
    // Read by autostart_specula_play.js.
    if setup.explored {
        args.push("-autostart-specula-explored".to_string());
    }
    if setup.revealed {
        args.push("-autostart-specula-revealed".to_string());
    }
    if !STARTING_RESOURCES.contains(&setup.resources) {
        return Err("unknown starting resources");
    }
    args.push(format!("-autostart-specula-resources={}", setup.resources));
    for (index, slot) in setup.slots.iter().enumerate() {
        let player = index + 1;
        if slot.civ != "random" && !civs().any(|(code, _)| code == slot.civ) {
            return Err("unknown civ");
        }
        args.push(format!("-autostart-civ={player}:{}", slot.civ));
        if let SlotKind::Petra(difficulty) = slot.kind {
            if difficulty > MAX_DIFFICULTY {
                return Err("unknown difficulty");
            }
            args.push(format!("-autostart-ai={player}:petra"));
            args.push(format!("-autostart-aidiff={player}:{difficulty}"));
        }
    }
    Ok(args)
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
