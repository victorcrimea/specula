//! The viewer stream, wire format v5: one binary WebSocket message per
//! keyframe or turn, varints and zigzag as in `wire.rs`, positions and
//! heights in 1/16 m.
//!
//! - kind 1 keyframe: varint turn, u64 match id, f32 game speed (the
//!   match's sim rate), varint first turn of the match's history, varint
//!   turns per segment, varint map size in metres, u64 terrain id (0: none
//!   yet, else fetch `/terrain/<id as 16 hex digits>`), icon table, player
//!   table, template table, string table, variant set table, varint count,
//!   then count x {varint id delta, entity}.
//! - kind 2 delta: varint turn, icons added to the table, u8 1 if a new
//!   player table follows (it replaces the old one) else 0, templates,
//!   strings and variant sets added to their tables, varint spawned,
//!   destroyed and changed counts. Spawned entries are keyframe entries,
//!   destroyed entries are id deltas, changed entries are {varint id delta,
//!   varint mask, then for each bit set, in this order: 1 zigzag dx, dz, dy;
//!   2 u32 colour; 4 varint icon; 8 varint player; 16 u8 angle; 32 varint
//!   animation, u8 animation flags; 64 varint animation speed; 128 varint
//!   sync repeat, zigzag sync offset; 256 zigzag owner; 512 varint template,
//!   varint actor, zigzag seed - id; 1024 varint variant set; 2048 u8
//!   construction}. An animation (32) starts, and a sync (128) is set, at
//!   the delta's turn.
//!
//! An entity is {zigzag x, zigzag z, zigzag y, u8 angle, u32 colour, varint
//! icon, varint player, zigzag owner, varint template, varint actor, zigzag
//! seed - id, varint variant set, varint animation, u8 animation flags,
//! varint animation speed, varint turns since the animation started, varint
//! sync repeat, zigzag sync offset, varint turns since the sync was set, u8
//! construction}:
//! - y is the absolute height, not the height above the ground;
//! - angle is the heading in 1/256 turn, clockwise from +z;
//! - colour is 0xFFRRGGBB, or 0 for an entity that is not on the minimap;
//! - player is the owner when the colour is that player's colour, so a
//!   viewer may recolour it; 0 when the colour is fixed;
//! - owner is the player index, -1 for none;
//! - template is a 0-based index into the template table;
//! - actor and animation are 1-based indices into the string table, 0 for
//!   none (no VisualActor, or no animation selected yet); seed is the
//!   actor's variation seed, meaningless without an actor;
//! - variant set is a 1-based index into the variant set table, 0 for no
//!   selections;
//! - animation flags: 1 plays once rather than loops;
//! - animation speed is in 1/256: m/s for walk and run, else a multiplier;
//! - sync repeat and offset are in ms, as the simulation gave them to
//!   `SetAnimationSyncRepeat`/`SetAnimationSyncOffset` (0: not synced);
//! - construction is how far the model has risen out of the ground in
//!   1/255, 0 when it is not offset at all.
//!
//! An icon table is varint count, count x {varint name length, UTF-8 name,
//! f32 size in CSS pixels}; the name is an entry of `/icons.bin`. An
//! entity's icon is a 1-based index into it, or 0 for none.
//! A player table is varint count n, then n x {varint name length, UTF-8
//! name, u32 colour, zigzag team (-1: none), u8 state (0 active, 1
//! defeated, 2 won), n x u8 stance towards player j (0 enemy, 1 ally, 2
//! neutral)}; index 0 is gaia.
//! Template and string tables are varint count, count x {varint length,
//! UTF-8 text}. Template names are as the simulation knows them, e.g.
//! `units/athen/infantry_spearman_b` or `foundation|structures/athen/house`;
//! a foundation's rising building is an entity of its own with a
//! `construction|` template. Strings are actor file names, animation names
//! and variant keys and selections.
//! A variant set table is varint count, count x {varint n, n x {varint key,
//! varint selection}}, 0-based string indices, sorted by key. The set holds
//! the VisualActor's variant selections except the `animation` key, which
//! always holds the animation name.
//! Tables only grow during a match, so an index once sent stays valid.
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
use std::io::ErrorKind;
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
use crate::wire::quantize_angle;
use crate::wire::quantize_position;

const MESSAGE_KEYFRAME: u8 = 1;
const MESSAGE_DELTA: u8 = 2;
const CHANGED_POSITION: u64 = 1;
const CHANGED_COLOR: u64 = 2;
const CHANGED_ICON: u64 = 4;
const CHANGED_PLAYER: u64 = 8;
const CHANGED_ANGLE: u64 = 16;
const CHANGED_ANIMATION: u64 = 32;
const CHANGED_ANIMATION_SPEED: u64 = 64;
const CHANGED_SYNC: u64 = 128;
const CHANGED_OWNER: u64 = 256;
const CHANGED_MODEL: u64 = 512;
const CHANGED_VARIANTS: u64 = 1024;
const CHANGED_CONSTRUCTION: u64 = 2048;
const ANIMATION_ONCE: u8 = 1;
/// About 50 s of turns; a viewer further behind is resent a keyframe.
const BACKLOG_MESSAGES: usize = 256;
/// 50 turns of 200 ms: a keyframe every 10 s of game time (init.md 3).
const SEGMENT_TURNS: u32 = 50;
const SEGMENT_ZSTD_LEVEL: i32 = 3;

pub type Message = Arc<[u8]>;

/// (key, selection) string indices, sorted by key.
pub type VariantSet = Vec<(u32, u32)>;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Dot {
    x: i32,
    z: i32,
    y: i32,
    angle: u8,
    color: u32,
    icon: u32,
    player: u32,
    owner: i32,
    template: u32,
    actor: u32,
    seed: u32,
    variants: u32,
    animation: u32,
    once: bool,
    speed: u32,
    /// The turn the animation was selected on.
    animation_turn: u32,
    sync_repeat: u32,
    sync_offset: i32,
    /// The turn the sync was set on.
    sync_turn: u32,
    construction: u8,
}

impl Dot {
    fn new(entity: &Entity, icon: u32, player_color: bool, turn: u32) -> Self {
        // Gaia keeps its colour in every diplomacy mode, as in the game.
        let player = match u32::try_from(entity.owner) {
            Ok(owner) if player_color && owner > 0 && entity.color.is_some() => owner,
            _ => 0,
        };
        let one_based = |index: Option<u32>| index.map_or(0, |index| index + 1);
        let visual = entity.visual;
        Dot {
            x: quantize_position(entity.x),
            z: quantize_position(entity.z),
            y: quantize_position(entity.y),
            angle: quantize_angle(entity.angle),
            color: entity.color.map_or(0, |rgb| 0xFF00_0000 | rgb),
            icon,
            player,
            owner: entity.owner,
            template: entity.template,
            actor: one_based(visual.and_then(|visual| visual.actor)),
            seed: visual.map_or(entity.id, |visual| visual.seed),
            variants: one_based(visual.and_then(|visual| visual.variants)),
            animation: one_based(visual.and_then(|visual| visual.animation)),
            once: visual.is_some_and(|visual| visual.once),
            speed: visual.map_or(0, |visual| quantize_speed(visual.speed)),
            animation_turn: turn,
            sync_repeat: visual.map_or(0, |visual| {
                quantize_millis(visual.sync_repeat).max(0) as u32
            }),
            sync_offset: visual.map_or(0, |visual| quantize_millis(visual.sync_offset)),
            sync_turn: turn,
            construction: quantize_construction(entity.construction),
        }
    }
}

/// Raw 16.16 to 1/256, rounded; a speed is never negative.
fn quantize_speed(raw: i32) -> u32 {
    ((i64::from(raw) + 128) >> 8).max(0) as u32
}

/// Raw 16.16 milliseconds to whole ones, rounded.
fn quantize_millis(raw: i32) -> i32 {
    ((i64::from(raw) + 0x8000) >> 16) as i32
}

/// Raw 16.16 progress to 1/255, rounded up so that a started construction
/// never reads as 0, which means no offset at all.
fn quantize_construction(raw: i32) -> u8 {
    if raw <= 0 {
        return 0;
    }
    ((i64::from(raw) * 255 + 0xFFFF) >> 16).clamp(1, 255) as u8
}

/// An append-only table: indices already sent stay valid, and entries added
/// since the last turn go out with the next one.
struct Table<T> {
    items: Vec<T>,
    sent: usize,
}

impl<T> Table<T> {
    fn new() -> Self {
        Table {
            items: Vec::new(),
            sent: 0,
        }
    }

    fn clear(&mut self) {
        self.items.clear();
        self.sent = 0;
    }

    /// The hook numbers its records densely from 0, so its index is the
    /// table index.
    fn insert(&mut self, index: u32, item: T, what: &str) -> io::Result<()> {
        if index as usize != self.items.len() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("{what} {index} out of order, expected {}", self.items.len()),
            ));
        }
        self.items.push(item);
        Ok(())
    }

    fn unsent(&self) -> &[T] {
        &self.items[self.sent..]
    }
}

/// The tables a keyframe carries.
struct Tables<'a> {
    icons: &'a [Icon],
    templates: &'a [String],
    strings: &'a [String],
    variant_sets: &'a [VariantSet],
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
    icons: Table<Icon>,
    templates: Table<String>,
    strings: Table<String>,
    variant_sets: Table<VariantSet>,
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

impl State {
    /// A subscriber must not get the unsent entries, which the next delta
    /// will add again; a keyframe of the turn being published must.
    fn tables(&self, with_unsent: bool) -> Tables<'_> {
        fn sent<T>(table: &Table<T>, with_unsent: bool) -> &[T] {
            if with_unsent {
                &table.items
            } else {
                &table.items[..table.sent]
            }
        }
        Tables {
            icons: sent(&self.icons, with_unsent),
            templates: sent(&self.templates, with_unsent),
            strings: sent(&self.strings, with_unsent),
            variant_sets: sent(&self.variant_sets, with_unsent),
        }
    }
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
                icons: Table::new(),
                templates: Table::new(),
                strings: Table::new(),
                variant_sets: Table::new(),
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
        state.templates.clear();
        state.strings.clear();
        state.variant_sets.clear();
        state.template_icons.clear();
        state.player_color_templates.clear();
        state.players.clear();
    }

    pub fn add_template(&self, index: u32, name: String) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .templates
            .insert(index, name, "template")
    }

    pub fn add_string(&self, index: u32, text: String) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .strings
            .insert(index, text, "string")
    }

    pub fn add_variant_set(&self, index: u32, pairs: VariantSet) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .variant_sets
            .insert(index, pairs, "variant set")
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
        let icons = &mut state.icons.items;
        let index = match icons
            .iter()
            .position(|known| known.name == icon.name && known.size == icon.size)
        {
            Some(index) => index,
            None => {
                icons.push(icon);
                icons.len() - 1
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
                let mut dot = Dot::new(entity, icon, player_color, turn);
                // The hook only sees the state after each turn, so an
                // animation counts as started on the turn it first shows.
                // The old match's ids are unrelated to the new one's.
                if let Some(before) = state.dots.get(&entity.id).filter(|_| !state.new_feed) {
                    if before.animation == dot.animation && before.once == dot.once {
                        dot.animation_turn = before.animation_turn;
                    }
                    if before.sync_repeat == dot.sync_repeat
                        && before.sync_offset == dot.sync_offset
                    {
                        dot.sync_turn = before.sync_turn;
                    }
                }
                (entity.id, dot)
            })
            .collect();
        let message: Message = if state.new_feed {
            // The history restarts only now, so until the new match's first
            // turn, viewers and segment requests still get the old one.
            state.match_id = new_match_id(state.match_id);
            state.first_turn = turn;
            state.segments.clear();
            state.open.clear();
            let keyframe: Message = encode_keyframe(state, turn, &state.tables(true), &dots).into();
            append_framed(&mut state.open, &keyframe);
            keyframe
        } else {
            let delta: Message = encode_delta(
                turn,
                &Tables {
                    icons: state.icons.unsent(),
                    templates: state.templates.unsent(),
                    strings: state.strings.unsent(),
                    variant_sets: state.variant_sets.unsent(),
                },
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
                let keyframe = encode_keyframe(state, turn, &state.tables(true), &dots);
                append_framed(&mut state.open, &keyframe);
            }
            append_framed(&mut state.open, &delta);
            delta
        };
        state.icons.sent = state.icons.items.len();
        state.templates.sent = state.templates.items.len();
        state.strings.sent = state.strings.items.len();
        state.variant_sets.sent = state.variant_sets.items.len();
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
        let keyframe = state
            .has_turn
            .then(|| encode_keyframe(&state, state.turn, &state.tables(false), &state.dots).into());
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

fn encode_keyframe(
    state: &State,
    turn: u32,
    tables: &Tables,
    dots: &BTreeMap<u32, Dot>,
) -> Vec<u8> {
    let mut out = Writer::default();
    out.u8(MESSAGE_KEYFRAME);
    out.varint(u64::from(turn));
    out.u64(state.match_id);
    out.f32(state.game_speed);
    out.varint(u64::from(state.first_turn));
    out.varint(u64::from(SEGMENT_TURNS));
    out.varint(u64::from(state.map_size));
    out.u64(terrain_id(state));
    write_icons(&mut out, tables.icons);
    write_players(&mut out, &state.players);
    write_strings(&mut out, tables.templates);
    write_strings(&mut out, tables.strings);
    write_variant_sets(&mut out, tables.variant_sets);
    out.varint(dots.len() as u64);
    let mut last_id = 0;
    for (id, dot) in dots {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_dot(&mut out, *id, turn, dot);
    }
    out.0
}

fn encode_delta(
    turn: u32,
    new_tables: &Tables,
    new_players: Option<&[Player]>,
    previous: &BTreeMap<u32, Dot>,
    current: &BTreeMap<u32, Dot>,
) -> Vec<u8> {
    let diff = diff(previous, current);
    let mut out = Writer::default();
    out.u8(MESSAGE_DELTA);
    out.varint(u64::from(turn));
    write_icons(&mut out, new_tables.icons);
    match new_players {
        Some(players) => {
            out.u8(1);
            write_players(&mut out, players);
        }
        None => out.u8(0),
    }
    write_strings(&mut out, new_tables.templates);
    write_strings(&mut out, new_tables.strings);
    write_variant_sets(&mut out, new_tables.variant_sets);
    out.varint(diff.spawned.len() as u64);
    out.varint(diff.destroyed.len() as u64);
    out.varint(diff.changed.len() as u64);
    let mut last_id = 0;
    for (id, dot) in &diff.spawned {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_dot(&mut out, *id, turn, dot);
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
        write_change(&mut out, *id, before, now);
    }
    out.0
}

fn write_change(out: &mut Writer, id: u32, before: &Dot, now: &Dot) {
    let changes = [
        (
            CHANGED_POSITION,
            before.x != now.x || before.z != now.z || before.y != now.y,
        ),
        (CHANGED_COLOR, before.color != now.color),
        (CHANGED_ICON, before.icon != now.icon),
        (CHANGED_PLAYER, before.player != now.player),
        (CHANGED_ANGLE, before.angle != now.angle),
        (
            CHANGED_ANIMATION,
            before.animation != now.animation || before.once != now.once,
        ),
        (CHANGED_ANIMATION_SPEED, before.speed != now.speed),
        (
            CHANGED_SYNC,
            before.sync_repeat != now.sync_repeat || before.sync_offset != now.sync_offset,
        ),
        (CHANGED_OWNER, before.owner != now.owner),
        (
            CHANGED_MODEL,
            before.template != now.template || before.actor != now.actor || before.seed != now.seed,
        ),
        (CHANGED_VARIANTS, before.variants != now.variants),
        (
            CHANGED_CONSTRUCTION,
            before.construction != now.construction,
        ),
    ];
    let mask = changes
        .iter()
        .filter(|(_, changed)| *changed)
        .fold(0, |mask, (bit, _)| mask | bit);
    out.varint(mask);
    if mask & CHANGED_POSITION != 0 {
        out.zigzag(i64::from(now.x) - i64::from(before.x));
        out.zigzag(i64::from(now.z) - i64::from(before.z));
        out.zigzag(i64::from(now.y) - i64::from(before.y));
    }
    if mask & CHANGED_COLOR != 0 {
        out.u32(now.color);
    }
    if mask & CHANGED_ICON != 0 {
        out.varint(u64::from(now.icon));
    }
    if mask & CHANGED_PLAYER != 0 {
        out.varint(u64::from(now.player));
    }
    if mask & CHANGED_ANGLE != 0 {
        out.u8(now.angle);
    }
    if mask & CHANGED_ANIMATION != 0 {
        out.varint(u64::from(now.animation));
        out.u8(animation_flags(now));
    }
    if mask & CHANGED_ANIMATION_SPEED != 0 {
        out.varint(u64::from(now.speed));
    }
    if mask & CHANGED_SYNC != 0 {
        out.varint(u64::from(now.sync_repeat));
        out.zigzag(i64::from(now.sync_offset));
    }
    if mask & CHANGED_OWNER != 0 {
        out.zigzag(i64::from(now.owner));
    }
    if mask & CHANGED_MODEL != 0 {
        out.varint(u64::from(now.template));
        out.varint(u64::from(now.actor));
        out.zigzag(i64::from(now.seed) - i64::from(id));
    }
    if mask & CHANGED_VARIANTS != 0 {
        out.varint(u64::from(now.variants));
    }
    if mask & CHANGED_CONSTRUCTION != 0 {
        out.u8(now.construction);
    }
}

fn animation_flags(dot: &Dot) -> u8 {
    if dot.once { ANIMATION_ONCE } else { 0 }
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

fn write_strings(out: &mut Writer, strings: &[String]) {
    out.varint(strings.len() as u64);
    for text in strings {
        out.varint(text.len() as u64);
        out.0.extend_from_slice(text.as_bytes());
    }
}

fn write_variant_sets(out: &mut Writer, sets: &[VariantSet]) {
    out.varint(sets.len() as u64);
    for set in sets {
        out.varint(set.len() as u64);
        for (key, selection) in set {
            out.varint(u64::from(*key));
            out.varint(u64::from(*selection));
        }
    }
}

/// `turn` is the message's turn, which the animation and sync turns are
/// counted back from.
fn write_dot(out: &mut Writer, id: u32, turn: u32, dot: &Dot) {
    out.zigzag(i64::from(dot.x));
    out.zigzag(i64::from(dot.z));
    out.zigzag(i64::from(dot.y));
    out.u8(dot.angle);
    out.u32(dot.color);
    out.varint(u64::from(dot.icon));
    out.varint(u64::from(dot.player));
    out.zigzag(i64::from(dot.owner));
    out.varint(u64::from(dot.template));
    out.varint(u64::from(dot.actor));
    out.zigzag(i64::from(dot.seed) - i64::from(id));
    out.varint(u64::from(dot.variants));
    out.varint(u64::from(dot.animation));
    out.u8(animation_flags(dot));
    out.varint(u64::from(dot.speed));
    out.varint(u64::from(turn - dot.animation_turn));
    out.varint(u64::from(dot.sync_repeat));
    out.zigzag(i64::from(dot.sync_offset));
    out.varint(u64::from(turn - dot.sync_turn));
    out.u8(dot.construction);
}
