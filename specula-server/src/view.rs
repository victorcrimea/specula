//! The viewer stream, wire format v4: one binary WebSocket message per
//! keyframe or turn, varints and zigzag as in `wire.rs`, positions in 1/16 m.
//!
//! - kind 1 keyframe: varint turn, u64 match id, f32 game speed (the
//!   match's sim rate), varint first turn of the match's history, varint
//!   turns per segment, varint map size in metres, u64 terrain id (0: none
//!   yet, else fetch `/terrain/<id as 16 hex digits>`), varint icon count,
//!   count x icon, player table, varint count, then count x {varint id
//!   delta, zigzag x, zigzag z, u32 colour, varint icon, varint player}.
//! - kind 2 delta: varint turn, varint count of icons added to the table,
//!   count x icon, u8 1 if a new player table follows (it replaces the old
//!   one) else 0, varint spawned, destroyed and changed counts. Spawned
//!   entries are keyframe entries, destroyed entries are id deltas, changed
//!   entries are {varint id delta, u8 mask, zigzag dx and dz if mask & 1,
//!   u32 colour if mask & 2, varint icon if mask & 4, varint player if
//!   mask & 8}.
//!
//! An icon is {varint name length, UTF-8 name, f32 size in CSS pixels}; the
//! name is an entry of `/icons.bin`. An entity's icon is a 1-based index
//! into the icon table, or 0 for none.
//! A player table is varint count n, then n x {varint name length, UTF-8
//! name, u32 colour, zigzag team (-1: none), u8 state (0 active, 1
//! defeated, 2 won), n x u8 stance towards player j (0 enemy, 1 ally, 2
//! neutral)}; index 0 is gaia.
//! Colour is 0xFFRRGGBB, or 0 for an entity that is not on the minimap.
//! An entity's player is its owner when its colour is that player's colour,
//! so a viewer may recolour it; 0 when the colour is fixed.
//! Ids in each list are sorted and delta-coded from 0.
//!
//! The match's history is split into segments, served at
//! `/match/<match id as 16 hex digits>/seg/<n>` as one zstd frame of
//! {varint length, message} entries. Segment n holds the turns from
//! `first turn + n * turns per segment` on; after a gap in the turns it
//! starts at the first turn it has. Its first message is a keyframe of its
//! first turn T, then comes the delta into T from the turn before (absent
//! in segment 0), so playback can cross into it without a keyframe, then
//! the deltas of the later turns. Every segment but the last is immutable.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use rocket::tokio::sync::broadcast;

use crate::feed::Entity;
use crate::feed::Icon;
use crate::feed::Player;
use crate::wire::Writer;
use crate::wire::diff;
use crate::wire::quantize_position;

const MESSAGE_KEYFRAME: u8 = 1;
const MESSAGE_DELTA: u8 = 2;
const CHANGED_POSITION: u8 = 1;
const CHANGED_COLOR: u8 = 2;
const CHANGED_ICON: u8 = 4;
const CHANGED_PLAYER: u8 = 8;
/// About 50 s of turns; a viewer further behind is resent a keyframe.
const BACKLOG_MESSAGES: usize = 256;
/// 50 turns of 200 ms: a keyframe every 10 s of game time (init.md 3).
const SEGMENT_TURNS: u32 = 50;
const SEGMENT_ZSTD_LEVEL: i32 = 3;

pub type Message = Arc<[u8]>;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Dot {
    x: i32,
    z: i32,
    color: u32,
    icon: u32,
    player: u32,
}

impl Dot {
    fn new(entity: &Entity, icon: u32, player_color: bool) -> Self {
        // Gaia keeps its colour in every diplomacy mode, as in the game.
        let player = match u32::try_from(entity.owner) {
            Ok(owner) if player_color && owner > 0 && entity.color.is_some() => owner,
            _ => 0,
        };
        Dot {
            x: quantize_position(entity.x),
            z: quantize_position(entity.z),
            color: entity.color.map_or(0, |rgb| 0xFF00_0000 | rgb),
            icon,
            player,
        }
    }
}

pub struct Hub {
    state: Mutex<State>,
}

struct State {
    turn: u32,
    map_size: u32,
    /// The sim rate the match was set up with.
    game_speed: f32,
    /// Names the current match's segments, so a URL from an earlier match
    /// cannot return this one's.
    match_id: u64,
    /// The turn segment 0 starts at.
    first_turn: u32,
    /// Closed segments, zstd-compressed.
    segments: Vec<Message>,
    /// The open segment, index `segments.len()`, uncompressed.
    open: Vec<u8>,
    terrain: Option<(u64, Message)>,
    dots: BTreeMap<u32, Dot>,
    /// Append-only per feed, so indices already sent stay valid.
    icons: Vec<Icon>,
    /// How many icons viewers have been sent; later ones go out with the
    /// next turn.
    icons_sent: usize,
    /// Template index to 1-based icon index.
    template_icons: HashMap<u32, u32>,
    /// Templates whose minimap colour is the owner's.
    player_color_templates: HashSet<u32>,
    players: Vec<Player>,
    /// The next delta must carry the player table.
    players_changed: bool,
    has_turn: bool,
    /// Set when a new observer connects: its entity ids belong to another
    /// match, so its first turn must replace, not patch, what viewers have.
    new_feed: bool,
    sender: broadcast::Sender<Message>,
}

impl Hub {
    pub fn new() -> Self {
        Hub {
            state: Mutex::new(State {
                turn: 0,
                map_size: 0,
                game_speed: 1.0,
                match_id: 0,
                first_turn: 0,
                segments: Vec::new(),
                open: Vec::new(),
                terrain: None,
                dots: BTreeMap::new(),
                icons: Vec::new(),
                icons_sent: 0,
                template_icons: HashMap::new(),
                player_color_templates: HashSet::new(),
                players: Vec::new(),
                players_changed: false,
                has_turn: false,
                new_feed: true,
                sender: broadcast::channel(BACKLOG_MESSAGES).0,
            }),
        }
    }

    pub fn start_feed(&self) {
        let mut state = self.state.lock().unwrap();
        state.new_feed = true;
        state.terrain = None;
        state.icons.clear();
        state.icons_sent = 0;
        state.template_icons.clear();
        state.player_color_templates.clear();
        state.players.clear();
    }

    pub fn set_template_player_color(&self, template: u32) {
        self.state
            .lock()
            .unwrap()
            .player_color_templates
            .insert(template);
    }

    pub fn set_players(&self, players: Vec<Player>) {
        let mut state = self.state.lock().unwrap();
        if state.players != players {
            state.players = players;
            state.players_changed = true;
        }
    }

    pub fn set_template_icon(&self, template: u32, icon: Icon) {
        let state = &mut *self.state.lock().unwrap();
        let index = match state
            .icons
            .iter()
            .position(|known| known.name == icon.name && known.size == icon.size)
        {
            Some(index) => index,
            None => {
                state.icons.push(icon);
                state.icons.len() - 1
            }
        };
        state.template_icons.insert(template, index as u32 + 1);
    }

    pub fn set_terrain(&self, id: u64, bytes: Vec<u8>) {
        self.state.lock().unwrap().terrain = Some((id, bytes.into()));
    }

    /// Only the current match's terrain is kept; an old id is gone.
    pub fn terrain(&self, id: u64) -> Option<Message> {
        let state = self.state.lock().unwrap();
        let (current, bytes) = state.terrain.as_ref()?;
        (*current == id).then(|| Arc::clone(bytes))
    }

    pub fn set_map(&self, size: u32, game_speed: f32) {
        let mut state = self.state.lock().unwrap();
        state.map_size = size;
        state.game_speed = game_speed;
    }

    pub fn publish(&self, turn: u32, entities: &[Entity]) -> io::Result<()> {
        let state = &mut *self.state.lock().unwrap();
        let dots: BTreeMap<u32, Dot> = entities
            .iter()
            .map(|entity| {
                let icon = state
                    .template_icons
                    .get(&entity.template)
                    .copied()
                    .unwrap_or(0);
                let player_color = state.player_color_templates.contains(&entity.template);
                (entity.id, Dot::new(entity, icon, player_color))
            })
            .collect();
        let message: Message = if state.new_feed {
            // The history restarts only now, so until the new match's first
            // turn, viewers and segment requests still get the old one.
            state.match_id = new_match_id(state.match_id);
            state.first_turn = turn;
            state.segments.clear();
            state.open.clear();
            let keyframe: Message = encode_keyframe(state, turn, &state.icons, &dots).into();
            append_framed(&mut state.open, &keyframe);
            keyframe
        } else {
            let delta: Message = encode_delta(
                turn,
                &state.icons[state.icons_sent..],
                state.players_changed.then_some(&state.players[..]),
                &state.dots,
                &dots,
            )
            .into();
            let next_start = state.first_turn + SEGMENT_TURNS * (state.segments.len() as u32 + 1);
            if turn >= next_start {
                let closed = zstd::bulk::compress(&state.open, SEGMENT_ZSTD_LEVEL)?;
                state.segments.push(closed.into());
                state.open.clear();
                let keyframe = encode_keyframe(state, turn, &state.icons, &dots);
                append_framed(&mut state.open, &keyframe);
            }
            append_framed(&mut state.open, &delta);
            delta
        };
        state.icons_sent = state.icons.len();
        state.players_changed = false;
        state.turn = turn;
        state.dots = dots;
        state.has_turn = true;
        state.new_feed = false;
        // No receivers is not an error: nobody is watching yet.
        let _ = state.sender.send(message);
        Ok(())
    }

    /// A segment of the current match, and whether it is closed. The open
    /// one is compressed on each request; it changes every turn anyway.
    pub fn segment(&self, match_id: u64, index: usize) -> Option<(Message, bool)> {
        let state = self.state.lock().unwrap();
        if match_id != state.match_id || !state.has_turn {
            return None;
        }
        if let Some(closed) = state.segments.get(index) {
            return Some((Arc::clone(closed), true));
        }
        if index != state.segments.len() {
            return None;
        }
        match zstd::bulk::compress(&state.open, SEGMENT_ZSTD_LEVEL) {
            Ok(bytes) => Some((bytes.into(), false)),
            Err(error) => {
                eprintln!("cannot compress the open segment: {error}");
                None
            }
        }
    }

    /// The keyframe (none before the first turn) and the receiver are taken
    /// under one lock, so the first delta a viewer receives applies to
    /// exactly that keyframe.
    pub fn subscribe(&self) -> (Option<Message>, broadcast::Receiver<Message>) {
        let state = self.state.lock().unwrap();
        // Icons not sent yet are not used by these dots, and the next delta
        // will add them, so they must not be in this table too.
        let keyframe = state.has_turn.then(|| {
            encode_keyframe(
                &state,
                state.turn,
                &state.icons[..state.icons_sent],
                &state.dots,
            )
            .into()
        });
        (keyframe, state.sender.subscribe())
    }
}

fn terrain_id(state: &State) -> u64 {
    state.terrain.as_ref().map_or(0, |(id, _)| *id)
}

/// Distinct from the previous id even if the clock did not move.
fn new_match_id(previous: u64) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64);
    if now == previous { now + 1 } else { now }
}

fn append_framed(segment: &mut Vec<u8>, message: &[u8]) {
    let mut out = Writer(std::mem::take(segment));
    out.varint(message.len() as u64);
    out.0.extend_from_slice(message);
    *segment = out.0;
}

/// `icons` are passed apart from `state` because a subscriber must not get
/// icons that the next delta will add again.
fn encode_keyframe(state: &State, turn: u32, icons: &[Icon], dots: &BTreeMap<u32, Dot>) -> Vec<u8> {
    let mut out = Writer::default();
    out.u8(MESSAGE_KEYFRAME);
    out.varint(u64::from(turn));
    out.u64(state.match_id);
    out.f32(state.game_speed);
    out.varint(u64::from(state.first_turn));
    out.varint(u64::from(SEGMENT_TURNS));
    out.varint(u64::from(state.map_size));
    out.u64(terrain_id(state));
    write_icons(&mut out, icons);
    write_players(&mut out, &state.players);
    out.varint(dots.len() as u64);
    let mut last_id = 0;
    for (id, dot) in dots {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_dot(&mut out, dot);
    }
    out.0
}

fn encode_delta(
    turn: u32,
    new_icons: &[Icon],
    new_players: Option<&[Player]>,
    previous: &BTreeMap<u32, Dot>,
    current: &BTreeMap<u32, Dot>,
) -> Vec<u8> {
    let diff = diff(previous, current);
    let mut out = Writer::default();
    out.u8(MESSAGE_DELTA);
    out.varint(u64::from(turn));
    write_icons(&mut out, new_icons);
    match new_players {
        Some(players) => {
            out.u8(1);
            write_players(&mut out, players);
        }
        None => out.u8(0),
    }
    out.varint(diff.spawned.len() as u64);
    out.varint(diff.destroyed.len() as u64);
    out.varint(diff.changed.len() as u64);
    let mut last_id = 0;
    for (id, dot) in &diff.spawned {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_dot(&mut out, dot);
    }
    let mut last_id = 0;
    for id in &diff.destroyed {
        out.varint(u64::from(id - last_id));
        last_id = *id;
    }
    let mut last_id = 0;
    for (id, before, now) in &diff.changed {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        let position = before.x != now.x || before.z != now.z;
        let color = before.color != now.color;
        let icon = before.icon != now.icon;
        let player = before.player != now.player;
        let mut mask = 0;
        if position {
            mask |= CHANGED_POSITION;
        }
        if color {
            mask |= CHANGED_COLOR;
        }
        if icon {
            mask |= CHANGED_ICON;
        }
        if player {
            mask |= CHANGED_PLAYER;
        }
        out.u8(mask);
        if position {
            out.zigzag(i64::from(now.x) - i64::from(before.x));
            out.zigzag(i64::from(now.z) - i64::from(before.z));
        }
        if color {
            out.u32(now.color);
        }
        if icon {
            out.varint(u64::from(now.icon));
        }
        if player {
            out.varint(u64::from(now.player));
        }
    }
    out.0
}

fn write_icons(out: &mut Writer, icons: &[Icon]) {
    out.varint(icons.len() as u64);
    for icon in icons {
        out.varint(icon.name.len() as u64);
        out.0.extend_from_slice(icon.name.as_bytes());
        out.f32(icon.size);
    }
}

fn write_players(out: &mut Writer, players: &[Player]) {
    out.varint(players.len() as u64);
    for player in players {
        out.varint(player.name.len() as u64);
        out.0.extend_from_slice(player.name.as_bytes());
        out.u32(0xFF00_0000 | player.color);
        out.zigzag(i64::from(player.team));
        out.u8(player.state);
        out.0.extend_from_slice(&player.stances);
    }
}

fn write_dot(out: &mut Writer, dot: &Dot) {
    out.zigzag(i64::from(dot.x));
    out.zigzag(i64::from(dot.z));
    out.u32(dot.color);
    out.varint(u64::from(dot.icon));
    out.varint(u64::from(dot.player));
}
