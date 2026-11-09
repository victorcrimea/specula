//! The viewer stream, wire format v18: messages for each keyframe, turn or
//! match end, varints and zigzag as in `wire.rs`, positions and heights in
//! 1/16 m, carried brotli-compressed as below.
//!
//! - kind 1 keyframe: varint turn, u8 wire version (18; a viewer stops
//!   at any other, since nothing after it would read right), u64 match id, f32 game speed (the
//!   match's sim rate), varint delay in ms (how long after the game the
//!   server shows it), varint first turn of the match's history, varint
//!   turns per segment, varint map size in metres, u64 terrain id (0: none
//!   yet, else fetch `/terrain/<id as 16 hex digits>`), icon table, player
//!   table, territory, template table, string table, variant set table,
//!   variable set table, range set table, varint n, n x economy (every player's, by index),
//!   varint count, then count x {varint id delta, entity}, then the
//!   projectiles in flight or lying on the ground: varint count, count x
//!   {varint id delta, projectile, varint turns since the launch}.
//! - kind 2 delta: varint turn, icons added to the table, u8 1 if a new
//!   player table follows (it replaces the old one) else 0, u8 1 if a new
//!   territory follows (likewise) else 0, templates,
//!   strings, variant sets, variable sets and range sets added to their tables, u8 1
//!   if economies follow else 0, then varint count, count x {varint player
//!   index, economy} of the players whose economy changed, varint spawned,
//!   destroyed and changed counts. Spawned entries are keyframe entries,
//!   destroyed entries are id deltas, changed entries are {varint id delta,
//!   varint mask, then for each bit set, in this order: 1 zigzag x, z
//!   change of the step; 16384 zigzag dy; 2 u32 colour; 4 varint icon; 8
//!   varint player; 16 u8 angle; 32 varint animation, u8 animation flags,
//!   varint sound group; 64 varint animation speed; 128 varint sync
//!   repeat, zigzag sync offset; 256 zigzag owner; 512 varint template,
//!   varint actor, zigzag seed - id; 1024 varint variant set; 2048 u8
//!   construction; 4096 varint visibility; 8192 nothing; 32768 varint
//!   variable set; 65536 u8 hitpoints; 131072 capture; 262144 varint range set}, then the sounds
//!   of the turn: varint count, count x sound, in the order the simulation
//!   played them, then the projectiles launched in the turn: varint count,
//!   count x {varint id delta, launch}, then the ids of the projectiles
//!   that hit something and are gone: varint count, count x id delta,
//!   then the entities the turn renamed: varint count, count x {varint old
//!   id delta, zigzag new id - old id}. An
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
//! seed - id, varint variant set, varint variable set, varint range set, varint animation,
//! u8 animation flags,
//! varint sound group, varint animation speed, varint turns since the
//! animation started, varint sync repeat, zigzag sync offset, varint turns
//! since the sync was set, u8 construction, varint visibility, zigzag step
//! x, zigzag step z, u8 motion flags, then u8 hitpoints if motion flag 2,
//! then capture if motion flag 4}:
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
//! - variable set is a 1-based index into the variable set table, 0 for
//!   no entity variables;
//! - range set is a 1-based index into the range set table, 0 for no
//!   range circles;
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
//! - motion flags: 1 follows the ground, 2 hitpoints follow, 4 capture
//!   follows;
//! - hitpoints are 0 without the Health component, else the hitpoints in
//!   1/255 of the max, rounded up and at least 1. Without motion flag 2
//!   they are 255 when the template's details have the Health component
//!   bit, else 0;
//! - capture is u8 n, n x u8 share of the max capture points by player
//!   index, in 1/255, rounded; n is 0 without Capturable, and so it is
//!   without motion flag 4.
//!
//! A rename is an entity replaced by another, as
//! `GuiInterface.GetRenamedEntities` lists them (promotion, a finished
//! foundation, an upgrade, packing); the new one spawns in the same delta.
//!
//! An economy is a player's top panel numbers: {zigzag population, zigzag
//! population limit, zigzag population max, varint m, m x {varint resource
//! code, varint amount rounded down, varint gatherers} sorted by code,
//! varint phase, varint k, k x zigzag panel entity id minus the one before
//! (from 0), in the order of `panelEntities`}. The code and phase are
//! 1-based indices into the string table, 0 for none.
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
//! one)}, then varint ceasefire end in simulation ms (0: none); index 0 is
//! gaia.
//! A territory is varint width, varint height (both 0: none yet), then
//! runs of {varint length, u8 tile} covering width * height tiles,
//! row-major, x fastest. A tile is 8 m square, as `ICmpTerritoryManager`'s
//! grid: owner in 0x1F, connected 0x20, blinking 0x40.
//! A template table is varint count, count x {varint length, UTF-8 name,
//! u8 1 if details follow else 0, then the details: varint flags, varint
//! overlay texture, varint overlay mask, f32 overlay line thickness, f32
//! overlay shape width, f32 overlay shape depth, f32 footprint width, f32
//! footprint depth, f32 footprint height, f32 status bar width, f32 status
//! bar height, f32 status bar height offset, varint generic name, varint
//! specific name, varint icon, varint selection group name, varint rank,
//! varint classes, varint visible classes, varint select sound group,
//! varint lang, varint phenotypes, varint components, varint rally line
//! texture, varint rally line mask, f32 rally line thickness, u8 rally line
//! start cap, u8 rally line end cap}. The flags and
//! component bits are those of the hook's template details (`SpeculaFeed.h`, kind 11), and so are the
//! meanings of the strings, which are 1-based indices into the string
//! table, 0 for none. A template the hook had no details for has none; the
//! details arrive in the same message as the template's name, so before
//! any entity uses it.
//! A string table is varint count, count x {varint length, UTF-8 text}.
//! Template names are as the simulation knows them, e.g.
//! `units/athen/infantry_spearman_b` or `foundation|structures/athen/house`;
//! a foundation's rising building is an entity of its own with a
//! `construction|` template, and corpses (`corpse|`) and what a dying
//! entity spawns (`decay|`, rubble) are entities too. Strings are actor file names, animation names,
//! variant keys and selections, sound group paths and the template
//! details' strings.
//! A variant set table is varint count, count x {varint n, n x {varint key,
//! varint selection}}, 0-based string indices, sorted by key. The set holds
//! the VisualActor's variant selections except the `animation` key, which
//! always holds the animation name.
//! A variable set table is varint count, count x {varint n, n x {varint
//! name, f32 value}}, 0-based string indices, sorted by name: the entity
//! variables the simulation gave `ICmpVisual::SetVariable` (Foundation.js
//! sets "numbuilders"), which particle emitters read.
//! A range set table is varint count, count x {varint n, n x {u8 kind (0
//! attack, 1 aura, 2 heal), f32 radius, varint texture, varint mask, f32
//! thickness}}: the circles the game draws round a selected entity, with
//! the texture and mask as 1-based string indices (0 for none), relative to
//! art/textures/selection/. The server numbers the sets itself, in the
//! order it first sees them.
//! Tables only grow during a match, so an index once sent stays valid.
//! Ids in each list are sorted and delta-coded from 0.
//!
//! The match's history is split into segments. Segment n holds the turns
//! from `first turn + n * turns per segment` on; after a gap in the turns
//! it starts at the first turn it has. It is a keyframe of its first turn
//! T and a frame: the delta into T from the turn before (absent in segment
//! 0), so playback can cross into it without a keyframe, then the deltas
//! of the later turns. A frame is one brotli stream of {varint length,
//! message} entries, flushed after each turn, so every turn's bytes can be
//! sent on their own; a keyframe is a brotli stream of its own.
//!
//! A segment is served at `/match/<match id as 16 hex digits>/seg/<n>` as
//! varint n, n bytes of the compressed keyframe, then the frame. Every
//! segment but the last is immutable and its frame finished; the last
//! one's frame is what has been flushed so far, a stream that needs more
//! input, until the match ends. Each segment is written to the history
//! (`history.rs`) as it closes and then read from there, so every match it
//! keeps stays served, with its terrain and data files; with the history
//! off, only the current and the previous match's, from memory.
//! `/match/<id>/info` is "<segments> <live>": the segment count, the open
//! one included, and 1 if it is the match being shown, else 0.
//!
//! Each WebSocket message is u8 envelope, then by envelope:
//! - 1 join: varint n, n bytes of a compressed keyframe, then the bytes of
//!   the frame that follows it so far (possibly none). A decoder of a new
//!   frame takes them; deltas in it not after the keyframe's turn are
//!   already in the keyframe. A new viewer gets the open segment this way,
//!   and every viewer gets a new match's first turn.
//! - 2 frame: the bytes one turn flushed, starting a new frame.
//! - 3 more: the bytes one turn flushed, continuing the frame.
//! - 4 ended: an ended message, uncompressed.
//! - 5 starts: varint ms until the first turn is shown. Sent while the
//!   server has turns but holds them all back (the delay), so a viewer
//!   that has nothing to show yet can tell when it will.
//!
//! So the compression is done once per match, not per viewer, and a
//! viewer joins at the open segment's keyframe (init.md 4).
//!
//! The encoding runs as the feed arrives, but each turn's output, and a
//! match's end, reaches the WebSockets and the segments only the delay
//! later, so nothing served is newer than that.
//!
//! The detail stream (`detail.rs`) is cut into segments on the same turns,
//! served at `/match/<match id>/detail/<n>`, and sent over its own
//! WebSocket with the same envelopes 1 to 3; a viewer joining it between
//! matches gets the ended match's final detail keyframe.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::io;
use std::io::ErrorKind;
use std::io::Write;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use rocket::tokio::sync::broadcast;
use specula_assets::wire::WIRE_VERSION;

use crate::detail::DetailEncoder;
use crate::feed::Details;
use crate::feed::Economy;
use crate::feed::Entity;
use crate::feed::Icon;
use crate::feed::Launch;
use crate::feed::Players;
use crate::feed::Sound;
use crate::feed::SoundTarget;
use crate::feed::TemplateDetails;
use crate::feed::Territory;
use crate::history::History;
use crate::terrain::Ground;
use crate::web;
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
const CHANGED_VARIABLES: u64 = 32768;
const CHANGED_HITPOINTS: u64 = 65536;
const CHANGED_CAPTURE: u64 = 131072;
const CHANGED_RANGES: u64 = 262144;
const MOTION_GROUNDED: u8 = 1;
const MOTION_HITPOINTS: u8 = 2;
const MOTION_CAPTURE: u8 = 4;
/// The hook's template component bit for Health.
const COMPONENT_HEALTH: u32 = 1;
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
/// 150 turns of 200 ms: a keyframe every 30 s of game time. Longer
/// frames compress better, and a viewer joining replays at most this many
/// deltas (`DATA_OPTIMIZATION.md`).
const SEGMENT_TURNS: u32 = 150;
/// Past 5 the ratio barely improves for several times the CPU
/// (`DATA_OPTIMIZATION.md` 12).
const BROTLI_QUALITY: u32 = 5;
const BROTLI_WINDOW_BITS: u32 = 22;
const ENVELOPE_JOIN: u8 = 1;
const ENVELOPE_FRAME: u8 = 2;
const ENVELOPE_MORE: u8 = 3;
const ENVELOPE_ENDED: u8 = 4;
const ENVELOPE_STARTS: u8 = 5;

pub type Message = Arc<[u8]>;

/// (key, selection) string indices, sorted by key.
pub type VariantSet = Vec<(u32, u32)>;

/// (name string index, value), sorted by name.
pub type VariableSet = Vec<(u32, f32)>;

/// One range circle; the radius and thickness are f32 bits so that sets can
/// be compared and hashed.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RangeCircle {
    kind: u8,
    radius: u32,
    /// 1-based string index, 0 for none.
    texture: u32,
    mask: u32,
    thickness: u32,
}

pub type RangeSet = Vec<RangeCircle>;

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
    variables: u32,
    /// 1-based index into the range set table, 0 for none.
    ranges: u32,
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
    /// 0 without Health, else the hitpoints in 1/255 of the max, rounded
    /// up and at least 1, so that a scratch shows and a living entity
    /// never reads as having none.
    hitpoints: u8,
    /// What a viewer moves it by each turn; only in what viewers hold,
    /// zero in the simulation's state.
    step: [i32; 2],
    /// Its height is the ground's, likewise.
    grounded: bool,
}

impl Dot {
    fn new(
        entity: &Entity,
        icon: u32,
        player_color: bool,
        turn: u32,
        hitpoints: u8,
        ranges: u32,
    ) -> Self {
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
            variables: one_based(visual.and_then(|visual| visual.variables)),
            ranges,
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
            hitpoints,
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

fn quantize_hitpoints(health: Option<(f32, f32)>) -> u8 {
    let Some((hitpoints, max)) = health else {
        return 0;
    };
    let fraction = if max > 0.0 { hitpoints / max } else { 1.0 };
    (fraction * 255.0).ceil().clamp(1.0, 255.0) as u8
}

/// Each player's share of the max capture points, in 1/255; empty without
/// Capturable.
fn quantize_capture(details: &Details, id: u32) -> Vec<u8> {
    let Some(capture) = details
        .entities
        .get(&id)
        .and_then(|groups| groups.capture.as_ref())
        .filter(|capture| capture.max > 0.0)
    else {
        return Vec::new();
    };
    capture
        .points
        .iter()
        .map(|points| (points / capture.max * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// A player's top panel numbers as viewers are sent them.
#[derive(Clone, PartialEq, Eq)]
struct Funds {
    population: i32,
    population_limit: i32,
    population_max: i32,
    /// (1-based code string, whole amount, gatherers), sorted by code.
    resources: Vec<(u32, u32, u32)>,
    /// 1-based string index, 0 for none.
    phase: u32,
    panel_entities: Vec<u32>,
}

impl Funds {
    /// The top panel shows whole amounts, so the fraction is never sent.
    fn new(economy: &Economy) -> Self {
        let one_based = |index: Option<u32>| index.map_or(0, |index| index + 1);
        Funds {
            population: economy.population,
            population_limit: economy.population_limit,
            population_max: economy.population_max,
            resources: economy
                .resources
                .iter()
                .map(|resource| {
                    (
                        one_based(resource.code),
                        resource.amount.floor().max(0.0) as u32,
                        u32::from(resource.gatherers),
                    )
                })
                .collect(),
            phase: one_based(economy.phase),
            panel_entities: economy.panel_entities.clone(),
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

/// A template as the template table sends it.
struct Template {
    name: String,
    /// `None` when the hook had no template node to read them from.
    details: Option<TemplateDetails>,
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
    templates: &'a [Template],
    strings: &'a [String],
    variant_sets: &'a [VariantSet],
    variable_sets: &'a [VariableSet],
    range_sets: &'a [RangeSet],
}

pub struct Hub {
    state: Mutex<State>,
    /// Wakes the publisher when something is queued.
    queued: Condvar,
    /// None when storage is off. Locked after `state` when both are, never
    /// before it; requests read its files with neither held.
    history: Option<Arc<Mutex<History>>>,
}

/// A terrain resource as served, and its brotli copy once made.
#[derive(Clone)]
pub struct TerrainFile {
    pub raw: Message,
    pub brotli: Arc<OnceLock<Message>>,
}

/// A finished match's history, kept for its viewers once the next one
/// starts.
struct Archive {
    match_id: u64,
    segments: Vec<Option<Message>>,
    detail_segments: Vec<Option<Message>>,
    terrain: Option<(u64, TerrainFile)>,
    data_files: DataFiles,
}

#[derive(Clone)]
struct Ended {
    at: Instant,
    /// The final state, compressed, for viewers who join before the next
    /// match: the tables a keyframe would be built from belong to the next
    /// feed by then.
    keyframe: Message,
    detail_keyframe: Message,
}

type Frame = brotli::CompressorWriter<Vec<u8>>;
/// The kind 18 and 19 JSON of a match, by the hash the detail stream names
/// it by.
type DataFiles = HashMap<u64, Arc<[u8]>>;

/// One turn of a stream as the encoder made it, compressed, waiting to be
/// shown: the encoding runs live while viewers see it `delay` later.
#[derive(Clone)]
enum TrackOp {
    /// A new match's history starts at this compressed keyframe.
    Restart(Message),
    /// A turn's delta. `next` closes the open segment, with the end of its
    /// frame, and opens one at a compressed keyframe; `bytes` are what the
    /// turn flushed into the frame.
    Push {
        next: Option<(Vec<u8>, Message)>,
        bytes: Vec<u8>,
    },
}

/// The live side of a stream: the open segment's brotli frame.
struct Compressor {
    frame: Option<Frame>,
    /// Segments closed in this match, which places the next keyframe.
    closed: u32,
}

impl Compressor {
    fn new() -> Self {
        Compressor {
            frame: None,
            closed: 0,
        }
    }

    fn restart(&mut self, keyframe: &[u8]) -> TrackOp {
        self.frame = None;
        self.closed = 0;
        TrackOp::Restart(compress(keyframe).into())
    }

    /// Adds a turn's delta, after starting a new segment at `keyframe` if
    /// there is one.
    fn push(&mut self, delta: &[u8], keyframe: Option<&[u8]>) -> io::Result<TrackOp> {
        let next = keyframe.map(|keyframe| {
            self.closed += 1;
            (self.finish(), compress(keyframe).into())
        });
        let frame = self.frame.get_or_insert_with(|| {
            Frame::new(Vec::new(), 4096, BROTLI_QUALITY, BROTLI_WINDOW_BITS)
        });
        let before = frame.get_ref().len();
        let mut entry = Vec::with_capacity(delta.len() + 5);
        append_framed(&mut entry, delta);
        frame.write_all(&entry)?;
        frame.flush()?;
        let bytes = frame.get_ref()[before..].to_vec();
        Ok(TrackOp::Push { next, bytes })
    }

    /// Ends the open segment's frame; returns the bytes that finishing it
    /// added after the last flush.
    fn finish(&mut self) -> Vec<u8> {
        let Some(frame) = self.frame.take() else {
            return Vec::new();
        };
        let flushed = frame.get_ref().len();
        let mut bytes = frame.into_inner();
        bytes.drain(..flushed);
        bytes
    }
}

/// One stream's segments and live subscribers as viewers see them: the
/// visual stream, and the detail stream (`detail.rs`), which is cut on the
/// same turns.
struct Track {
    /// Closed segments, as served; `None` once on disk, so a long match
    /// does not grow in memory.
    segments: Vec<Option<Message>>,
    /// The open segment, index `segments.len()`: its compressed keyframe,
    /// and its frame once a delta has gone into it. The frame's output is
    /// kept whole, since it is the segment's body and what a joining viewer
    /// is sent.
    open_keyframe: Message,
    frame: Option<Vec<u8>>,
    sender: broadcast::Sender<Message>,
}

impl Track {
    fn new() -> Self {
        Track {
            segments: Vec::new(),
            open_keyframe: Arc::new([]),
            frame: None,
            sender: broadcast::channel(BACKLOG_MESSAGES).0,
        }
    }

    /// Returns the message every viewer is sent. `store` writes a closed
    /// segment to disk and tells whether it did.
    fn apply(&mut self, op: TrackOp, store: impl FnOnce(usize, &[u8]) -> bool) -> Message {
        match op {
            TrackOp::Restart(keyframe) => {
                self.segments.clear();
                self.frame = None;
                self.open_keyframe = keyframe;
                envelope_join(&self.open_keyframe, &[]).into()
            }
            TrackOp::Push { next, bytes } => {
                if let Some((tail, keyframe)) = next {
                    self.close(&tail, store);
                    self.open_keyframe = keyframe;
                }
                let envelope = if self.frame.is_some() {
                    ENVELOPE_MORE
                } else {
                    ENVELOPE_FRAME
                };
                self.frame.get_or_insert_default().extend_from_slice(&bytes);
                let mut message = vec![envelope];
                message.extend_from_slice(&bytes);
                message.into()
            }
        }
    }

    /// Keeps the open segment, its frame ended by `tail`: on disk if
    /// `store` manages, else in memory.
    fn close(&mut self, tail: &[u8], store: impl FnOnce(usize, &[u8]) -> bool) {
        let mut segment = Vec::new();
        append_framed(&mut segment, &self.open_keyframe);
        if let Some(frame) = self.frame.take() {
            segment.extend_from_slice(&frame);
        }
        segment.extend_from_slice(tail);
        let stored = store(self.segments.len(), &segment);
        self.segments.push((!stored).then(|| segment.into()));
    }

    /// The open segment as served: its frame so far, unfinished.
    fn open_segment(&self) -> Vec<u8> {
        let mut segment = Vec::new();
        append_framed(&mut segment, &self.open_keyframe);
        if let Some(frame) = &self.frame {
            segment.extend_from_slice(frame);
        }
        segment
    }

    /// A segment of the current match, and whether it is closed.
    fn segment(&self, index: usize) -> Lookup {
        if let Some(closed) = self.segments.get(index) {
            return match closed {
                Some(closed) => Lookup::Found(Arc::clone(closed), true),
                None => Lookup::OnDisk,
            };
        }
        if index != self.segments.len() {
            return Lookup::Missing;
        }
        Lookup::Found(self.open_segment().into(), false)
    }

    /// The open segment's keyframe and its frame so far.
    fn join(&self) -> Message {
        let frame = self.frame.as_deref().unwrap_or_default();
        envelope_join(&self.open_keyframe, frame).into()
    }
}

/// Where a segment asked for is.
enum Lookup {
    /// In memory, and whether it is closed.
    Found(Message, bool),
    OnDisk,
    Missing,
}

/// What the encoder made, held until it is due.
#[derive(Clone)]
enum Release {
    Turn {
        /// The id of the match this turn starts.
        start: Option<u64>,
        visual: TrackOp,
        detail: TrackOp,
    },
    End {
        match_id: u64,
        visual_tail: Vec<u8>,
        detail_tail: Vec<u8>,
        ended: Ended,
        terrain: Option<(u64, TerrainFile)>,
        data_files: DataFiles,
    },
}

/// The stream as viewers see it, `delay` behind the encoder.
struct Shown {
    /// Names the shown match's segments, so a URL from an earlier match
    /// cannot return this one's.
    match_id: u64,
    has_turn: bool,
    visual: Track,
    detail: Track,
    /// Set from the end of a match until the next one's first turn.
    ended: Option<Ended>,
    previous: Option<Archive>,
}

impl Shown {
    /// `encoding`: the match the encoder is on, which the history must not
    /// delete to make room.
    fn apply(&mut self, release: Release, history: Option<&Mutex<History>>, encoding: u64) {
        let protect = [
            encoding,
            self.previous.as_ref().map_or(0, |archive| archive.match_id),
        ];
        let store = |match_id: u64, detail: bool| {
            move |index: usize, bytes: &[u8]| {
                let Some(history) = history else {
                    return false;
                };
                let written = history
                    .lock()
                    .unwrap()
                    .write_segment(match_id, detail, index, bytes, &protect);
                if let Err(error) = &written {
                    eprintln!("cannot store segment {index} of match {match_id:016x}: {error}");
                }
                written.is_ok()
            }
        };
        match release {
            Release::Turn {
                start,
                visual,
                detail,
            } => {
                // The history restarts only now, so until the new match's
                // first turn, viewers and segment requests still get the
                // old one.
                if let Some(match_id) = start {
                    self.match_id = match_id;
                    self.ended = None;
                }
                self.has_turn = true;
                let visual = self.visual.apply(visual, store(self.match_id, false));
                let detail = self.detail.apply(detail, store(self.match_id, true));
                // No receivers is not an error: nobody is watching yet.
                let _ = self.visual.sender.send(visual);
                let _ = self.detail.sender.send(detail);
            }
            Release::End {
                match_id,
                visual_tail,
                detail_tail,
                mut ended,
                terrain,
                data_files,
            } => {
                self.visual.close(&visual_tail, store(match_id, false));
                self.detail.close(&detail_tail, store(match_id, true));
                self.previous = Some(Archive {
                    match_id,
                    segments: std::mem::take(&mut self.visual.segments),
                    detail_segments: std::mem::take(&mut self.detail.segments),
                    terrain,
                    data_files,
                });
                // Viewers learn of the end only now.
                ended.at = Instant::now();
                self.ended = Some(ended);
                let _ = self
                    .visual
                    .sender
                    .send(envelope_ended(&encode_ended(match_id, 0)).into());
            }
        }
    }
}

struct State {
    turn: u32,
    map_size: u32,
    /// The sim rate the match was set up with.
    game_speed: f32,
    /// The match being encoded, which viewers may not see yet.
    match_id: u64,
    /// The turn segment 0 starts at.
    first_turn: u32,
    /// How far behind the game viewers are held, so that the stream cannot
    /// be used to scout the opponent.
    delay: Duration,
    visual: Compressor,
    detail: Compressor,
    /// Oldest first, each with when it is due.
    pending: VecDeque<(Instant, Release)>,
    shown: Shown,
    /// In play mode, the stream as the player sees it: every release at
    /// once, never stored, while `shown` keeps the delay for observers.
    live: Option<Shown>,
    detail_encoder: DetailEncoder,
    terrain: Option<(u64, TerrainFile)>,
    /// The kind 18 and 19 JSON of the match, by the hash the detail
    /// stream names it by.
    data_files: DataFiles,
    ground: Option<Ground>,
    /// What viewers hold after the last turn, which the next delta builds
    /// on; near the simulation's state, not equal to it.
    dots: BTreeMap<u32, Dot>,
    /// Capture shares by id, beside `dots` so that a dot stays `Copy`; an
    /// entity without any has no entry.
    captures: BTreeMap<u32, Vec<u8>>,
    /// By player index: the latest, and what viewers hold.
    economy: Vec<Funds>,
    economy_shown: Vec<Funds>,
    icons: Table<Icon>,
    templates: Table<Template>,
    strings: Table<String>,
    variant_sets: Table<VariantSet>,
    variable_sets: Table<VariableSet>,
    range_sets: Table<RangeSet>,
    /// Index in `range_sets`, for finding a set again.
    range_set_ids: HashMap<RangeSet, u32>,
    /// Template index to 1-based icon index.
    template_icons: HashMap<u32, u32>,
    /// Templates whose minimap colour is the owner's.
    player_color_templates: HashSet<u32>,
    players: Players,
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
    ended: bool,
    /// When the feed last connected or sent a turn, for the observer
    /// watchdog.
    feed_activity: Option<Instant>,
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
            variable_sets: sent(&self.variable_sets, with_unsent),
            range_sets: sent(&self.range_sets, with_unsent),
        }
    }
}

impl Shown {
    fn new() -> Self {
        Shown {
            match_id: 0,
            has_turn: false,
            visual: Track::new(),
            detail: Track::new(),
            ended: None,
            previous: None,
        }
    }
}

impl Hub {
    /// `live`: also keep the undelayed stream `/play` reads.
    pub fn new(delay: Duration, history: Option<History>, live: bool) -> Self {
        Hub {
            queued: Condvar::new(),
            history: history.map(|history| Arc::new(Mutex::new(history))),
            state: Mutex::new(State {
                turn: 0,
                map_size: 0,
                game_speed: 1.0,
                match_id: 0,
                first_turn: 0,
                delay,
                visual: Compressor::new(),
                detail: Compressor::new(),
                pending: VecDeque::new(),
                shown: Shown::new(),
                live: live.then(Shown::new),
                detail_encoder: DetailEncoder::new(),
                terrain: None,
                data_files: HashMap::new(),
                ground: None,
                dots: BTreeMap::new(),
                captures: BTreeMap::new(),
                economy: Vec::new(),
                economy_shown: Vec::new(),
                icons: Table::new(),
                templates: Table::new(),
                strings: Table::new(),
                variant_sets: Table::new(),
                variable_sets: Table::new(),
                range_sets: Table::new(),
                range_set_ids: HashMap::new(),
                template_icons: HashMap::new(),
                player_color_templates: HashSet::new(),
                players: Players::default(),
                players_changed: false,
                territory: None,
                territory_changed: false,
                sounds: Vec::new(),
                launches: Vec::new(),
                removals: Vec::new(),
                shots: BTreeMap::new(),
                has_turn: false,
                new_feed: true,
                ended: false,
                feed_activity: None,
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
        state.variable_sets.clear();
        state.range_sets.clear();
        state.range_set_ids.clear();
        state.template_icons.clear();
        state.player_color_templates.clear();
        state.players = Players::default();
        state.territory = None;
        state.economy.clear();
        state.sounds.clear();
        state.launches.clear();
        state.removals.clear();
    }

    pub fn add_template(&self, index: u32, name: String) -> io::Result<()> {
        self.state.lock().unwrap().templates.insert(
            index,
            Template {
                name,
                details: None,
            },
            "template",
        )
    }

    /// The hook sends them right after the template, before any turn uses
    /// it, so the entry is still unsent.
    pub fn set_template_details(&self, index: u32, details: TemplateDetails) -> io::Result<()> {
        let state = &mut *self.state.lock().unwrap();
        let template = state
            .templates
            .items
            .get_mut(index as usize)
            .filter(|_| index as usize >= state.templates.sent)
            .ok_or_else(|| {
                io::Error::new(
                    ErrorKind::InvalidData,
                    format!("details of template {index}, not the last one sent"),
                )
            })?;
        template.details = Some(details);
        Ok(())
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

    pub fn add_variable_set(&self, index: u32, pairs: VariableSet) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .variable_sets
            .insert(index, pairs, "variable set")
    }

    pub fn set_template_player_color(&self, template: u32) {
        self.state
            .lock()
            .unwrap()
            .player_color_templates
            .insert(template);
    }

    pub fn set_players(&self, players: Players) {
        let mut state = self.state.lock().unwrap();
        if state.players != players {
            state.players = players;
            state.players_changed = true;
        }
    }

    pub fn set_economy(&self, economy: &[Economy]) {
        self.state.lock().unwrap().economy = economy.iter().map(Funds::new).collect();
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
        let file = TerrainFile {
            raw: bytes.into(),
            brotli: Arc::new(OnceLock::new()),
        };
        // The highest quality takes seconds on a large map, which the feed
        // thread must not wait for.
        let (raw, brotli) = (Arc::clone(&file.raw), Arc::clone(&file.brotli));
        std::thread::spawn(move || {
            let _ = brotli.set(web::compress(&raw).into());
        });
        state.terrain = Some((id, file));
        state.ground = Some(ground);
    }

    /// Keeps a kind 18 or 19 JSON for `/data/<hash>`, and returns that hash.
    pub fn add_data(&self, json: Vec<u8>) -> u64 {
        let mut hasher = DefaultHasher::new();
        json.hash(&mut hasher);
        let hash = hasher.finish();
        let state = &mut *self.state.lock().unwrap();
        if state.data_files.contains_key(&hash) {
            return hash;
        }
        // Before the match's first turn it has no id yet; then the files
        // are stored with the terrain.
        if state.has_turn && !state.new_feed && !state.ended {
            let match_id = state.match_id;
            self.store(state, |history, protect| {
                history.write_data(match_id, hash, &json, protect)
            });
        }
        state.data_files.insert(hash, json.into());
        hash
    }

    /// The current and the previous match's files and those of a match
    /// whose end viewers have not reached are in memory; older ones are
    /// read from the history, while it keeps them.
    pub fn data(&self, hash: u64) -> Option<Arc<[u8]>> {
        {
            let state = self.state.lock().unwrap();
            let archived = state
                .shown
                .previous
                .as_ref()
                .and_then(|archive| archive.data_files.get(&hash));
            let ending = ending(&state.pending)
                .find_map(|(_, data_files)| data_files.get(&hash))
                .or(archived);
            if let Some(file) = state.data_files.get(&hash).or(ending) {
                return Some(Arc::clone(file));
            }
        }
        let path = self.history.as_ref()?.lock().unwrap().data_path(hash)?;
        Some(std::fs::read(path).ok()?.into())
    }

    /// As `data`, for terrain.
    pub fn terrain(&self, id: u64) -> Option<TerrainFile> {
        {
            let state = self.state.lock().unwrap();
            let archived = state
                .shown
                .previous
                .as_ref()
                .and_then(|archive| archive.terrain.as_ref());
            let known = [state.terrain.as_ref(), archived]
                .into_iter()
                .chain(ending(&state.pending).map(|(terrain, _)| terrain))
                .flatten()
                .find(|(known, _)| *known == id);
            if let Some((_, file)) = known {
                return Some(file.clone());
            }
        }
        let (raw, brotli) = self.history.as_ref()?.lock().unwrap().terrain_paths(id)?;
        let file = TerrainFile {
            raw: std::fs::read(raw).ok()?.into(),
            brotli: Arc::new(OnceLock::new()),
        };
        if let Ok(bytes) = std::fs::read(brotli) {
            let _ = file.brotli.set(bytes.into());
        }
        Some(file)
    }

    /// Writes a file of the match being encoded to the history; a failure
    /// costs only the stored copy, so it is logged and otherwise ignored.
    fn store(&self, state: &State, write: impl FnOnce(&mut History, &[u64]) -> io::Result<()>) {
        let Some(history) = &self.history else {
            return;
        };
        let protect = [
            state.shown.match_id,
            state
                .shown
                .previous
                .as_ref()
                .map_or(0, |archive| archive.match_id),
        ];
        if let Err(error) = write(&mut history.lock().unwrap(), &protect) {
            eprintln!(
                "cannot store a file of match {:016x}: {error}",
                state.match_id
            );
        }
    }

    /// The terrain and the files gathered before the first turn of the
    /// match just started. They are served before the delay anyway, so
    /// storing them now shows nothing early.
    fn store_match_files(&self, state: &State) {
        let Some(history) = &self.history else {
            return;
        };
        let match_id = state.match_id;
        if let Some((id, file)) = &state.terrain {
            let id = *id;
            self.store(state, |history, protect| {
                history.write_terrain(match_id, id, &file.raw, protect)
            });
            // The brotli copy takes seconds on a large map.
            let (history, brotli) = (Arc::clone(history), Arc::clone(&file.brotli));
            std::thread::spawn(move || {
                let bytes = brotli.wait();
                if let Err(error) =
                    history
                        .lock()
                        .unwrap()
                        .write_terrain_brotli(match_id, id, bytes, &[])
                {
                    eprintln!("cannot store the brotli terrain of match {match_id:016x}: {error}");
                }
            });
        }
        for (hash, json) in &state.data_files {
            self.store(state, |history, protect| {
                history.write_data(match_id, *hash, json, protect)
            });
        }
    }

    pub fn feed_activity(&self) -> Option<Instant> {
        self.state.lock().unwrap().feed_activity
    }

    /// Called when the feed is gone. A feed that never sent a turn ends
    /// nothing.
    pub fn end_match(&self) {
        let state = &mut *self.state.lock().unwrap();
        if !state.has_turn || state.ended {
            return;
        }
        let keyframe: Message = compress(&encode_keyframe(
            state,
            state.turn,
            &state.tables(true),
            &state.dots,
            &state.shots,
        ))
        .into();
        let detail_keyframe: Message = compress(&state.detail_encoder.keyframe(
            state.turn,
            state.match_id,
            |id| state.dots.get(&id).map(|dot| dot.template),
        ))
        .into();
        let release = Release::End {
            match_id: state.match_id,
            visual_tail: state.visual.finish(),
            detail_tail: state.detail.finish(),
            ended: Ended {
                at: Instant::now(),
                keyframe,
                detail_keyframe,
            },
            terrain: state.terrain.clone(),
            data_files: std::mem::take(&mut state.data_files),
        };
        state.ended = true;
        println!("match {:016x} ended at turn {}", state.match_id, state.turn);
        self.queue(state, release);
    }

    fn queue(&self, state: &mut State, release: Release) {
        if !state.shown.has_turn
            && matches!(release, Release::Turn { .. })
            && first_turn_due(&state.pending).is_none()
        {
            let _ = state.shown.visual.sender.send(envelope_starts(state.delay));
        }
        let encoding = state.match_id;
        if let Some(live) = &mut state.live {
            live.apply(release.clone(), None, encoding);
        }
        state
            .pending
            .push_back((Instant::now() + state.delay, release));
        self.queued.notify_one();
    }

    /// Shows viewers what the encoder made once it is due; runs for good.
    pub fn run_publisher(&self) {
        let mut state = self.state.lock().unwrap();
        loop {
            let now = Instant::now();
            while let Some((due, _)) = state.pending.front()
                && *due <= now
            {
                let (_, release) = state.pending.pop_front().unwrap();
                let encoding = state.match_id;
                state
                    .shown
                    .apply(release, self.history.as_deref(), encoding);
            }
            state = match state.pending.front() {
                Some((due, _)) => {
                    let wait = due.saturating_duration_since(now);
                    self.queued.wait_timeout(state, wait).unwrap().0
                }
                None => self.queued.wait(state).unwrap(),
            };
        }
    }

    pub fn set_map(&self, size: u32, game_speed: f32) {
        let mut state = self.state.lock().unwrap();
        state.map_size = size;
        state.game_speed = game_speed;
    }

    /// `details` is the feed's state as of this turn.
    pub fn publish(&self, turn: u32, entities: &[Entity], details: &Details) -> io::Result<()> {
        let state = &mut *self.state.lock().unwrap();
        let ranges: HashMap<u32, u32> = entities
            .iter()
            .filter_map(|entity| {
                let overlays = details.entities.get(&entity.id)?.ranges.as_ref()?;
                if overlays.is_empty() {
                    return None;
                }
                let one_based = |index: Option<u32>| index.map_or(0, |index| index + 1);
                let set: RangeSet = overlays
                    .iter()
                    .map(|overlay| RangeCircle {
                        kind: overlay.kind,
                        radius: overlay.radius.to_bits(),
                        texture: one_based(overlay.texture),
                        mask: one_based(overlay.mask),
                        thickness: overlay.thickness.to_bits(),
                    })
                    .collect();
                let next = state.range_sets.items.len() as u32 + 1;
                let index = *state.range_set_ids.entry(set.clone()).or_insert(next);
                if index == next {
                    state.range_sets.items.push(set);
                }
                Some((entity.id, index))
            })
            .collect();
        let dots: BTreeMap<u32, Dot> = entities
            .iter()
            .map(|entity| {
                let icon = state
                    .template_icons
                    .get(&entity.template)
                    .copied()
                    .unwrap_or(0);
                let player_color = state.player_color_templates.contains(&entity.template);
                let health = details
                    .entities
                    .get(&entity.id)
                    .and_then(|groups| groups.health);
                let ranges = ranges.get(&entity.id).copied().unwrap_or(0);
                let mut dot = Dot::new(
                    entity,
                    icon,
                    player_color,
                    turn,
                    quantize_hitpoints(health),
                    ranges,
                );
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
        let captures: BTreeMap<u32, Vec<u8>> = entities
            .iter()
            .map(|entity| (entity.id, quantize_capture(details, entity.id)))
            .filter(|(_, shares)| !shares.is_empty())
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
        let (release, shown): (Release, BTreeMap<u32, Dot>) = if state.new_feed {
            state.match_id = new_match_id(state.match_id);
            state.ended = false;
            state.first_turn = turn;
            let ground = state.ground.as_ref();
            let shown: BTreeMap<u32, Dot> = dots
                .iter()
                .map(|(id, dot)| (*id, arrive(ground, dot)))
                .collect();
            state.captures = captures;
            let keyframe = encode_keyframe(state, turn, &state.tables(true), &shown, &shots);
            let visual = state.visual.restart(&keyframe);
            // The keyframe holds what the turn's delta would bring.
            state.detail_encoder.reset();
            state
                .detail_encoder
                .delta(turn, details, |id| shown.contains_key(&id));
            let detail_keyframe = state.detail_encoder.keyframe(turn, state.match_id, |id| {
                shown.get(&id).map(|dot| dot.template)
            });
            let detail = state.detail.restart(&detail_keyframe);
            self.store_match_files(state);
            let release = Release::Turn {
                start: Some(state.match_id),
                visual,
                detail,
            };
            (release, shown)
        } else {
            let (delta, shown) = encode_delta(
                turn,
                &Tables {
                    icons: state.icons.unsent(),
                    templates: state.templates.unsent(),
                    strings: state.strings.unsent(),
                    variant_sets: state.variant_sets.unsent(),
                    variable_sets: state.variable_sets.unsent(),
                    range_sets: state.range_sets.unsent(),
                },
                state.players_changed.then_some(&state.players),
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
                &Status {
                    templates: &state.templates.items,
                    previous_captures: &state.captures,
                    captures: &captures,
                    economy: &state.economy,
                    economy_shown: &state.economy_shown,
                    renames: &details.renames,
                },
            );
            state.captures = captures;
            let detail_delta = state
                .detail_encoder
                .delta(turn, details, |id| dots.contains_key(&id));
            let next_start = state.first_turn + SEGMENT_TURNS * (state.visual.closed + 1);
            let (keyframe, detail_keyframe) = if turn >= next_start {
                (
                    Some(encode_keyframe(
                        state,
                        turn,
                        &state.tables(true),
                        &shown,
                        &shots,
                    )),
                    Some(state.detail_encoder.keyframe(turn, state.match_id, |id| {
                        shown.get(&id).map(|dot| dot.template)
                    })),
                )
            } else {
                (None, None)
            };
            let visual = state.visual.push(&delta, keyframe.as_deref())?;
            let detail = state
                .detail
                .push(&detail_delta, detail_keyframe.as_deref())?;
            let release = Release::Turn {
                start: None,
                visual,
                detail,
            };
            (release, shown)
        };
        state.icons.sent = state.icons.items.len();
        state.templates.sent = state.templates.items.len();
        state.strings.sent = state.strings.items.len();
        state.variant_sets.sent = state.variant_sets.items.len();
        state.variable_sets.sent = state.variable_sets.items.len();
        state.range_sets.sent = state.range_sets.items.len();
        state.players_changed = false;
        state.territory_changed = false;
        state.economy_shown = state.economy.clone();
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
        self.queue(state, release);
        Ok(())
    }

    /// A segment of the current or the previous match, and whether it is
    /// closed.
    /// `live`: as `/play` sees the stream, else as `/` does.
    pub fn segment(&self, match_id: u64, index: usize, live: bool) -> Option<(Message, bool)> {
        self.track_segment(match_id, index, false, live)
    }

    /// A detail stream segment, as `segment`.
    pub fn detail_segment(
        &self,
        match_id: u64,
        index: usize,
        live: bool,
    ) -> Option<(Message, bool)> {
        self.track_segment(match_id, index, true, live)
    }

    fn track_segment(
        &self,
        match_id: u64,
        index: usize,
        detail: bool,
        live: bool,
    ) -> Option<(Message, bool)> {
        match self.memory_segment(match_id, index, detail, live) {
            Lookup::Found(bytes, closed) => return Some((bytes, closed)),
            Lookup::OnDisk => {}
            Lookup::Missing => return None,
        }
        let path = self
            .history
            .as_ref()?
            .lock()
            .unwrap()
            .segment_path(match_id, detail, index)?;
        // Only closed segments are stored. The match may have been deleted
        // since the lookup; then the file is gone, as it would be after.
        let bytes = std::fs::read(path).ok()?;
        Some((bytes.into(), true))
    }

    fn memory_segment(&self, match_id: u64, index: usize, detail: bool, live: bool) -> Lookup {
        let locked = self.state.lock().unwrap();
        let Some(state) = locked.view(live) else {
            return Lookup::Missing;
        };
        // An ended match is archived but keeps its id until the next one
        // starts, so the archive is asked first.
        if let Some(archive) = state
            .previous
            .as_ref()
            .filter(|archive| archive.match_id == match_id)
        {
            let segments = if detail {
                &archive.detail_segments
            } else {
                &archive.segments
            };
            return match segments.get(index) {
                Some(Some(bytes)) => Lookup::Found(Arc::clone(bytes), true),
                Some(None) => Lookup::OnDisk,
                None => Lookup::Missing,
            };
        }
        if match_id != state.match_id || !state.has_turn {
            return Lookup::OnDisk;
        }
        if detail {
            state.detail.segment(index)
        } else {
            state.visual.segment(index)
        }
    }

    /// For a page opening a match by its id: how many segments it has
    /// (the open one included), and whether it is the match being shown,
    /// which is followed over the WebSocket rather than played from its
    /// segments alone.
    pub fn match_info(&self, match_id: u64) -> Option<(usize, bool)> {
        {
            let state = &self.state.lock().unwrap().shown;
            if let Some(archive) = state
                .previous
                .as_ref()
                .filter(|archive| archive.match_id == match_id)
            {
                return Some((archive.segments.len(), false));
            }
            if match_id == state.match_id && state.has_turn {
                return Some((state.visual.segments.len() + 1, true));
            }
        }
        let count = self
            .history
            .as_ref()?
            .lock()
            .unwrap()
            .segment_count(match_id)?;
        Some((count, false))
    }

    /// What a new viewer is sent first (nothing before the first turn) and
    /// the receiver are taken under one lock, so the first bytes a viewer
    /// receives continue exactly the frame it was sent.
    /// `live` as for `segment`; None when there is no live stream.
    pub fn subscribe(&self, live: bool) -> Option<(Vec<Message>, broadcast::Receiver<Message>)> {
        let locked = self.state.lock().unwrap();
        let state = locked.view(live)?;
        let first = if let Some(ended) = &state.ended {
            let since = ended.at.elapsed().as_millis() as u64;
            vec![
                envelope_join(&ended.keyframe, &[]).into(),
                envelope_ended(&encode_ended(state.match_id, since)).into(),
            ]
        } else if state.has_turn {
            vec![state.visual.join()]
        } else if let Some(due) = first_turn_due(&locked.pending).filter(|_| !live) {
            vec![envelope_starts(
                due.saturating_duration_since(Instant::now()),
            )]
        } else {
            Vec::new()
        };
        Some((first, state.visual.sender.subscribe()))
    }

    /// As `subscribe`, for the detail stream; an ended match's viewers get
    /// its final state.
    pub fn subscribe_detail(
        &self,
        live: bool,
    ) -> Option<(Vec<Message>, broadcast::Receiver<Message>)> {
        let locked = self.state.lock().unwrap();
        let state = locked.view(live)?;
        let first = if let Some(ended) = &state.ended {
            vec![envelope_join(&ended.detail_keyframe, &[]).into()]
        } else if state.has_turn {
            vec![state.detail.join()]
        } else {
            Vec::new()
        };
        Some((first, state.detail.sender.subscribe()))
    }
}

impl State {
    /// The stream `/play` (`live`) or `/` sees.
    fn view(&self, live: bool) -> Option<&Shown> {
        if live {
            self.live.as_ref()
        } else {
            Some(&self.shown)
        }
    }
}

/// When the first queued turn is shown.
fn first_turn_due(pending: &VecDeque<(Instant, Release)>) -> Option<Instant> {
    pending
        .iter()
        .find(|(_, release)| matches!(release, Release::Turn { .. }))
        .map(|(due, _)| *due)
}

/// The terrain and files of each queued match end.
fn ending(
    pending: &VecDeque<(Instant, Release)>,
) -> impl Iterator<Item = (Option<&(u64, TerrainFile)>, &DataFiles)> {
    pending.iter().filter_map(|(_, release)| match release {
        Release::End {
            terrain,
            data_files,
            ..
        } => Some((terrain.as_ref(), data_files)),
        Release::Turn { .. } => None,
    })
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

/// Keyframes are compressed on their own: a viewer starts at one.
fn compress(bytes: &[u8]) -> Vec<u8> {
    let mut writer = Frame::new(Vec::new(), 4096, BROTLI_QUALITY, BROTLI_WINDOW_BITS);
    // Writing to a Vec cannot fail.
    writer.write_all(bytes).unwrap();
    writer.into_inner()
}

fn envelope_join(keyframe: &[u8], frame: &[u8]) -> Vec<u8> {
    let mut out = Writer::default();
    out.u8(ENVELOPE_JOIN);
    append_framed(&mut out.0, keyframe);
    out.0.extend_from_slice(frame);
    out.0
}

fn envelope_starts(wait: Duration) -> Message {
    let mut out = Writer::default();
    out.u8(ENVELOPE_STARTS);
    out.varint(wait.as_millis() as u64);
    out.0.into()
}

fn envelope_ended(message: &[u8]) -> Vec<u8> {
    let mut out = vec![ENVELOPE_ENDED];
    out.extend_from_slice(message);
    out
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
    out.u8(WIRE_VERSION);
    out.u64(state.match_id);
    out.f32(state.game_speed);
    out.varint(state.delay.as_millis() as u64);
    out.varint(u64::from(state.first_turn));
    out.varint(u64::from(SEGMENT_TURNS));
    out.varint(u64::from(state.map_size));
    out.u64(terrain_id(state));
    write_icons(&mut out, tables.icons);
    write_players(&mut out, &state.players);
    write_territory(&mut out, state.territory.as_ref());
    write_templates(&mut out, tables.templates);
    write_strings(&mut out, tables.strings);
    write_variant_sets(&mut out, tables.variant_sets);
    write_variable_sets(&mut out, tables.variable_sets);
    write_range_sets(&mut out, tables.range_sets);
    out.varint(state.economy.len() as u64);
    for funds in &state.economy {
        write_funds(&mut out, funds);
    }
    out.varint(dots.len() as u64);
    let mut last_id = 0;
    for (id, dot) in dots {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        let hitpoints = default_hitpoints(&state.templates.items, dot.template);
        let capture = capture_of(&state.captures, *id);
        write_dot(&mut out, *id, turn, dot, hitpoints, capture);
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
    new_players: Option<&Players>,
    new_territory: Option<&Territory>,
    ground: Option<&Ground>,
    previous: &BTreeMap<u32, Dot>,
    current: &BTreeMap<u32, Dot>,
    sounds: &[Sound],
    launches: &[(u32, Shot)],
    removals: &[u32],
    status: &Status,
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
                let (mut change, dot) = reckon(ground, before, now);
                if capture_of(status.previous_captures, *id) != capture_of(status.captures, *id) {
                    change.mask |= CHANGED_CAPTURE;
                }
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
    write_templates(&mut out, new_tables.templates);
    write_strings(&mut out, new_tables.strings);
    write_variant_sets(&mut out, new_tables.variant_sets);
    write_variable_sets(&mut out, new_tables.variable_sets);
    write_range_sets(&mut out, new_tables.range_sets);
    let funds: Vec<(usize, &Funds)> = status
        .economy
        .iter()
        .enumerate()
        .filter(|(player, funds)| status.economy_shown.get(*player) != Some(*funds))
        .collect();
    if funds.is_empty() {
        out.u8(0);
    } else {
        out.u8(1);
        out.varint(funds.len() as u64);
        for (player, funds) in funds {
            out.varint(player as u64);
            write_funds(&mut out, funds);
        }
    }
    out.varint(spawned.len() as u64);
    out.varint(destroyed.len() as u64);
    out.varint(changed.len() as u64);
    let mut last_id = 0;
    for (id, dot) in &spawned {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        let hitpoints = default_hitpoints(status.templates, dot.template);
        let capture = capture_of(status.captures, *id);
        write_dot(&mut out, *id, turn, dot, hitpoints, capture);
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
        write_change(&mut out, *id, change, now, capture_of(status.captures, *id));
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
    let mut renames = status.renames.to_vec();
    renames.sort_unstable();
    out.varint(renames.len() as u64);
    let mut last_id = 0;
    for (old, new) in renames {
        out.varint(u64::from(old - last_id));
        last_id = old;
        out.zigzag(i64::from(new) - i64::from(old));
    }
    (out.0, shown)
}

/// What a delta sends besides the visuals.
struct Status<'a> {
    /// The whole table, for the hitpoints an entity defaults to.
    templates: &'a [Template],
    previous_captures: &'a BTreeMap<u32, Vec<u8>>,
    captures: &'a BTreeMap<u32, Vec<u8>>,
    economy: &'a [Funds],
    economy_shown: &'a [Funds],
    /// (old id, new id).
    renames: &'a [(u32, u32)],
}

/// What an entity sent without hitpoints has: full with Health, else none.
/// Most entities are at it, so keyframes leave it out.
fn default_hitpoints(templates: &[Template], template: u32) -> u8 {
    let health = templates
        .get(template as usize)
        .and_then(|template| template.details.as_ref())
        .is_some_and(|details| details.components & COMPONENT_HEALTH != 0);
    if health { 255 } else { 0 }
}

fn capture_of(captures: &BTreeMap<u32, Vec<u8>>, id: u32) -> &[u8] {
    captures.get(&id).map_or(&[], Vec::as_slice)
}

fn write_funds(out: &mut Writer, funds: &Funds) {
    out.zigzag(i64::from(funds.population));
    out.zigzag(i64::from(funds.population_limit));
    out.zigzag(i64::from(funds.population_max));
    out.varint(funds.resources.len() as u64);
    for (code, amount, gatherers) in &funds.resources {
        out.varint(u64::from(*code));
        out.varint(u64::from(*amount));
        out.varint(u64::from(*gatherers));
    }
    out.varint(u64::from(funds.phase));
    out.varint(funds.panel_entities.len() as u64);
    let mut last_id = 0;
    for id in &funds.panel_entities {
        out.zigzag(i64::from(*id) - i64::from(last_id));
        last_id = *id;
    }
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
        (CHANGED_VARIABLES, before.variables != now.variables),
        (CHANGED_RANGES, before.ranges != now.ranges),
        (CHANGED_HITPOINTS, before.hitpoints != now.hitpoints),
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

fn write_change(out: &mut Writer, id: u32, change: &Change, now: &Dot, capture: &[u8]) {
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
    if mask & CHANGED_VARIABLES != 0 {
        out.varint(u64::from(now.variables));
    }
    if mask & CHANGED_RANGES != 0 {
        out.varint(u64::from(now.ranges));
    }
    if mask & CHANGED_HITPOINTS != 0 {
        out.u8(now.hitpoints);
    }
    if mask & CHANGED_CAPTURE != 0 {
        write_capture(out, capture);
    }
}

fn write_capture(out: &mut Writer, capture: &[u8]) {
    out.u8(capture.len() as u8);
    out.0.extend_from_slice(capture);
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

fn write_players(out: &mut Writer, players: &Players) {
    out.varint(players.players.len() as u64);
    for player in &players.players {
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
    out.varint(u64::from(players.ceasefire_end));
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

fn write_templates(out: &mut Writer, templates: &[Template]) {
    out.varint(templates.len() as u64);
    for template in templates {
        out.varint(template.name.len() as u64);
        out.0.extend_from_slice(template.name.as_bytes());
        let Some(details) = &template.details else {
            out.u8(0);
            continue;
        };
        out.u8(1);
        let one_based = |index: Option<u32>| u64::from(index.map_or(0, |index| index + 1));
        out.varint(u64::from(details.flags));
        out.varint(one_based(details.overlay_texture));
        out.varint(one_based(details.overlay_mask));
        for value in [
            details.line_thickness,
            details.overlay_width,
            details.overlay_depth,
            details.footprint_width,
            details.footprint_depth,
            details.footprint_height,
            details.bar_width,
            details.bar_height,
            details.bar_offset,
        ] {
            out.f32(value);
        }
        for string in [
            details.generic_name,
            details.specific_name,
            details.icon,
            details.group_name,
            details.rank,
            details.classes,
            details.visible_classes,
            details.select_sound,
            details.lang,
            details.phenotypes,
        ] {
            out.varint(one_based(string));
        }
        out.varint(u64::from(details.components));
        out.varint(one_based(details.rally_texture));
        out.varint(one_based(details.rally_mask));
        out.f32(details.rally_thickness);
        out.u8(details.rally_start_cap);
        out.u8(details.rally_end_cap);
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

fn write_range_sets(out: &mut Writer, sets: &[RangeSet]) {
    out.varint(sets.len() as u64);
    for set in sets {
        out.varint(set.len() as u64);
        for circle in set {
            out.u8(circle.kind);
            out.f32(f32::from_bits(circle.radius));
            out.varint(u64::from(circle.texture));
            out.varint(u64::from(circle.mask));
            out.f32(f32::from_bits(circle.thickness));
        }
    }
}

fn write_variable_sets(out: &mut Writer, sets: &[VariableSet]) {
    out.varint(sets.len() as u64);
    for set in sets {
        out.varint(set.len() as u64);
        for (name, value) in set {
            out.varint(u64::from(*name));
            out.f32(*value);
        }
    }
}

/// `turn` is the message's turn, which the animation and sync turns are
/// counted back from; `hitpoints` is what the entity's template defaults
/// to.
fn write_dot(out: &mut Writer, id: u32, turn: u32, dot: &Dot, hitpoints: u8, capture: &[u8]) {
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
    out.varint(u64::from(dot.variables));
    out.varint(u64::from(dot.ranges));
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
    let mut motion = if dot.grounded { MOTION_GROUNDED } else { 0 };
    if dot.hitpoints != hitpoints {
        motion |= MOTION_HITPOINTS;
    }
    if !capture.is_empty() {
        motion |= MOTION_CAPTURE;
    }
    out.u8(motion);
    if motion & MOTION_HITPOINTS != 0 {
        out.u8(dot.hitpoints);
    }
    if motion & MOTION_CAPTURE != 0 {
        write_capture(out, capture);
    }
}
