//! The viewer stream, wire format v10: one binary WebSocket message per
//! keyframe, turn or match end, varints and zigzag as in `wire.rs`, positions and
//! heights in 1/16 m.
//!
//! - kind 1 keyframe: varint turn, u64 match id, f32 game speed (the
//!   match's sim rate), varint first turn of the match's history, varint
//!   turns per segment, varint map size in metres, u64 terrain id (0: none
//!   yet, else fetch `/terrain/<id as 16 hex digits>`), icon table, player
//!   table, territory, template table, string table, variant set table,
//!   varint count, then count x {varint id delta, entity}, then the
//!   projectiles in flight or lying on the ground: varint count, count x
//!   {varint id delta, projectile, varint turns since the launch}.
//! - kind 2 delta: varint turn, icons added to the table, u8 1 if a new
//!   player table follows (it replaces the old one) else 0, u8 1 if a new
//!   territory follows (likewise) else 0, templates,
//!   strings and variant sets added to their tables, varint spawned,
//!   destroyed and changed counts. Spawned entries are keyframe entries,
//!   destroyed entries are id deltas, changed entries are {varint id delta,
//!   varint mask, then for each bit set, in this order: 1 zigzag x, z
//!   change of the step; 16384 zigzag dy; 2 u32 colour; 4 varint icon; 8
//!   varint player; 16 u8 angle; 32 varint animation, u8 animation flags,
//!   varint sound group; 64 varint animation speed; 128 varint sync
//!   repeat, zigzag sync offset; 256 zigzag owner; 512 varint template,
//!   varint actor, zigzag seed - id; 1024 varint variant set; 2048 u8
//!   construction; 4096 varint visibility; 8192 nothing}, then the sounds
//!   of the turn: varint count, count x sound, in the order the simulation
//!   played them, then the projectiles launched in the turn: varint count,
//!   count x {varint id delta, launch}, then the ids of the projectiles
//!   that hit something and are gone: varint count, count x id delta. An
//!   animation (32) starts, and a sync (128) is set, at the delta's turn;
//!   it is also sent when only its sound group changed.
//! - kind 3 ended: u64 match id, varint ms since the match ended. No more
//!   turns of that match follow; the next keyframe with another id is the
//!   next match. A viewer joining between matches gets the final keyframe
//!   and then this message.
//!
//! A delta moves the entities as a viewer would, and lists only the
//! entities whose result would be wrong. A delta applies in this order:
//! remove the destroyed entities; update every entity that is either
//! listed or has a step that is not zero, as below; add the spawned ones.
//! Updating an entity:
//! - its step, in 1/16 m per turn, changes by the change sent (1); it
//!   moves by the step in x and z;
//! - 8192 makes it follow the ground, 16384 makes it stop following it
//!   and adds dy to its height; while it follows the ground, its height is
//!   the ground's at its position, `CTerrain::GetExactGroundLevelFixed` at
//!   the wire position rounded to 1/16 m (the terrain resource's
//!   heightmap; nothing follows the ground without a terrain);
//! - if its step is not zero, the animation speed is |step| / 16 / 0.2 *
//!   256, rounded, unless 64 is sent, and the angle is atan2(step x, step
//!   z) in 1/256 turn, rounded, unless 16 is sent;
//! - every other field set in the mask is replaced.
//!
//! The server sends a speed or angle only when this reckoning is more than
//! 5% or 1/256 turn off, and a height when it is more than 1/16 m off, so
//! these three are near the simulation's, not equal to it.
//!
//! An entity is {zigzag x, zigzag z, zigzag y, u8 angle, u32 colour, varint
//! icon, varint player, zigzag owner, varint template, varint actor, zigzag
//! seed - id, varint variant set, varint animation, u8 animation flags,
//! varint sound group, varint animation speed, varint turns since the
//! animation started, varint sync repeat, zigzag sync offset, varint turns
//! since the sync was set, u8 construction, varint visibility, zigzag step
//! x, zigzag step z, u8 motion flags}:
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
//! - sound group is a 1-based index into the string table, 0 for none: the
//!   group, relative to audio/, the animation plays at its sound or event
//!   point (the actor's `sound`, else `event`), once per loop;
//! - animation speed is in 1/256: m/s for walk and run, else a multiplier;
//! - sync repeat and offset are in ms, as the simulation gave them to
//!   `SetAnimationSyncRepeat`/`SetAnimationSyncOffset` (0: not synced);
//! - construction is how far the model has risen out of the ground in
//!   1/255, 0 when it is not offset at all;
//! - visibility has bit p set when player p sees the entity (LOS visible),
//!   for p below 16;
//! - step is how far it moves each turn until a delta changes it, in 1/16
//!   m;
//! - motion flags: 1 follows the ground.
//!
//! A sound is {varint group, u8 target, then by target: 0 (an entity's)
//! varint entity id, zigzag owner, zigzag x, zigzag z, zigzag y, varint
//! visibility; 1 (at a position, heard regardless of LOS) zigzag x, zigzag
//! z, zigzag y; 2 (only for one player, not positioned) zigzag player}.
//! The group is a 0-based index into the string table, a sound group path
//! relative to audio/. An entity's position, owner and visibility are as of
//! the call, since it may be gone by the end of the turn. They are sent
//! relative to the entity's as the delta leaves it, or as it was before when
//! the delta destroyed it: owner and position minus its, visibility XOR its
//! (unchanged when the viewer does not know the entity). Keyframes carry
//! no sounds.
//!
//! A projectile is {varint shooter id (0: none), zigzag owner, varint
//! actor, zigzag seed - shooter id, varint variant set, zigzag x, zigzag z,
//! zigzag y launch point, zigzag x, zigzag z, zigzag y target, varint
//! speed, zigzag gravity, varint projectile actor, varint impact actor,
//! varint impact lifetime}, as `CCmpProjectileManager::LaunchProjectile`
//! was given it:
//! - owner, actor, seed and variant set are the shooter's as of the launch
//!   turn (the turn before when it died in it), as in an entity, so its
//!   projectile prop can be found after it is gone; owner -1 and the rest
//!   0 without a shooter;
//! - the launch point is the simulation's, not the shooter's ammo prop's;
//! - speed is horizontal, in 1/256 m/s; gravity in 1/256 m/s^2;
//! - the projectile and impact actors are 1-based indices into the string
//!   table, 0 for none: without a projectile actor the shooter's actor's
//!   `projectile` prop is drawn;
//! - impact lifetime is in ms.
//!
//! A launch, in a delta, is a projectile sent relative to its shooter as
//! sounds are: {varint shooter id, then only when the viewer does not know
//! the shooter (or it is 0): zigzag owner, varint actor, zigzag seed -
//! shooter id, varint variant set; then the launch point minus the
//! shooter's position (nothing subtracted without a known shooter), the
//! target minus the launch point, and the rest as in a projectile}; with a
//! known shooter, owner, actor, seed and variant set are the shooter's.
//!
//! A projectile is kept from its launch until it hits something or until
//! it would have lain on the ground for 30 s.
//!
//! An icon table is varint count, count x {varint name length, UTF-8 name,
//! f32 size in CSS pixels}; the name is an entry of `/icons.bin`. An
//! entity's icon is a 1-based index into it, or 0 for none.
//! A player table is varint count n, then n x {varint name length, UTF-8
//! name, u32 colour, zigzag team (-1: none), u8 state (0 active, 1
//! defeated, 2 won), n x u8 stance towards player j (0 enemy, 1 ally, 2
//! neutral), varint civ length, UTF-8 civ code (as in
//! simulation/data/civs/), u8 battle (1 when `BattleDetection` reports
//! one)}; index 0 is gaia.
//! A territory is varint width, varint height (both 0: none yet), then
//! runs of {varint length, u8 tile} covering width * height tiles,
//! row-major, x fastest. A tile is 8 m square, as `ICmpTerritoryManager`'s
//! grid: owner in 0x1F, connected 0x20, blinking 0x40.
//! Template and string tables are varint count, count x {varint length,
//! UTF-8 text}. Template names are as the simulation knows them, e.g.
//! `units/athen/infantry_spearman_b` or `foundation|structures/athen/house`;
//! a foundation's rising building is an entity of its own with a
//! `construction|` template. Strings are actor file names, animation names,
//! variant keys and selections and sound group paths.
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
//! the deltas of the later turns. Every segment but the last is immutable;
//! once the match has ended, the last is too. The segments and terrain of
//! the match before the current one stay served, so its viewers can keep
//! watching it.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use rocket::tokio::sync::broadcast;

use crate::feed::Entity;
use crate::feed::Icon;
use crate::feed::Launch;
use crate::feed::Player;
use crate::feed::Sound;
use crate::feed::SoundTarget;
use crate::feed::Territory;
use crate::terrain::Ground;
use crate::wire::Writer;
use crate::wire::quantize_angle;
use crate::wire::quantize_position;

const MESSAGE_KEYFRAME: u8 = 1;
const MESSAGE_DELTA: u8 = 2;
const MESSAGE_ENDED: u8 = 3;
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
const CHANGED_VISIBILITY: u64 = 4096;
const CHANGED_GROUNDED: u64 = 8192;
const CHANGED_HEIGHT: u64 = 16384;
const MOTION_GROUNDED: u8 = 1;
/// How far a reckoned animation speed may be off, in 1/256 of the
/// simulation's: 5%, as `DATA_OPTIMIZATION.md` measured.
const SPEED_TOLERANCE_256: u32 = 13;
/// How far a reckoned heading may be off, in 1/256 turn.
const ANGLE_TOLERANCE: u8 = 1;
/// A height within this of the ground, in 1/16 m, follows the ground.
const GROUND_TOLERANCE: i32 = 1;
const SOUND_ENTITY: u8 = 0;
const SOUND_POSITION: u8 = 1;
const SOUND_PLAYER: u8 = 2;
const ANIMATION_ONCE: u8 = 1;
/// The simulation's turn, in seconds.
const TURN_SECS: f64 = 0.2;
/// `PROJECTILE_DECAY_TIME`, plus a margin for the client's launch point,
/// which moves the landing a little from the one reckoned here.
const PROJECTILE_KEEP_SECS: f64 = 35.0;
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
    sound_group: u32,
    visibility: u16,
    /// What a viewer moves it by each turn; only in what viewers hold,
    /// zero in the simulation's state.
    step: [i32; 2],
    /// Its height is the ground's, likewise.
    grounded: bool,
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
            sound_group: one_based(visual.and_then(|visual| visual.sound_group)),
            visibility: entity.visibility,
            step: [0, 0],
            grounded: false,
        }
    }
}

/// A projectile as viewers are sent it.
#[derive(Clone, Copy)]
struct Shot {
    launch_turn: u32,
    /// The last turn it is sent in keyframes.
    last_turn: u32,
    shooter: u32,
    owner: i32,
    actor: u32,
    seed: u32,
    variants: u32,
    launch: [i32; 3],
    target: [i32; 3],
    speed: u32,
    gravity: i32,
    projectile_actor: u32,
    impact_actor: u32,
    impact_lifetime: u32,
}

impl Shot {
    fn new(launch: &Launch, turn: u32, shooter: Option<&Dot>) -> Self {
        let one_based = |index: Option<u32>| index.map_or(0, |index| index + 1);
        let metres = |raw: i32| f64::from(raw) / 65536.0;
        let flight_secs = {
            let dx = metres(launch.target[0]) - metres(launch.launch[0]);
            let dz = metres(launch.target[2]) - metres(launch.launch[2]);
            let speed = metres(launch.speed);
            if speed > 0.0 {
                dx.hypot(dz) / speed
            } else {
                0.0
            }
        };
        let impact_secs = metres(launch.impact_lifetime).max(0.0);
        let keep_turns =
            ((flight_secs + PROJECTILE_KEEP_SECS + impact_secs) / TURN_SECS).ceil() as u32;
        Shot {
            launch_turn: turn,
            last_turn: turn.saturating_add(keep_turns),
            shooter: if shooter.is_some() { launch.shooter } else { 0 },
            owner: shooter.map_or(-1, |dot| dot.owner),
            actor: shooter.map_or(0, |dot| dot.actor),
            seed: shooter.map_or(0, |dot| dot.seed),
            variants: shooter.map_or(0, |dot| dot.variants),
            launch: launch.launch.map(quantize_position),
            target: launch.target.map(quantize_position),
            speed: quantize_speed(launch.speed),
            gravity: ((i64::from(launch.gravity) + 128) >> 8) as i32,
            projectile_actor: one_based(launch.actor),
            impact_actor: one_based(launch.impact_actor),
            impact_lifetime: (impact_secs * 1000.0).round() as u32,
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

/// A finished match's history, kept for its viewers once the next one
/// starts.
struct Archive {
    match_id: u64,
    segments: Vec<Message>,
    terrain: Option<(u64, Message)>,
}

struct Ended {
    at: Instant,
    /// The final state, for viewers who join before the next match: the
    /// tables a keyframe would be built from belong to the next feed by
    /// then.
    keyframe: Message,
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
    ground: Option<Ground>,
    /// What viewers hold after the last turn, which the next delta builds
    /// on; near the simulation's state, not equal to it.
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
    territory: Option<Territory>,
    /// The next delta must carry the territory.
    territory_changed: bool,
    /// The sounds of the next turn.
    sounds: Vec<Sound>,
    /// The projectile calls of the next turn.
    launches: Vec<Launch>,
    removals: Vec<u32>,
    /// By id: launched and neither hit nor long on the ground.
    shots: BTreeMap<u32, Shot>,
    has_turn: bool,
    /// Set when a new observer connects: its entity ids belong to another
    /// match, so its first turn must replace, not patch, what viewers have.
    new_feed: bool,
    /// Set from the end of a match until the next one's first turn.
    ended: Option<Ended>,
    previous: Option<Archive>,
    /// When the feed last connected or sent a turn, for the observer
    /// watchdog.
    feed_activity: Option<Instant>,
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
                ground: None,
                dots: BTreeMap::new(),
                icons: Table::new(),
                templates: Table::new(),
                strings: Table::new(),
                variant_sets: Table::new(),
                template_icons: HashMap::new(),
                player_color_templates: HashSet::new(),
                players: Vec::new(),
                players_changed: false,
                territory: None,
                territory_changed: false,
                sounds: Vec::new(),
                launches: Vec::new(),
                removals: Vec::new(),
                shots: BTreeMap::new(),
                has_turn: false,
                new_feed: true,
                ended: None,
                previous: None,
                feed_activity: None,
                sender: broadcast::channel(BACKLOG_MESSAGES).0,
            }),
        }
    }

    pub fn start_feed(&self) {
        let mut state = self.state.lock().unwrap();
        state.new_feed = true;
        state.feed_activity = Some(Instant::now());
        state.terrain = None;
        state.ground = None;
        state.icons.clear();
        state.templates.clear();
        state.strings.clear();
        state.variant_sets.clear();
        state.template_icons.clear();
        state.player_color_templates.clear();
        state.players.clear();
        state.territory = None;
        state.sounds.clear();
        state.launches.clear();
        state.removals.clear();
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

    pub fn set_territory(&self, territory: Territory) {
        let mut state = self.state.lock().unwrap();
        if state.territory.as_ref() != Some(&territory) {
            state.territory = Some(territory);
            state.territory_changed = true;
        }
    }

    pub fn set_sounds(&self, sounds: Vec<Sound>) {
        self.state.lock().unwrap().sounds = sounds;
    }

    pub fn set_projectiles(&self, launches: Vec<Launch>, removals: Vec<u32>) {
        let state = &mut *self.state.lock().unwrap();
        state.launches = launches;
        state.removals = removals;
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

    pub fn set_terrain(&self, id: u64, bytes: Vec<u8>, ground: Ground) {
        let mut state = self.state.lock().unwrap();
        // Viewers hold heights reckoned from the match's first terrain.
        if state.has_turn && !state.new_feed {
            eprintln!("terrain {id:016x} arrived during a match, ignored");
            return;
        }
        state.terrain = Some((id, bytes.into()));
        state.ground = Some(ground);
    }

    /// Only the current and the previous match's terrain are kept; an
    /// older id is gone.
    pub fn terrain(&self, id: u64) -> Option<Message> {
        let state = self.state.lock().unwrap();
        let archived = state
            .previous
            .as_ref()
            .and_then(|archive| archive.terrain.as_ref());
        [state.terrain.as_ref(), archived]
            .into_iter()
            .flatten()
            .find(|(known, _)| *known == id)
            .map(|(_, bytes)| Arc::clone(bytes))
    }

    pub fn feed_activity(&self) -> Option<Instant> {
        self.state.lock().unwrap().feed_activity
    }

    /// Called when the feed is gone. A feed that never sent a turn ends
    /// nothing.
    pub fn end_match(&self) {
        let state = &mut *self.state.lock().unwrap();
        if !state.has_turn || state.ended.is_some() {
            return;
        }
        match zstd::bulk::compress(&state.open, SEGMENT_ZSTD_LEVEL) {
            Ok(closed) => {
                state.segments.push(closed.into());
                state.open.clear();
            }
            // The history then ends at the last closed segment.
            Err(error) => eprintln!("cannot compress the last segment: {error}"),
        }
        let keyframe: Message = encode_keyframe(
            state,
            state.turn,
            &state.tables(true),
            &state.dots,
            &state.shots,
        )
        .into();
        state.previous = Some(Archive {
            match_id: state.match_id,
            segments: std::mem::take(&mut state.segments),
            terrain: state.terrain.clone(),
        });
        state.ended = Some(Ended {
            at: Instant::now(),
            keyframe,
        });
        println!("match {:016x} ended at turn {}", state.match_id, state.turn);
        let _ = state.sender.send(encode_ended(state.match_id, 0).into());
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
        // The old match's projectile ids are unrelated to the new one's.
        let mut shots = if state.new_feed {
            BTreeMap::new()
        } else {
            state.shots.clone()
        };
        shots.retain(|_, shot| shot.last_turn >= turn);
        let mut removals: Vec<u32> = state
            .removals
            .iter()
            .copied()
            .filter(|id| shots.remove(id).is_some())
            .collect();
        removals.sort_unstable();
        let mut launches = Vec::with_capacity(state.launches.len());
        for launch in &state.launches {
            // Taken out in the turn that shot it: nothing to draw.
            if state.removals.contains(&launch.id) {
                continue;
            }
            let shooter = dots
                .get(&launch.shooter)
                .or_else(|| state.dots.get(&launch.shooter))
                .filter(|_| launch.shooter != 0);
            let shot = Shot::new(launch, turn, shooter);
            shots.insert(launch.id, shot);
            launches.push((launch.id, shot));
        }
        launches.sort_unstable_by_key(|(id, _)| *id);
        let (message, shown): (Message, BTreeMap<u32, Dot>) = if state.new_feed {
            // The history restarts only now, so until the new match's first
            // turn, viewers and segment requests still get the old one.
            state.match_id = new_match_id(state.match_id);
            state.ended = None;
            state.first_turn = turn;
            state.segments.clear();
            state.open.clear();
            let ground = state.ground.as_ref();
            let shown = dots
                .iter()
                .map(|(id, dot)| (*id, arrive(ground, dot)))
                .collect();
            let keyframe: Message =
                encode_keyframe(state, turn, &state.tables(true), &shown, &shots).into();
            append_framed(&mut state.open, &keyframe);
            (keyframe, shown)
        } else {
            let (delta, shown) = encode_delta(
                turn,
                &Tables {
                    icons: state.icons.unsent(),
                    templates: state.templates.unsent(),
                    strings: state.strings.unsent(),
                    variant_sets: state.variant_sets.unsent(),
                },
                state.players_changed.then_some(&state.players[..]),
                state
                    .territory_changed
                    .then_some(state.territory.as_ref())
                    .flatten(),
                state.ground.as_ref(),
                &state.dots,
                &dots,
                &state.sounds,
                &launches,
                &removals,
            );
            let delta: Message = delta.into();
            let next_start = state.first_turn + SEGMENT_TURNS * (state.segments.len() as u32 + 1);
            if turn >= next_start {
                let closed = zstd::bulk::compress(&state.open, SEGMENT_ZSTD_LEVEL)?;
                state.segments.push(closed.into());
                state.open.clear();
                let keyframe = encode_keyframe(state, turn, &state.tables(true), &shown, &shots);
                append_framed(&mut state.open, &keyframe);
            }
            append_framed(&mut state.open, &delta);
            (delta, shown)
        };
        state.icons.sent = state.icons.items.len();
        state.templates.sent = state.templates.items.len();
        state.strings.sent = state.strings.items.len();
        state.variant_sets.sent = state.variant_sets.items.len();
        state.players_changed = false;
        state.territory_changed = false;
        // A keyframe carries no sounds, so a new match's first turn drops
        // its own.
        state.sounds.clear();
        state.launches.clear();
        state.removals.clear();
        state.shots = shots;
        state.turn = turn;
        state.dots = shown;
        state.has_turn = true;
        state.new_feed = false;
        state.feed_activity = Some(Instant::now());
        // No receivers is not an error: nobody is watching yet.
        let _ = state.sender.send(message);
        Ok(())
    }

    /// A segment of the current or the previous match, and whether it is
    /// closed. The open one is compressed on each request; it changes every
    /// turn anyway.
    pub fn segment(&self, match_id: u64, index: usize) -> Option<(Message, bool)> {
        let state = self.state.lock().unwrap();
        // An ended match is archived but keeps its id until the next one
        // starts, so the archive is asked first.
        if let Some(archive) = state
            .previous
            .as_ref()
            .filter(|archive| archive.match_id == match_id)
        {
            return Some((Arc::clone(archive.segments.get(index)?), true));
        }
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

    /// What a new viewer is sent first (nothing before the first turn) and
    /// the receiver are taken under one lock, so the first delta a viewer
    /// receives applies to exactly that keyframe.
    pub fn subscribe(&self) -> (Vec<Message>, broadcast::Receiver<Message>) {
        let state = self.state.lock().unwrap();
        let first = if let Some(ended) = &state.ended {
            let since = ended.at.elapsed().as_millis() as u64;
            vec![
                Arc::clone(&ended.keyframe),
                encode_ended(state.match_id, since).into(),
            ]
        } else if state.has_turn {
            vec![
                encode_keyframe(
                    &state,
                    state.turn,
                    &state.tables(false),
                    &state.dots,
                    &state.shots,
                )
                .into(),
            ]
        } else {
            Vec::new()
        };
        (first, state.sender.subscribe())
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

fn encode_ended(match_id: u64, since_ms: u64) -> Vec<u8> {
    let mut out = Writer::default();
    out.u8(MESSAGE_ENDED);
    out.u64(match_id);
    out.varint(since_ms);
    out.0
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
    shots: &BTreeMap<u32, Shot>,
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
    write_territory(&mut out, state.territory.as_ref());
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
    out.varint(shots.len() as u64);
    let mut last_id = 0;
    for (id, shot) in shots {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_shot(&mut out, shot);
        out.varint(u64::from(turn - shot.launch_turn));
    }
    out.0
}

#[allow(clippy::too_many_arguments)]
fn encode_delta(
    turn: u32,
    new_tables: &Tables,
    new_players: Option<&[Player]>,
    new_territory: Option<&Territory>,
    ground: Option<&Ground>,
    previous: &BTreeMap<u32, Dot>,
    current: &BTreeMap<u32, Dot>,
    sounds: &[Sound],
    launches: &[(u32, Shot)],
    removals: &[u32],
) -> (Vec<u8>, BTreeMap<u32, Dot>) {
    let mut shown = BTreeMap::new();
    let mut spawned = Vec::new();
    let mut changed = Vec::new();
    for (id, now) in current {
        match previous.get(id) {
            None => {
                let dot = arrive(ground, now);
                spawned.push((*id, dot));
                shown.insert(*id, dot);
            }
            Some(before) => {
                let (change, dot) = reckon(ground, before, now);
                if change.mask != 0 {
                    changed.push((*id, change, now));
                }
                shown.insert(*id, dot);
            }
        }
    }
    let destroyed: BTreeMap<u32, Dot> = previous
        .iter()
        .filter(|(id, _)| !current.contains_key(id))
        .map(|(id, dot)| (*id, *dot))
        .collect();
    let known = |id: u32| shown.get(&id).or_else(|| destroyed.get(&id));
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
    match new_territory {
        Some(territory) => {
            out.u8(1);
            write_territory(&mut out, Some(territory));
        }
        None => out.u8(0),
    }
    write_strings(&mut out, new_tables.templates);
    write_strings(&mut out, new_tables.strings);
    write_variant_sets(&mut out, new_tables.variant_sets);
    out.varint(spawned.len() as u64);
    out.varint(destroyed.len() as u64);
    out.varint(changed.len() as u64);
    let mut last_id = 0;
    for (id, dot) in &spawned {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_dot(&mut out, *id, turn, dot);
    }
    let mut last_id = 0;
    for id in destroyed.keys() {
        out.varint(u64::from(id - last_id));
        last_id = *id;
    }
    let mut last_id = 0;
    for (id, change, now) in &changed {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_change(&mut out, *id, change, now);
    }
    out.varint(sounds.len() as u64);
    for sound in sounds {
        let entity = match sound.target {
            SoundTarget::Entity { id, .. } => known(id),
            _ => None,
        };
        write_sound(&mut out, sound, entity);
    }
    out.varint(launches.len() as u64);
    let mut last_id = 0;
    for (id, shot) in launches {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        let shooter = known(shot.shooter).filter(|_| shot.shooter != 0);
        write_launch(&mut out, shot, shooter);
    }
    out.varint(removals.len() as u64);
    let mut last_id = 0;
    for id in removals {
        out.varint(u64::from(id - last_id));
        last_id = *id;
    }
    (out.0, shown)
}

/// What a delta sends for one entity besides the fields it copies from
/// the simulation's state.
#[derive(Clone, Copy)]
struct Change {
    mask: u64,
    /// The change of the step.
    step: [i32; 2],
    /// Added to the height, with `CHANGED_HEIGHT`.
    dy: i32,
}

/// A new entity as viewers take it.
fn arrive(ground: Option<&Ground>, now: &Dot) -> Dot {
    let mut dot = *now;
    if let Some(ground) = ground {
        let height = ground.at(dot.x, dot.z);
        if (dot.y - height).abs() <= GROUND_TOLERANCE {
            dot.grounded = true;
            dot.y = height;
        }
    }
    dot
}

/// The update a viewer makes to `before` for `change`, the values sent
/// taken from `now`.
fn advance(ground: Option<&Ground>, before: &Dot, change: &Change, now: &Dot) -> Dot {
    let mut step = before.step;
    if change.mask & CHANGED_POSITION != 0 {
        step[0] += change.step[0];
        step[1] += change.step[1];
    }
    // The mask carries every exact field that differs, so those are now's.
    let mut dot = Dot {
        x: before.x + step[0],
        z: before.z + step[1],
        y: before.y,
        angle: before.angle,
        speed: before.speed,
        step,
        grounded: before.grounded,
        ..*now
    };
    if change.mask & CHANGED_GROUNDED != 0 {
        dot.grounded = true;
    }
    if change.mask & CHANGED_HEIGHT != 0 {
        dot.grounded = false;
        dot.y += change.dy;
    }
    if dot.grounded
        && let Some(ground) = ground
    {
        dot.y = ground.at(dot.x, dot.z);
    }
    let moved = step != [0, 0];
    if change.mask & CHANGED_ANGLE != 0 {
        dot.angle = now.angle;
    } else if moved {
        dot.angle = step_angle(step);
    }
    if change.mask & CHANGED_ANIMATION_SPEED != 0 {
        dot.speed = now.speed;
    } else if moved {
        dot.speed = step_speed(step);
    }
    dot
}

/// What to send so that a viewer's update of `before` comes near enough
/// to `now`, and the viewer's result.
fn reckon(ground: Option<&Ground>, before: &Dot, now: &Dot) -> (Change, Dot) {
    let mut change = Change {
        mask: exact_changes(before, now),
        step: [
            now.x - before.x - before.step[0],
            now.z - before.z - before.step[1],
        ],
        dy: 0,
    };
    if change.step != [0, 0] {
        change.mask |= CHANGED_POSITION;
    }
    // Each round adds what the viewer would still get wrong; height, speed
    // and angle do not depend on one another, so this settles in a few.
    for _ in 0..3 {
        let dot = advance(ground, before, &change, now);
        let mut more = 0;
        if dot.grounded {
            if (now.y - dot.y).abs() > GROUND_TOLERANCE {
                more |= CHANGED_HEIGHT;
            }
        } else if ground
            .is_some_and(|ground| (now.y - ground.at(dot.x, dot.z)).abs() <= GROUND_TOLERANCE)
        {
            more |= CHANGED_GROUNDED;
        } else if dot.y != now.y {
            more |= CHANGED_HEIGHT;
        }
        if u64::from(now.speed.abs_diff(dot.speed)) * 256
            > u64::from(now.speed) * u64::from(SPEED_TOLERANCE_256)
        {
            more |= CHANGED_ANIMATION_SPEED;
        }
        let angle_off = now
            .angle
            .wrapping_sub(dot.angle)
            .min(dot.angle.wrapping_sub(now.angle));
        if angle_off > ANGLE_TOLERANCE {
            more |= CHANGED_ANGLE;
        }
        if more & CHANGED_HEIGHT != 0 {
            // dy is from the height the viewer has before adding it.
            let probe = Change {
                mask: (change.mask | CHANGED_HEIGHT) & !CHANGED_GROUNDED,
                dy: 0,
                ..change
            };
            change.dy = now.y - advance(ground, before, &probe, now).y;
            change.mask &= !CHANGED_GROUNDED;
        }
        if more & !change.mask == 0 {
            break;
        }
        change.mask |= more;
    }
    (change, advance(ground, before, &change, now))
}

/// In 1/256 m/s, from 1/16 m per 0.2 s turn. `sqrt` is correctly rounded
/// everywhere, so viewers get exactly the same.
fn step_speed(step: [i32; 2]) -> u32 {
    let squared = i64::from(step[0]).pow(2) + i64::from(step[1]).pow(2);
    ((squared as f64).sqrt() * 80.0).round() as u32
}

/// In 1/256 turn, clockwise from +z.
fn step_angle(step: [i32; 2]) -> u8 {
    let turns = f64::from(step[0]).atan2(f64::from(step[1])) / std::f64::consts::TAU;
    (turns * 256.0).round().rem_euclid(256.0) as u8
}

/// The mask bits of the fields a viewer cannot reckon.
fn exact_changes(before: &Dot, now: &Dot) -> u64 {
    let changes = [
        (CHANGED_COLOR, before.color != now.color),
        (CHANGED_ICON, before.icon != now.icon),
        (CHANGED_PLAYER, before.player != now.player),
        (
            CHANGED_ANIMATION,
            before.animation != now.animation
                || before.once != now.once
                || before.sound_group != now.sound_group,
        ),
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
        (CHANGED_VISIBILITY, before.visibility != now.visibility),
    ];
    changes
        .iter()
        .filter(|(_, changed)| *changed)
        .fold(0, |mask, (bit, _)| mask | bit)
}

fn write_shot(out: &mut Writer, shot: &Shot) {
    out.varint(u64::from(shot.shooter));
    write_shot_origin(out, shot);
    for point in [shot.launch, shot.target] {
        out.zigzag(i64::from(point[0]));
        out.zigzag(i64::from(point[2]));
        out.zigzag(i64::from(point[1]));
    }
    write_shot_flight(out, shot);
}

fn write_shot_origin(out: &mut Writer, shot: &Shot) {
    out.zigzag(i64::from(shot.owner));
    out.varint(u64::from(shot.actor));
    out.zigzag(i64::from(shot.seed) - i64::from(shot.shooter));
    out.varint(u64::from(shot.variants));
}

fn write_shot_flight(out: &mut Writer, shot: &Shot) {
    out.varint(u64::from(shot.speed));
    out.zigzag(i64::from(shot.gravity));
    out.varint(u64::from(shot.projectile_actor));
    out.varint(u64::from(shot.impact_actor));
    out.varint(u64::from(shot.impact_lifetime));
}

/// `shooter` is the shooter as viewers know it, if they do.
fn write_launch(out: &mut Writer, shot: &Shot, shooter: Option<&Dot>) {
    out.varint(u64::from(shot.shooter));
    let origin = match shooter {
        Some(dot) => [dot.x, dot.y, dot.z],
        None => {
            write_shot_origin(out, shot);
            [0; 3]
        }
    };
    for axis in [0, 2, 1] {
        out.zigzag(i64::from(shot.launch[axis]) - i64::from(origin[axis]));
    }
    for axis in [0, 2, 1] {
        out.zigzag(i64::from(shot.target[axis]) - i64::from(shot.launch[axis]));
    }
    write_shot_flight(out, shot);
}

/// `entity` is the sound's entity as viewers know it, if they do.
fn write_sound(out: &mut Writer, sound: &Sound, entity: Option<&Dot>) {
    out.varint(u64::from(sound.group));
    let position = |out: &mut Writer, base: [i32; 3]| {
        out.zigzag(i64::from(quantize_position(sound.x)) - i64::from(base[0]));
        out.zigzag(i64::from(quantize_position(sound.z)) - i64::from(base[1]));
        out.zigzag(i64::from(quantize_position(sound.y)) - i64::from(base[2]));
    };
    match sound.target {
        SoundTarget::Entity { id, owner } => {
            let (base_owner, base, base_visibility) = entity.map_or((0, [0; 3], 0), |dot| {
                (dot.owner, [dot.x, dot.z, dot.y], dot.visibility)
            });
            out.u8(SOUND_ENTITY);
            out.varint(u64::from(id));
            out.zigzag(i64::from(owner) - i64::from(base_owner));
            position(out, base);
            out.varint(u64::from(sound.visibility ^ base_visibility));
        }
        SoundTarget::Position => {
            out.u8(SOUND_POSITION);
            position(out, [0; 3]);
        }
        SoundTarget::Player(player) => {
            out.u8(SOUND_PLAYER);
            out.zigzag(i64::from(player));
        }
    }
}

fn write_change(out: &mut Writer, id: u32, change: &Change, now: &Dot) {
    let mask = change.mask;
    out.varint(mask);
    if mask & CHANGED_POSITION != 0 {
        out.zigzag(i64::from(change.step[0]));
        out.zigzag(i64::from(change.step[1]));
    }
    if mask & CHANGED_HEIGHT != 0 {
        out.zigzag(i64::from(change.dy));
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
        out.varint(u64::from(now.sound_group));
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
    if mask & CHANGED_VISIBILITY != 0 {
        out.varint(u64::from(now.visibility));
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
        out.varint(player.civ.len() as u64);
        out.0.extend_from_slice(player.civ.as_bytes());
        out.u8(u8::from(player.battle));
    }
}

/// Run-length coded: a territory is a few large areas of one value.
fn write_territory(out: &mut Writer, territory: Option<&Territory>) {
    let Some(territory) = territory else {
        out.varint(0);
        out.varint(0);
        return;
    };
    out.varint(u64::from(territory.width));
    out.varint(u64::from(territory.height));
    for run in territory.tiles.chunk_by(|a, b| a == b) {
        out.varint(run.len() as u64);
        out.u8(run[0]);
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
    out.varint(u64::from(dot.sound_group));
    out.varint(u64::from(dot.speed));
    out.varint(u64::from(turn - dot.animation_turn));
    out.varint(u64::from(dot.sync_repeat));
    out.zigzag(i64::from(dot.sync_offset));
    out.varint(u64::from(turn - dot.sync_turn));
    out.u8(dot.construction);
    out.varint(u64::from(dot.visibility));
    out.zigzag(i64::from(dot.step[0]));
    out.zigzag(i64::from(dot.step[1]));
    out.u8(if dot.grounded { MOTION_GROUNDED } else { 0 });
}
