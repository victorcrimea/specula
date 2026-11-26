//! A match's setup room (D13, D23, D35-D39): the settings the host edits and
//! the visitors watch, over one websocket per visitor. The room holds the
//! settings and the phase; who sits where is the match's `Seats`. Lock order:
//! registry, then the room, then `Play`, `Seats`, `Hub`; nothing in those
//! calls a room. `begin_launch` resolves the settings under the room's lock
//! and the registry releases it before it starts the engine.
//!
//! Every accepted change bumps a revision; each socket then rebuilds its own
//! state (its `you`, the launch blocker) and sends all of it, never a diff.

use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use rocket::tokio::sync::watch;
use serde_json::Value;
use serde_json::json;

use crate::catalog::AI_BEHAVIORS;
use crate::catalog::AI_DIFFICULTIES;
use crate::catalog::Catalog;
use crate::catalog::MapType;
use crate::nickname::Nickname;
use crate::seats::Ready;
use crate::seats::Seats;
use crate::settings::Choice;
use crate::settings::LandscapeChoice;
use crate::settings::Launch;
use crate::settings::MAPS_UP_TO_8_PLAYERS;
use crate::settings::MAX_PLAYERS_SMALL_MAP;
use crate::settings::MapInfo;
use crate::settings::Settings;
use crate::settings::gamesetup::CEASEFIRE_MAX;
use crate::settings::gamesetup::Change;
use crate::settings::gamesetup::MAP_FILTERS;
use crate::settings::gamesetup::MAX_TEAMS;
use crate::settings::gamesetup::RELIC_DURATION_MAX;
use crate::settings::gamesetup::SEA_LEVEL_RISE_MAX;
use crate::settings::gamesetup::WONDER_DURATION_MAX;
use crate::settings::gamesetup::available_filters;
use crate::settings::gamesetup::map_choices;
use crate::settings::gamesetup::map_filter_names;

/// D15: a setup nobody starts closes after this.
pub const SETUP_LIMIT: Duration = Duration::from_secs(600);
/// D16: a setup with no visitor left closes after this.
pub const EMPTY_LIMIT: Duration = Duration::from_secs(15);
/// U2: a member whose last setup socket closed counts as gone after this,
/// so a refresh keeps the seat and the host powers.
pub const LEAVE_GRACE: Duration = Duration::from_secs(5);
/// Closed setups remembered, so their page can say why.
pub const CLOSED_KEEP: usize = 256;
/// D62: a rematch holds each old player's slot for this long.
pub const RESERVE_FOR: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Timeout,
    Empty,
    Stopping,
    Replaced,
}

impl CloseReason {
    pub fn name(self) -> &'static str {
        match self {
            CloseReason::Timeout => "timeout",
            CloseReason::Empty => "empty",
            CloseReason::Stopping => "stopping",
            CloseReason::Replaced => "replaced",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Open,
    Launching,
    Launched,
    Closed(CloseReason),
}

pub struct SetupRoom {
    inner: Mutex<RoomInner>,
    /// Bumped on every change; each socket rebuilds its own state.
    revision: watch::Sender<u64>,
    /// The server lets a game start with AI players alone.
    ai_only: bool,
}

struct RoomInner {
    settings: Settings,
    phase: Phase,
    opened: Instant,
    /// Every open setup socket, member or not (D16 counts visitors).
    sockets: u32,
    empty_since: Option<Instant>,
    /// The old players' slots a rematch holds (M35).
    reserved: Vec<Reservation>,
}

struct Reservation {
    cookie: String,
    name: Nickname,
    player: u8,
    until: Instant,
}

pub enum Request {
    Join(Nickname),
    Change(Change),
    Take(usize),
    StepDown,
    Move { member: usize, slot: usize },
    Unseat(usize),
    Ready(Ready),
    Kick { member: usize, ban: bool },
    Launch,
}

impl SetupRoom {
    pub fn new(settings: Settings, reserved: Vec<(String, Nickname, u8)>, ai_only: bool) -> Self {
        let until = Instant::now() + RESERVE_FOR;
        SetupRoom {
            inner: Mutex::new(RoomInner {
                settings,
                phase: Phase::Open,
                opened: Instant::now(),
                sockets: 0,
                empty_since: Some(Instant::now()),
                reserved: reserved
                    .into_iter()
                    .map(|(cookie, name, player)| Reservation {
                        cookie,
                        name,
                        player,
                        until,
                    })
                    .collect(),
            }),
            revision: watch::channel(0).0,
            ai_only,
        }
    }

    /// The map and the seat count of an open setup, for the lobby; None
    /// once it launched or closed.
    pub fn lobby_info(&self) -> Option<(MapInfo, usize)> {
        let inner = self.inner.lock().unwrap();
        matches!(inner.phase, Phase::Open | Phase::Launching)
            .then(|| (inner.settings.map_info(), inner.settings.player_count.max))
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    /// `send_modify` works from any thread and needs no receiver.
    pub fn bump(&self) {
        self.revision.send_modify(|revision| *revision += 1);
    }

    pub fn open_socket(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.sockets += 1;
        inner.empty_since = None;
    }

    pub fn close_socket(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.sockets = inner.sockets.saturating_sub(1);
        if inner.sockets == 0 {
            inner.empty_since = Some(Instant::now());
        }
    }

    pub fn close(&self, reason: CloseReason) {
        self.inner.lock().unwrap().phase = Phase::Closed(reason);
        self.bump();
    }

    /// Why an open setup is due to close, if it is (D15, D16).
    pub fn due(&self) -> Option<CloseReason> {
        let inner = self.inner.lock().unwrap();
        if inner.phase != Phase::Open {
            return None;
        }
        if inner.opened.elapsed() >= SETUP_LIMIT {
            return Some(CloseReason::Timeout);
        }
        if inner.sockets == 0
            && inner
                .empty_since
                .is_some_and(|since| since.elapsed() >= EMPTY_LIMIT)
        {
            return Some(CloseReason::Empty);
        }
        None
    }

    /// After members left: the seats they freed change what the settings
    /// allow.
    pub fn after_leave(&self, catalog: &Catalog, seats: &Seats, was_seated: bool) {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.phase != Phase::Open {
                return;
            }
            if was_seated {
                inner.settings.on_seated(catalog, &seats.players());
                seats.reset_ready();
            }
            seats.force_host_stay();
            prune(&mut inner, seats);
        }
        self.bump();
    }

    /// Applies what `cookie` asked, if it is theirs to ask (D31, U5);
    /// anything else is dropped, with a reason for debugging.
    pub fn handle(
        &self,
        catalog: &Catalog,
        seats: &Seats,
        cookie: &str,
        request: Request,
    ) -> Result<(), &'static str> {
        let mut inner = self.inner.lock().unwrap();
        if inner.phase != Phase::Open {
            return Err("the setup is closed");
        }
        if seats.is_banned(cookie) {
            return Err("you are banned from this match");
        }
        let RoomInner {
            settings, reserved, ..
        } = &mut *inner;
        let member = seats.index_of(cookie);
        if member.is_none() && !matches!(request, Request::Join(_)) {
            return Err("not joined");
        }
        // A kicked member keeps their entry: the setup socket closes once
        // it sees the kick, and until then nothing may be asked.
        if member.is_some() && !matches!(request, Request::Join(_)) && !seats.is_present(cookie) {
            return Err("join first");
        }
        let host = seats.is_host(cookie);
        let own = seats.player_of(cookie);
        // Whether the change resets everyone's Ready (U3).
        let mut reset = true;
        match request {
            Request::Join(nickname) => {
                seats.join(cookie, nickname);
                let now = Instant::now();
                let reserved_slot = reserved.iter().position(|hold| {
                    hold.cookie == cookie
                        && hold.until > now
                        && slot_open(settings, seats, usize::from(hold.player) - 1)
                });
                if let Some(at) = reserved_slot {
                    let player = reserved.remove(at).player;
                    seats.set_player(cookie, Some(player))?;
                    settings.on_seated(catalog, &seats.players());
                } else {
                    let count = settings.player_count.players;
                    let taken = seats.take_back(cookie, |player| {
                        let slot = usize::from(player) - 1;
                        slot < count
                            && settings.player_ai.get(slot).is_some_and(Option::is_none)
                            && settings.player_removed.get(slot) != Some(&true)
                    });
                    if taken.is_some() {
                        settings.on_seated(catalog, &seats.players());
                    } else {
                        reset = false;
                    }
                }
            }
            Request::Change(change) => {
                if host {
                    // As vanilla's `PlayerAssignmentItem`: an AI or an empty
                    // or removed slot unassigns the human who sat there.
                    let slot = match &change {
                        Change::Ai { index, .. }
                        | Change::Unassign { index }
                        | Change::Remove { index } => Some(*index),
                        _ => None,
                    };
                    let unassign = matches!(&change, Change::Unassign { .. });
                    let victim = slot
                        .and_then(|index| u8::try_from(index + 1).ok())
                        .and_then(|player| seats.cookie_of_player(player));
                    let mut seated = seats.players();
                    if victim.is_some()
                        && let Some(index) = slot
                    {
                        seated.retain(|player| usize::from(*player) != index + 1);
                    }
                    // On a copy, so a refused change unseats nobody.
                    let mut trial = settings.clone();
                    trial.apply(catalog, change, &seated)?;
                    if let Some(index) = slot.filter(|_| unassign) {
                        reserved.retain(|hold| usize::from(hold.player) != index + 1);
                    }
                    if let Some(victim) = victim {
                        let _ = seats.set_player(&victim, None);
                    }
                    seats.unseat_beyond(trial.player_count.players);
                    trial.on_seated(catalog, &seats.players());
                    *settings = trial;
                } else if let (Some(player), Change::Civ { index, .. }) = (own, &change)
                    && *index == usize::from(player) - 1
                {
                    settings.apply(catalog, change, &seats.players())?;
                } else {
                    return Err("only the host changes that");
                }
            }
            Request::Take(slot) => {
                let player = open_slot(settings, seats, slot, reserved, cookie)?;
                if let Some(held) = own {
                    settings.swap_slots(catalog, usize::from(held) - 1, slot);
                }
                seats.set_player(cookie, Some(player))?;
                settings.on_seated(catalog, &seats.players());
            }
            Request::StepDown => {
                if own.is_none() {
                    return Err("not seated");
                }
                seats.set_player(cookie, None)?;
                settings.on_seated(catalog, &seats.players());
            }
            Request::Move { member, slot } => {
                if !host {
                    return Err("only the host moves players");
                }
                let mover = seats.cookie_at(member).ok_or("no such member")?;
                if slot >= settings.player_count.players {
                    return Err("no such slot");
                }
                let target = u8::try_from(slot + 1).map_err(|_| "no such slot")?;
                let from = seats.player_of(&mover);
                if from == Some(target) {
                    return Err("already there");
                }
                match (seats.cookie_of_player(target), from) {
                    (Some(holder), Some(held)) => {
                        seats.swap_players(&mover, &holder)?;
                        settings.swap_slots(catalog, usize::from(held) - 1, slot);
                    }
                    (Some(holder), None) => {
                        seats.set_player(&holder, None)?;
                        seats.set_player(&mover, Some(target))?;
                    }
                    (None, from) => {
                        if let Some(held) = from {
                            settings.swap_slots(catalog, usize::from(held) - 1, slot);
                        }
                        seats.set_player(&mover, Some(target))?;
                    }
                }
                settings.on_seated(catalog, &seats.players());
            }
            Request::Unseat(member) => {
                if !host {
                    return Err("only the host unseats players");
                }
                let target = seats.cookie_at(member).ok_or("no such member")?;
                seats.set_player(&target, None)?;
                settings.on_seated(catalog, &seats.players());
            }
            Request::Ready(state) => {
                seats.set_ready(cookie, state)?;
                reset = false;
            }
            Request::Kick { member, ban } => {
                if !host {
                    return Err("only the host kicks");
                }
                let target = seats.cookie_at(member).ok_or("no such member")?;
                seats.kick(&target, ban)?;
                reserved.retain(|hold| hold.cookie != target);
                settings.on_seated(catalog, &seats.players());
            }
            // The registry starts the game.
            Request::Launch => return Err("not a room's request"),
        }
        if reset {
            seats.reset_ready();
        }
        seats.force_host_stay();
        prune(&mut inner, seats);
        drop(inner);
        self.bump();
        Ok(())
    }

    /// Drops the expired reservations, so a late follower's slot is free
    /// again (M35): answers whether any went.
    pub fn expire_reservations(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.reserved.len();
        let now = Instant::now();
        inner.reserved.retain(|hold| hold.until > now);
        let changed = inner.reserved.len() != before;
        drop(inner);
        if changed {
            self.bump();
        }
        changed
    }

    /// Checks a launch and resolves the settings (M16); the registry
    /// starts the engine after this releases the room, then calls
    /// `finish_launch`.
    pub fn begin_launch(
        &self,
        catalog: &Catalog,
        seats: &Seats,
        cookie: &str,
        match_id: &str,
    ) -> Result<Launch, &'static str> {
        let mut inner = self.inner.lock().unwrap();
        if inner.phase != Phase::Open {
            return Err("the game is starting");
        }
        if !seats.is_host(cookie) {
            return Err("only the host can start the game");
        }
        if let Some(reason) = blocker(seats, &inner.reserved, &inner.settings, self.ai_only) {
            return Err(reason);
        }
        let seated = seats.seated_names();
        let assignments: Vec<crate::settings::Assignment> = seated
            .iter()
            .map(|(player, name)| crate::settings::Assignment {
                player: *player,
                name: name.as_str(),
            })
            .collect();
        let launch = inner
            .settings
            .launch(catalog, &assignments, match_id)
            .map_err(|error| {
                eprintln!("cannot launch the setup: {error}");
                "the settings cannot start a game"
            })?;
        inner.phase = Phase::Launching;
        Ok(launch)
    }

    pub fn finish_launch(&self, ok: bool) {
        {
            let mut inner = self.inner.lock().unwrap();
            // A close meanwhile (a stop) stays.
            if inner.phase == Phase::Launching {
                inner.phase = if ok { Phase::Launched } else { Phase::Open };
            }
        }
        self.bump();
    }

    /// What one socket is sent: the whole state, as its member sees it,
    /// and the phase it was taken in, which a socket closes on when it is
    /// over (read apart from the text, it could have moved on meanwhile).
    pub fn state_json(
        &self,
        seats: &Seats,
        catalog: &Catalog,
        viewer: Option<&str>,
    ) -> (String, Phase) {
        let inner = self.inner.lock().unwrap();
        let phase = inner.phase;
        match phase {
            Phase::Launched => return (json!({ "type": "launched" }).to_string(), phase),
            Phase::Closed(reason) => return (closed_json(reason), phase),
            Phase::Open | Phase::Launching => {}
        }
        let settings = &inner.settings;
        let (members, host) = seats.snapshot();
        let members: Vec<Value> = members
            .iter()
            .enumerate()
            .map(|(index, member)| {
                json!({
                    "index": index,
                    "name": member.name.as_str(),
                    "player": member.player,
                    "present": member.present,
                    "connected": member.connected,
                    "ready": member.ready.as_u8(),
                })
            })
            .collect();
        let kind = settings.map.kind;
        let filters: Vec<&str> = kind
            .map(|kind| {
                available_filters(catalog, kind)
                    .iter()
                    .map(|filter| filter.name)
                    .collect()
            })
            .unwrap_or_default();
        let maps = kind
            .map(|kind| {
                map_choices(
                    catalog,
                    kind,
                    &settings.gui.map_filter,
                    settings.player_count.players,
                )
            })
            .unwrap_or_default();
        let raw = |value: &Option<Value>| value.clone().unwrap_or(Value::Null);
        let sea_level = settings.sea_level_rise.value.map(|_| {
            json!({
                "min": settings.sea_level_rise.min,
                "max": settings.sea_level_rise.max,
            })
        });
        // Apart from the rest: one `json!` this deep passes the macro's
        // recursion limit.
        let offer = json!({
            "map_filters": filters,
            "maps": maps,
            "map_size": settings.map_size.available,
            "biomes": settings.biome.available,
            "daytime": raw(&settings.daytime.data),
            "landscape": raw(&settings.landscape.data),
            "placements": settings.player_placement.available,
            "water_levels": raw(&settings.water_level.options),
            "trigger_difficulties": settings.trigger_difficulty.available,
            "sea_level_rise": sea_level,
            "relic": settings.relic.available,
            "wonder": settings.wonder.available,
            "last_man_standing": settings.last_man_standing.available,
            "regicide_garrison": settings.regicide_garrison.available,
            "locked_teams": settings.locked_teams.available,
            "scenario": settings.is_scenario(),
            "nomad": kind == Some(MapType::Random),
            "fast_forward": settings.gui.allow_fast_forward != Some(false),
            "colors": settings.player_color.available,
            "civ_locked": settings.player_civ.locked,
            "team_locked": settings.player_team.locked,
            "color_locked": settings.player_color.locked,
            "victory_disabled": settings.victory_conditions.disabled,
            "player_count_max": settings.player_count.max,
            "hidden_maps": hidden_maps(catalog, settings),
            "pop_per_player": settings.population.per_player.is_some(),
            "resources_per_player": settings.starting_resources.per_player.is_some(),
            "removed": settings.player_removed,
            "ai": settings.player_ai.iter().map(|ai| ai.as_ref().map(|ai| json!({
                "bot": ai.bot,
                "difficulty": ai.difficulty,
                "behavior": ai.behavior,
            }))).collect::<Vec<_>>(),
        });
        let state = json!({
            "type": "state",
            "revision": *self.revision.borrow(),
            "you": viewer.and_then(|cookie| seats.index_of(cookie)),
            "host": host,
            "closes_in_ms": u64::try_from(SETUP_LIMIT.saturating_sub(inner.opened.elapsed()).as_millis()).unwrap_or(0),
            "members": members,
            "player_count": settings.player_count.players,
            "attributes": settings.to_init_attributes(),
            "gui": { "map_filter": settings.gui.map_filter },
            "offer": offer,
            "launch_blocker": blocker(seats, &inner.reserved, &inner.settings, self.ai_only),
            "reserved": inner.reserved.iter().map(|hold| json!({
                "slot": hold.player - 1,
                "name": hold.name.as_str(),
            })).collect::<Vec<_>>(),
        });
        (state.to_string(), phase)
    }
}

/// Why a launch cannot start, from the seats and the rematch's reservations
/// (U3, M35).
fn blocker(
    seats: &Seats,
    reserved: &[Reservation],
    settings: &Settings,
    ai_only: bool,
) -> Option<&'static str> {
    if !reserved.is_empty() {
        return Some("waiting for the players of the last game");
    }
    if seats.players().is_empty() {
        // With the server's opt-in, a game of AIs alone is watched, not played.
        let ai_slot = (0..settings.player_count.players).any(|slot| {
            settings.player_removed.get(slot) != Some(&true)
                && settings.player_ai.get(slot).is_some_and(Option::is_some)
        });
        if !(ai_only && ai_slot) {
            return Some("no player is seated");
        }
    }
    if !seats.everyone_ready() {
        return Some("not every player is ready");
    }
    None
}

/// Whether a rematch reservation's slot is still open for its player.
fn slot_open(settings: &Settings, seats: &Seats, slot: usize) -> bool {
    slot < settings.player_count.players
        && settings.player_removed.get(slot) != Some(&true)
        && settings.player_ai.get(slot).is_some_and(Option::is_none)
        && u8::try_from(slot + 1).is_ok_and(|player| seats.cookie_of_player(player).is_none())
}

/// The slot's player number if a member may sit there now.
fn open_slot(
    settings: &Settings,
    seats: &Seats,
    slot: usize,
    reserved: &[Reservation],
    cookie: &str,
) -> Result<u8, &'static str> {
    if slot >= settings.player_count.players {
        return Err("no such slot");
    }
    if settings.player_removed.get(slot) == Some(&true)
        || settings.player_ai.get(slot).is_some_and(Option::is_some)
    {
        return Err("the slot is not open");
    }
    let player = u8::try_from(slot + 1).map_err(|_| "no such slot")?;
    let now = Instant::now();
    if reserved
        .iter()
        .any(|hold| hold.cookie != cookie && hold.player == player && hold.until > now)
    {
        return Err("that slot is reserved");
    }
    if seats.cookie_of_player(player).is_some() {
        return Err("that slot is taken");
    }
    Ok(player)
}

/// Drops the reservations that no longer hold a slot (M35), so a launch
/// waits only for a follower who can still come.
fn prune(inner: &mut RoomInner, seats: &Seats) -> bool {
    let before = inner.reserved.len();
    let now = Instant::now();
    let count = inner.settings.player_count.players;
    let ai = inner.settings.player_ai.clone();
    let removed = inner.settings.player_removed.clone();
    inner.reserved.retain(|hold| {
        let slot = usize::from(hold.player) - 1;
        hold.until > now
            && slot < count
            && removed.get(slot) != Some(&true)
            && ai.get(slot).is_some_and(Option::is_none)
            && seats.cookie_of_player(hold.player).is_none()
    });
    inner.reserved.len() != before
}

pub fn closed_json(reason: CloseReason) -> String {
    json!({ "type": "closed", "reason": reason.name() }).to_string()
}

/// What a setup socket is sent when its member was kicked, or banned with
/// the kick (D54).
pub fn kicked_json(ban: bool) -> String {
    json!({ "type": "kicked", "ban": ban }).to_string()
}

/// A socket's text, if it is a request.
pub fn parse(text: &str) -> Option<Request> {
    let value: Value = serde_json::from_str(text).ok()?;
    let index = |key: &str| usize::try_from(value[key].as_u64()?).ok();
    match value["type"].as_str()? {
        "join" => Some(Request::Join(Nickname::parse(value["nickname"].as_str()?)?)),
        "change" => parse_change(&value).map(Request::Change),
        "take" => Some(Request::Take(index("slot")?)),
        "step_down" => Some(Request::StepDown),
        "move" => Some(Request::Move {
            member: index("member")?,
            slot: index("slot")?,
        }),
        "unseat" => Some(Request::Unseat(index("member")?)),
        "ready" => Some(Request::Ready(Ready::from_u8(
            u8::try_from(value["state"].as_u64()?).ok()?,
        )?)),
        "kick" => Some(Request::Kick {
            member: index("member")?,
            ban: value["ban"].as_bool().unwrap_or(false),
        }),
        "launch" => Some(Request::Launch),
        _ => None,
    }
}

fn parse_change(request: &Value) -> Option<Change> {
    let name = request["name"].as_str()?;
    let value = &request["value"];
    let text = || value.as_str().map(str::to_string);
    let flag = || value.as_bool();
    let number = || value.as_f64();
    let whole = || u32::try_from(value.as_u64()?).ok();
    let index = || usize::try_from(request["index"].as_u64()?).ok();
    Some(match name {
        "map_type" => Change::MapType(MapType::from_name(value.as_str()?)?),
        "map_filter" => Change::MapFilter(text()?),
        "map" => Change::Map(Choice::from_name(value.as_str()?)),
        "browse_map" => Change::BrowseMap {
            kind: MapType::from_name(request["map_type"].as_str()?)?,
            filter: request["map_filter"].as_str()?.to_string(),
            map: Choice::from_name(value.as_str()?),
        },
        "map_size" => Change::MapSize(whole()?),
        "player_count" => Change::PlayerCount(usize::try_from(value.as_u64()?).ok()?),
        "biome" => Change::Biome(Choice::from_name(value.as_str()?)),
        "daytime" => Change::Daytime(Choice::from_name(value.as_str()?)),
        "placement" => Change::Placement(Choice::from_name(value.as_str()?)),
        "landscape" => {
            let id = value.as_str()?;
            Change::Landscape(if request["group"].as_bool() == Some(true) {
                LandscapeChoice::RandomGroup(id.to_string())
            } else if id == "random" {
                LandscapeChoice::Random
            } else {
                LandscapeChoice::Fixed(id.to_string())
            })
        }
        "water_level" => Change::WaterLevel(text()?),
        "trigger_difficulty" => Change::TriggerDifficulty(u8::try_from(value.as_u64()?).ok()?),
        "sea_level_rise" => Change::SeaLevelRise(number()?),
        "ceasefire" => Change::Ceasefire(number()?),
        "relic_count" => Change::RelicCount(number()?),
        "relic_duration" => Change::RelicDuration(number()?),
        "wonder_duration" => Change::WonderDuration(number()?),
        "game_speed" => Change::GameSpeed(number()?),
        "starting_resources" => Change::StartingResources(whole()?),
        "population_cap" => Change::PopulationCap(whole()?),
        "population_cap_type" => Change::PopulationCapType(text()?),
        "victory_condition" => Change::VictoryCondition {
            name: text()?,
            enabled: request["enabled"].as_bool()?,
        },
        "explored" => Change::Explored(flag()?),
        "revealed" => Change::Revealed(flag()?),
        "allied" => Change::Allied(flag()?),
        "disable_spies" => Change::DisableSpies(flag()?),
        "disable_treasures" => Change::DisableTreasures(flag()?),
        "nomad" => Change::Nomad(flag()?),
        "last_man_standing" => Change::LastManStanding(flag()?),
        "regicide_garrison" => Change::RegicideGarrison(flag()?),
        "locked_teams" => Change::LockedTeams(flag()?),
        "cheats" => Change::Cheats(flag()?),
        "civ" => Change::Civ {
            index: index()?,
            civ: Choice::from_name(value.as_str()?),
        },
        "team" => Change::Team {
            index: index()?,
            team: i32::try_from(value.as_i64()?).ok()?,
        },
        "color" => {
            let channels = value.as_array()?;
            let channel = |at: usize| u8::try_from(channels.get(at)?.as_u64()?).ok();
            Change::Color {
                index: index()?,
                color: [channel(0)?, channel(1)?, channel(2)?],
            }
        }
        "ai" => Change::Ai {
            index: index()?,
            bot: text()?,
        },
        "ai_bot" => Change::AiBot {
            index: index()?,
            bot: if value.is_null() { None } else { Some(text()?) },
        },
        "ai_difficulty" => Change::AiDifficulty {
            index: index()?,
            difficulty: u8::try_from(value.as_u64()?).ok()?,
        },
        "ai_behavior" => Change::AiBehavior {
            index: index()?,
            behavior: text()?,
        },
        "unassign" => Change::Unassign { index: index()? },
        "remove" => Change::Remove { index: index()? },
        "reset_civs" => Change::ResetCivs,
        "reset_teams" => Change::ResetTeams,
        _ => return None,
    })
}

/// The catalog's lists the page offers, sent once when a socket opens.
/// Civ names are not here: the page embeds `civs.txt`.
pub fn catalog_json(catalog: &Catalog) -> String {
    json!({
        "type": "catalog",
        "map_types": MapType::ALL.iter().map(|kind| kind.name()).collect::<Vec<_>>(),
        "map_filters": MAP_FILTERS
            .iter()
            .map(|filter| json!({ "name": filter.name, "title": filter.title }))
            .collect::<Vec<_>>(),
        "maps": catalog.maps.iter().map(|map| json!({
            "path": map.path,
            "kind": map.kind.name(),
            "name": map.name,
            "description": map.description,
            "preview": map.preview,
            "filters": map_filter_names(map),
        })).collect::<Vec<_>>(),
        "civs": catalog.civs.iter()
            .map(|civ| json!({ "code": civ.code, "selectable": civ.selectable }))
            .collect::<Vec<_>>(),
        "ais": catalog.ais.iter().map(|ai| json!({
            "id": ai.id,
            "name": ai.name,
            "description": ai.description,
            "hidden": ai.hidden,
        })).collect::<Vec<_>>(),
        "ai_difficulties": AI_DIFFICULTIES,
        "ai_behaviors": AI_BEHAVIORS,
        "map_sizes": catalog.map_sizes.iter().map(|size| json!({
            "tiles": size.tiles,
            "name": size.name,
            "tooltip": size.tooltip,
        })).collect::<Vec<_>>(),
        "game_speeds": catalog.game_speeds.iter().map(|speed| json!({
            "speed": speed.speed,
            "title": speed.title,
            "fast_forward": speed.fast_forward,
        })).collect::<Vec<_>>(),
        "starting_resources": catalog.starting_resources.iter()
            .map(|entry| json!({ "resources": entry.resources, "title": entry.title }))
            .collect::<Vec<_>>(),
        "population_capacities": catalog.population_capacities.iter().map(|entry| json!({
            "name": entry.name,
            "title": entry.title,
            "cap_title": entry.cap_title,
            "tooltip": entry.tooltip,
            "cap_tooltip": entry.cap_tooltip,
            "appendage": entry.appendage,
            "options": entry.options,
        })).collect::<Vec<_>>(),
        "trigger_difficulties": catalog.trigger_difficulties.iter().map(|entry| json!({
            "difficulty": entry.difficulty,
            "name": entry.name,
            "title": entry.title,
            "tooltip": entry.tooltip,
        })).collect::<Vec<_>>(),
        "victory_conditions": catalog.victory_conditions.iter().map(|entry| json!({
            "name": entry.name,
            "title": entry.title,
            "description": entry.description,
            "gui_order": entry.gui_order,
        })).collect::<Vec<_>>(),
        "player_placements": catalog.player_placements.iter().map(|entry| json!({
            "id": entry.id,
            "name": entry.name,
            "description": entry.description,
        })).collect::<Vec<_>>(),
        "biomes": catalog.biomes.iter().map(|entry| json!({
            "id": entry.id,
            "title": entry.title,
            "description": entry.description,
        })).collect::<Vec<_>>(),
        "max_teams": MAX_TEAMS,
        "ceasefire_max": CEASEFIRE_MAX,
        "relic_duration_max": RELIC_DURATION_MAX,
        "wonder_duration_max": WONDER_DURATION_MAX,
        "sea_level_rise_max": SEA_LEVEL_RISE_MAX,
    })
    .to_string()
}

/// The random maps the setup page leaves out because the game has more than
/// 8 players, as `map_choices` does; its map browser lists the whole
/// catalog itself.
fn hidden_maps(catalog: &Catalog, settings: &Settings) -> Vec<String> {
    if settings.map.kind != Some(MapType::Random)
        || settings.player_count.players <= MAX_PLAYERS_SMALL_MAP
    {
        return Vec::new();
    }
    catalog
        .maps
        .iter()
        .filter(|map| {
            let name = map.path.rsplit('/').next().unwrap_or_default();
            map.kind == MapType::Random && MAPS_UP_TO_8_PLAYERS.contains(&name)
        })
        .map(|map| map.path.clone())
        .collect()
}
