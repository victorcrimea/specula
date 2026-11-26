//! The viewer stream, wire format v32: messages for each keyframe, turn or
//! match end, varints and zigzag as in `wire.rs`, positions and heights in
//! 1/16 m, carried brotli-compressed as below.
//!
//! - kind 1 keyframe: varint turn, u8 wire version (27; a viewer stops
//!   at any other, since nothing after it would read right), u64 match id, f32 game speed (the
//!   match's sim rate), varint delay in ms (how long after the game the
//!   server shows it), varint first turn of the match's history, varint
//!   turns per segment, varint map size in metres, u64 terrain id (0: none
//!   yet, else fetch `/terrain/<id as 16 hex digits>`), varint terrain
//!   revision (0: that terrain is whole; else a seat's, revealed in parts:
//!   fetch `/play/terrain/<id>`, which holds at least this many reveals),
//!   icon table, player table, territory, LOS, template table, string
//!   table, variant set table, variable set table, range set table, choice
//!   table, varint n, n x economy (every player's, by index),
//!   varint count, then count x {varint id delta, entity}, then the
//!   projectiles in flight or lying on the ground: varint count, count x
//!   {varint id delta, projectile, varint turns since the launch}, then
//!   the players who have the game paused: varint n, n x varint player
//!   index (v23), then the pause ledger (v24): varint m, m x {varint
//!   player, varint ms of pause time left, u8 flags (1 holds a pause, 2
//!   disconnected, 4 charged now), varint ms until the seat's pause ends
//!   after its budget ran out (0: none)}.
//! - kind 2 delta: varint turn, icons added to the table, u8 1 if a new
//!   player table follows (it replaces the old one) else 0, u8 1 if a new
//!   territory follows (likewise) else 0, u8 1 if LOS changes follow
//!   (below) else 0, u8 1 if a reveal follows else 0, then varint the
//!   terrain revision it brings and the reveal (`terrain.rs`), templates,
//!   strings, variant sets, variable sets, range sets and choices added to their tables, u8 1
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
//!   variable set; 65536 u8 hitpoints; 131072 capture; 262144 varint range
//!   set; 524288 mirage; 1048576 varint choices; 2097152 exact pose;
//!   4194304 groups}, then the sounds
//!   of the turn: varint count, count x sound, in the order the simulation
//!   played them, then the projectiles launched in the turn (for a seat,
//!   those that came into its sight): varint count, count x {varint id
//!   delta, launch}, then the ids of the projectiles
//!   that hit something and are gone: varint count, count x id delta,
//!   then the entities the turn renamed: varint count, count x {varint old
//!   id delta, zigzag new id - old id}. An
//!   animation (32) starts, and a sync (128) is set, at the delta's turn;
//!   it is also sent when only its sound group changed.
//! - kind 3 ended: u64 match id, varint ms since the match ended. No more
//!   turns of that match follow; the next keyframe with another id is the
//!   next match. A viewer joining between matches gets the final keyframe
//!   and then this message.
//! - kind 4 pause (v23, with the ledger from v24): varint turn (the last
//!   one played), varint n, n x varint player index: the players who have
//!   the game paused from now on, none for a resume, then the ledger (as a
//!   keyframe's). It sits in the frame between the deltas of the turns
//!   around it, since the game plays no turn while paused, and applies at
//!   its place in the frame. It is sent whenever the engine's pauser set
//!   changes or the ledger's shape changes, and once a second while a
//!   budget is charged or a countdown runs. Budgets are as of the message;
//!   a charged one runs down in real time after it.
//! - kind 5 flare (v25): varint turn, u8 sender player, f32 x, f32 z (in
//!   metres, little-endian). It is sent at once to the sender's mutual
//!   allies' seat streams and, with the delay, to the observers'. It sits in
//!   the frame like a pause, but is never in a keyframe (D51).
//! - kind 6 notification (v26): varint turn, u8 note (1 attacked, 2
//!   auto-queue stopped), u8 attacker player (Gaia 0), u8 flags (1 the
//!   target is a domestic animal, 2 it is a unit), varint target entity,
//!   f32 x, f32 z (metres; target and position 0 for a note 2).
//!   It is sent at once to the seat of the player it is for and to no
//!   observer; it sits in the frame like a flare and is never in a keyframe.
//! - kind 7 event (v27): varint turn, varint length, that many bytes of a
//!   UTF-8 JSON object (a defeat, win, phase, tribute, barter, diplomacy,
//!   AI chat or network line, which the client formats). It is sent at once
//!   to the seats it is for and, with the delay, to the observers. It sits
//!   in the frame like a flare and is never in a keyframe or the history
//!   (CONTROL-IMPL V105).
//! - kind 8 simulation time (v32): varint turn, varint microseconds, the
//!   mean wall time of the engine's turn updates over the last second or
//!   so. It is sent at once to every seat and, with the delay, to the
//!   observers, and sits in the frame like an event, so a stored match
//!   replays it; never in a keyframe.
//!
//! The players section (a keyframe's and a delta's player table) ends with
//! the engine's time notes (v27): varint count, then count x {varint end
//! ms, varint players count, zigzag each player, varint length, JSON
//! bytes}; then (v28) u8 lock teams (1: `settings.LockTeams`), n x u8
//! shared LOS (1: `hasSharedLos`; a seat is told only its own).
//!
//! The observers' flare envelope ends with the sender's chat nickname: a
//! varint length and the UTF-8 bytes (length 0 for none).
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
//! seed - id, varint choices, varint variant set, varint variable set, varint range set, varint animation,
//! u8 animation flags,
//! varint sound group, varint animation speed, varint turns since the
//! animation started, varint sync repeat, zigzag sync offset, varint turns
//! since the sync was set, u8 construction, varint visibility, mirage, zigzag step
//! x, zigzag step z, u8 motion flags, then u8 hitpoints if motion flag 2,
//! then capture if motion flag 4, then the exact pose if motion flag 8,
//! then the groups if motion flag 16}:
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
//! - visibility holds two bits per player p below 16, from bit 2p:
//!   `GetLosVisibility` for p, 0 hidden, 1 fogged, 2 visible; a seat's has
//!   only its player's;
//! - mirage is varint player + 1, 0 for an entity that is not a mirage,
//!   then, if it is one, varint the id of the entity it stands for (0:
//!   gone, or a seat's, which follows its handle instead). A mirage is
//!   what that player last saw of a building or resource in the fog; only
//!   that player's view shows it;
//! - choices are a 1-based index into the choice table, 0 for none: a
//!   seat is never sent an actor's seed (by default the entity id; it is
//!   0 then), but the variants the engine picks from it;
//! - step is how far it moves each turn until a delta changes it, in 1/16
//!   m;
//! - motion flags: 1 follows the ground, 2 hitpoints follow, 4 capture
//!   follows, 8 exact pose follows, 16 groups follow;
//! - the exact pose (motion flag 8, change bit 2097152) is u8 present, and
//!   when 1 zigzag x residual, zigzag z residual, zigzag angle, all raw
//!   16.16: the residuals are the engine's x and z less the sent ones
//!   (within half a 1/16 m step), the angle in radians. It is sent for an
//!   entity whose template has a static or cluster obstruction, which
//!   does not move, and absent without it;
//! - the groups (motion flag 16, change bit 4194304) are two varints, the
//!   entity's obstruction control groups, 0 for none, for a wall piece
//!   (a template with the WallPiece flag) that is not a mirage. In the
//!   observers' stream they are the feed's ids, a piece without any being
//!   its own group; in a seat's they are tokens that the seat numbers
//!   from 1 as it first meets a group, as a group may name an entity that
//!   is gone, so they compare with one another and with nothing else;
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
//! (from 0), in the order of `panelEntities`, u8 1 if training is blocked
//! (v29)}. The code and phase are 1-based indices into the string table, 0
//! for none.
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
//! actor, zigzag seed - shooter id, varint variant set, varint choices
//! (as an entity's, of the shooter's look as it shoots), zigzag x, zigzag z,
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
//! sounds are: {varint turns since its launch (0 but for a seat, which is
//! sent it once it comes into sight), varint shooter id, then with a known
//! shooter varint choices, else (or with 0) zigzag owner, varint actor,
//! zigzag seed - shooter id, varint variant set, varint choices; then the
//! launch point minus the
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
//! neutral, 255 unknown: a seat is told only its own stances and the
//! others' towards it), varint civ length, UTF-8 civ code (as in
//! simulation/data/civs/), u8 battle (1 when `BattleDetection` reports
//! one)}, then varint ceasefire end in simulation ms (0: none); index 0 is
//! gaia.
//! A territory is varint width, varint height (both 0: none yet), then
//! runs of {varint length, u8 tile} covering width * height tiles,
//! row-major, x fastest. A tile is 8 m square, as `ICmpTerritoryManager`'s
//! grid: owner in 0x1F, connected 0x20, blinking 0x40; 0x80 alone is a
//! tile a seat has not been shown, which no border is traced along.
//! LOS is varint vertices per side v (0: none yet), varint n, n x {varint
//! player, runs of {varint length, u8 state} covering v * v vertices,
//! row-major, x fastest}: each player's `CLosQuerier` state, bit 1
//! explored and bit 2 visible, one vertex every 4 m (the observers' stream
//! has every player's, a seat's its own). LOS changes are varint n, n x
//! {varint player, varint runs, runs x {varint vertices skipped since the
//! last run (from 0), varint count, ceil(count / 4) bytes of states, four
//! to a byte from the low bits}}.
//! A choice table is varint count, count x 3 x {varint n, n x varint
//! string}: the variant names the engine picks at random for an actor
//! (`specula_assets::variation::chosen`), 0-based string indices.
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
//! start cap, u8 rally line end cap, varint resource supply type, varint
//! gather rate names, varint dropsite types, varint attack types, varint
//! order sound groups, varint garrison classes, varint turret classes,
//! varint heal classes, varint unhealable classes, varint garrison size,
//! then u8 placement flags (1 BuildRestrictions, 2 WallPiece), u8 obstruction
//! type (0 none, 1 static, 2 unit, 3 cluster) and, when not 0, zigzag
//! width, zigzag depth (raw 16.16), u8 obstruction flags (1 BlockMovement,
//! 2 BlockFoundation, 4 BlockConstruction, 8 BlockPathfinding, 128
//! Active), varint cluster shape count, each shape 4 x zigzag x, z, width,
//! depth (raw 16.16)}. The
//! flags and
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
//! - 6 flare: f32 x, f32 z, an observer's flare. Only on the observers'
//!   socket, at once, and never in a segment or in the history (D51).
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
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use rocket::tokio::sync::broadcast;
use specula_assets::wire::WIRE_VERSION;

use crate::actors::Actors;
use crate::detail::DetailEncoder;
use crate::feed::Details;
use crate::feed::Economy;
use crate::feed::Entity;
use crate::feed::Icon;
use crate::feed::Launch;
use crate::feed::LosRun;
use crate::feed::Note;
use crate::feed::ObstructionKind;
use crate::feed::Players;
use crate::feed::Sound;
use crate::feed::SoundTarget;
use crate::feed::TemplateDetails;
use crate::feed::Terrain;
use crate::feed::Territory;
use crate::feed::TimeNote;
use crate::flare;
use crate::history::History;
use crate::history::Meta;
use crate::history::MetaPlayer;
use crate::limit;
use crate::pause;
use crate::registry::MatchId;
use crate::summary_envelope;
use crate::terrain::Ground;
use crate::web;
use crate::wire::Writer;
use crate::wire::quantize_angle;
use crate::wire::quantize_position;

mod seat;

use seat::Seat;
use seat::SeatInput;
use seat::SeatWater;
use seat::ShownReveal;

const MESSAGE_KEYFRAME: u8 = 1;
const MESSAGE_DELTA: u8 = 2;
const MESSAGE_ENDED: u8 = 3;
const MESSAGE_PAUSE: u8 = 4;
const MESSAGE_FLARE: u8 = 5;
const MESSAGE_NOTIFICATION: u8 = 6;
const MESSAGE_EVENT: u8 = 7;
const MESSAGE_SIM_TIME: u8 = 8;
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
const CHANGED_MIRAGE: u64 = 524288;
const CHANGED_CHOSEN: u64 = 1048576;
const CHANGED_EXACT: u64 = 2097152;
const CHANGED_GROUPS: u64 = 4194304;
const MOTION_GROUNDED: u8 = 1;
const MOTION_HITPOINTS: u8 = 2;
const MOTION_CAPTURE: u8 = 4;
const MOTION_EXACT: u8 = 8;
const MOTION_GROUPS: u8 = 16;
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
const ENVELOPE_FLARE: u8 = 6;

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
    /// Two bits per player, `feed::visibility_of`.
    visibility: u32,
    /// The player a mirage is for, plus 1; 0 for an entity that is not
    /// one.
    mirage: u32,
    /// The mirage's parent, 0 for none.
    parent: u32,
    /// A seat's: 1-based index into the choice table, the variants the
    /// engine picks from the seed, which a seat is not sent; 0 otherwise.
    chosen: u32,
    /// 0 without Health, else the hitpoints in 1/255 of the max, rounded
    /// up and at least 1, so that a scratch shows and a living entity
    /// never reads as having none.
    hitpoints: u8,
    /// What a viewer moves it by each turn; only in what viewers hold,
    /// zero in the simulation's state.
    step: [i32; 2],
    /// Its height is the ground's, likewise.
    grounded: bool,
    /// For a static or cluster obstruction, the exact pose past the
    /// quantised one: the x and z residuals in raw 16.16 (within half a
    /// 1/16 m step) and the raw 16.16 angle; zero otherwise.
    fine: [i32; 2],
    exact_angle: i32,
    exact: bool,
    /// For a wall piece, its two control groups, each 0 for none: the
    /// feed's ids in the observers' streams, a seat's own tokens in a
    /// seat's (see `Stream::control_groups`).
    groups: [u32; 2],
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
            mirage: entity.mirage.map_or(0, |mirage| mirage.player as u32 + 1),
            parent: entity.mirage.and_then(|mirage| mirage.parent).unwrap_or(0),
            chosen: 0,
            hitpoints,
            step: [0, 0],
            grounded: false,
            fine: [0, 0],
            exact_angle: 0,
            exact: false,
            groups: [0, 0],
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
    /// As a dot's, for the shooter's actor as it shoots.
    chosen: u32,
    launch: [i32; 3],
    target: [i32; 3],
    speed: u32,
    gravity: i32,
    projectile_actor: u32,
    impact_actor: u32,
    impact_lifetime: u32,
}

/// What a seat adds to a launch: how many turns ago it was launched (a
/// seat is sent a projectile only once it comes into sight), the shooter's
/// actor when the seat may not know the shooter, and its choices.
#[derive(Clone, Copy, Default)]
struct LaunchExtra {
    age: u32,
    origin: Option<ShotOrigin>,
    chosen: u32,
}

/// A shooter's owner and look, without its id.
#[derive(Clone, Copy)]
struct ShotOrigin {
    owner: i32,
    actor: u32,
    variants: u32,
}

impl Shot {
    fn new(launch: &Launch, turn: u32, shooter: Option<&Dot>, extra: LaunchExtra) -> Self {
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
        let launch_turn = turn.saturating_sub(extra.age);
        let (owner, actor, variants) = match (shooter, extra.origin) {
            (Some(dot), _) => (dot.owner, dot.actor, dot.variants),
            (None, Some(origin)) => (origin.owner, origin.actor, origin.variants),
            (None, None) => (-1, 0, 0),
        };
        Shot {
            launch_turn,
            last_turn: launch_turn.saturating_add(keep_turns),
            shooter: if shooter.is_some() { launch.shooter } else { 0 },
            owner,
            actor,
            // A seat gets the choices instead.
            seed: if extra.chosen != 0 {
                0
            } else {
                shooter.map_or(0, |dot| dot.seed)
            },
            variants,
            chosen: extra.chosen,
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
#[derive(Clone, Default, PartialEq, Eq)]
struct Funds {
    population: i32,
    population_limit: i32,
    population_max: i32,
    /// (1-based code string, whole amount, gatherers), sorted by code.
    resources: Vec<(u32, u32, u32)>,
    /// 1-based string index, 0 for none.
    phase: u32,
    panel_entities: Vec<u32>,
    training_blocked: bool,
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
            training_blocked: economy.training_blocked,
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
    choices: &'a [[Vec<u32>; 3]],
}

pub struct Hub {
    state: Mutex<State>,
    /// Wakes the publisher when something is queued.
    queued: Condvar,
    /// None when storage is off. Locked after `state` when both are, never
    /// before it; requests read its files with neither held.
    history: Option<Arc<Mutex<History>>>,
    /// Open observer visual sockets, for the lobby (M40); kept outside
    /// `state` so counting never waits on the encoder.
    observers: AtomicU32,
}

/// Counts one observer socket while it lives.
pub struct ObserverGuard(Arc<Hub>);

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        self.0.observers.fetch_sub(1, Ordering::Relaxed);
    }
}

/// One minimap-visible entity of a thumbnail, as the minimap draws it.
pub struct ThumbDot {
    /// In 1/16 m.
    pub x: i32,
    pub z: i32,
    /// 0xRRGGBB.
    pub color: u32,
    pub structure: bool,
}

/// What the lobby's thumbnail draws, captured when a turn is encoded and
/// shown once its delay is over, so a thumbnail never shows more than a
/// viewer sees (D61).
pub struct Thumb {
    /// In metres.
    pub map_size: u32,
    /// 0xRRGGBB by player index, gaia first.
    pub colors: Vec<u32>,
    pub territory: Option<Territory>,
    pub dots: Vec<ThumbDot>,
    /// Explored mode: the terrain tiles observers may see, by tile; the
    /// rest is drawn black. Empty before anything is explored.
    pub known: Option<Arc<Vec<bool>>>,
}

/// 10 s of game time; the lobby refreshes no faster.
const THUMB_TURNS: u32 = 50;

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
    summaries: Vec<Arc<SummaryEntry>>,
}

/// One capture of a match, shared by the queue, the shown match and the
/// thread compressing it.
pub struct SummaryEntry {
    pub match_id: u64,
    pub turn: u32,
    inner: Mutex<EntryState>,
}

struct EntryState {
    /// The envelope `{"turn":N,"final":b,"metadata":<full JSON>}`,
    /// uncompressed, until the brotli copy exists.
    raw: Option<Arc<[u8]>>,
    brotli: Option<Arc<[u8]>>,
    /// The publisher has shown it to `/`.
    released: bool,
    /// It is in the history, or there is none.
    stored: bool,
}

/// A capture's envelope and whether it is compressed.
pub struct SummaryBody {
    pub bytes: Arc<[u8]>,
    pub brotli: bool,
}

/// Compresses off the feed and seat threads: quality 11 takes long enough
/// on a large capture to stall a stream.
pub(crate) fn spawn_brotli(raw: Arc<[u8]>, store: impl FnOnce(Arc<[u8]>) + Send + 'static) {
    std::thread::spawn(move || {
        let brotli: Arc<[u8]> = web::compress(&raw).into();
        store(brotli);
    });
}

/// Writes the history copy of a released capture, once. The caller must
/// not hold the entry's lock.
fn store_summary(history: Option<&Mutex<History>>, entry: &SummaryEntry, protect: &[u64]) {
    let Some(history) = history else {
        return;
    };
    let brotli = {
        let mut inner = entry.inner.lock().unwrap();
        if !inner.released || inner.stored {
            return;
        }
        let Some(brotli) = inner.brotli.clone() else {
            return;
        };
        inner.stored = true;
        brotli
    };
    if let Err(error) =
        history
            .lock()
            .unwrap()
            .write_summary(entry.match_id, entry.turn, &brotli, protect)
    {
        eprintln!(
            "cannot store summary {} of match {:016x}: {error}",
            entry.turn, entry.match_id
        );
    }
}

/// The capture at or before `turn`, or the last one.
fn summary_body(entries: &[Arc<SummaryEntry>], turn: Option<u32>) -> Option<SummaryBody> {
    let entry = entries
        .iter()
        .rev()
        .find(|entry| turn.is_none_or(|turn| entry.turn <= turn))?;
    let inner = entry.inner.lock().unwrap();
    if let Some(brotli) = &inner.brotli {
        return Some(SummaryBody {
            bytes: Arc::clone(brotli),
            brotli: true,
        });
    }
    inner.raw.as_ref().map(|raw| SummaryBody {
        bytes: Arc::clone(raw),
        brotli: false,
    })
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
        /// Explored mode: what this turn added to the explored vertices,
        /// for the shadow of the reveal (`ShownReveal`).
        explored: Option<Arc<seat::ExploredGrowth>>,
    },
    /// Queued like the turns around it, so `/` sees it `delay` late.
    Summary(Arc<SummaryEntry>),
    /// The lobby's thumbnail snapshot, delayed like a turn (M41).
    Thumb(Arc<Thumb>),
    /// A pause or resume, a message in the visual frame.
    Pause(TrackOp),
    /// A player's flare or notification, a message in the visual frame.
    Flare(TrackOp),
    /// A control message in the detail frame.
    Detail(TrackOp),
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
    /// Every capture released for the shown match, in turn order.
    summaries: Vec<Arc<SummaryEntry>>,
    /// The latest thumbnail snapshot released (M41).
    thumb: Option<Arc<Thumb>>,
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
                ..
            } => {
                // The history restarts only now, so until the new match's
                // first turn, viewers and segment requests still get the
                // old one.
                if let Some(match_id) = start {
                    self.match_id = match_id;
                    self.ended = None;
                    self.summaries.clear();
                }
                self.has_turn = true;
                let visual = self.visual.apply(visual, store(self.match_id, false));
                let detail = self.detail.apply(detail, store(self.match_id, true));
                // No receivers is not an error: nobody is watching yet.
                let _ = self.visual.sender.send(visual);
                let _ = self.detail.sender.send(detail);
            }
            Release::Pause(op) | Release::Flare(op) => {
                // Before the match's first turn there is no frame to put it in.
                if !self.has_turn || self.ended.is_some() {
                    return;
                }
                let message = self.visual.apply(op, store(self.match_id, false));
                let _ = self.visual.sender.send(message);
            }
            Release::Detail(op) => {
                if !self.has_turn || self.ended.is_some() {
                    return;
                }
                let message = self.detail.apply(op, store(self.match_id, true));
                let _ = self.detail.sender.send(message);
            }
            Release::Thumb(thumb) => self.thumb = Some(thumb),
            Release::Summary(entry) => {
                if entry.match_id != self.match_id {
                    return;
                }
                self.summaries.push(Arc::clone(&entry));
                entry.inner.lock().unwrap().released = true;
                store_summary(history, &entry, &protect);
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
                    summaries: std::mem::take(&mut self.summaries),
                });
                // Viewers learn of the end only now.
                ended.at = Instant::now();
                self.ended = Some(ended);
                let _ = self
                    .visual
                    .sender
                    .send(envelope_ended(&encode_ended(match_id, 0)).into());
                if let Some(history) = history {
                    history.lock().unwrap().set_live(match_id, false);
                }
            }
        }
    }

    /// A message sent only on the visual socket at once, which no track
    /// holds: an observer's flare is never stored (D51).
    fn side(&self, message: Message) {
        if !self.has_turn || self.ended.is_some() {
            return;
        }
        let _ = self.visual.sender.send(message);
    }
}

/// What every stream of the match is encoded with besides its own state.
struct Common {
    match_id: u64,
    game_speed: f32,
    /// How long after the game the stream is shown.
    delay: Duration,
    first_turn: u32,
    map_size: u32,
    terrain_id: u64,
}

/// One encoding of the match: the observers', from the feed as it is, or a
/// seat's, from what its player may see (`filter.rs`). Its tables are its
/// own, so a seat's indices say nothing of what it was not sent.
struct Stream {
    visual: Compressor,
    detail: Compressor,
    detail_encoder: DetailEncoder,
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
    /// The LOS grids the stream carries, by player index, and what viewers
    /// hold of them.
    los: Los,
    los_shown: Los,
    /// A seat's terrain: how many reveals it has had (0: the terrain
    /// resource is whole), and the next delta's reveal, if any.
    terrain_revision: u32,
    reveal: Option<Vec<u8>>,
    /// A seat's: the variant choices, each three sets of 0-based string
    /// indices, and by id this turn the entities' 1-based ones.
    choices: Table<[Vec<u32>; 3]>,
    choice_ids: HashMap<[Vec<u32>; 3], u32>,
    dot_choices: HashMap<u32, u32>,
    /// A seat's: by id this turn, the turns the entities' animations and
    /// syncs started on as the observers' stream counts them, since an
    /// entity that comes back into sight gets a new id but the engine
    /// played its animation and decay on all the while.
    dot_starts: HashMap<u32, (u32, u32)>,
    /// The control groups of the wall pieces that have any, by this
    /// stream's id: the feed's ids in the observers' stream, a seat's
    /// tokens (not handles, as a group may name an entity that is gone) in
    /// a seat's. A piece without an entry is its own group in the observers'
    /// stream (`raw_groups`), and none in a seat's.
    control_groups: HashMap<u32, [u32; 2]>,
    raw_groups: bool,
    /// A seat's additions to this turn's launches, by projectile id.
    launch_extras: HashMap<u32, LaunchExtra>,
    /// The players who have the game paused, ascending.
    paused: Vec<u32>,
    /// The pause ledger the stream last sent (D41).
    ledger: Vec<pause::Entry>,
}

/// LOS vertex states (`feed::LOS_EXPLORED`, `feed::LOS_VISIBLE`) by player
/// index, row-major, x fastest.
#[derive(Clone, Default, PartialEq, Eq)]
struct Los {
    side: u32,
    players: BTreeMap<u32, Vec<u8>>,
}

impl Stream {
    fn new() -> Self {
        Stream {
            visual: Compressor::new(),
            detail: Compressor::new(),
            detail_encoder: DetailEncoder::new(),
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
            los: Los::default(),
            los_shown: Los::default(),
            terrain_revision: 0,
            reveal: None,
            choices: Table::new(),
            choice_ids: HashMap::new(),
            dot_choices: HashMap::new(),
            dot_starts: HashMap::new(),
            control_groups: HashMap::new(),
            raw_groups: false,
            launch_extras: HashMap::new(),
            paused: Vec::new(),
            ledger: Vec::new(),
        }
    }

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
            choices: sent(&self.choices, with_unsent),
        }
    }

    fn set_players(&mut self, players: Players) {
        if self.players != players {
            self.players = players;
            self.players_changed = true;
        }
    }

    fn set_territory(&mut self, territory: Territory) {
        if self.territory.as_ref() != Some(&territory) {
            self.territory = Some(territory);
            self.territory_changed = true;
        }
    }

    /// The 1-based icon index of an icon, added to the table if new.
    fn intern_icon(&mut self, icon: Icon) -> u32 {
        let icons = &mut self.icons.items;
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
        index as u32 + 1
    }

    /// What a placement check reads of a dot: the exact pose of a static
    /// obstruction, which never moves so that it costs once, and a wall
    /// piece's control groups.
    fn place(&self, dot: &mut Dot, entity: &Entity) {
        let Some(details) = self
            .templates
            .items
            .get(entity.template as usize)
            .and_then(|template| template.details.as_ref())
        else {
            return;
        };
        if details
            .obstruction
            .as_ref()
            .is_some_and(|obstruction| obstruction.kind != ObstructionKind::Unit)
        {
            dot.exact = true;
            dot.fine = [
                entity.x - (quantize_position(entity.x) << 12),
                entity.z - (quantize_position(entity.z) << 12),
            ];
            dot.exact_angle = entity.angle;
        }
        // Walls snap to the player's own towers, never to a mirage.
        if details.placement_flags & 2 != 0 && entity.mirage.is_none() {
            dot.groups =
                self.control_groups
                    .get(&entity.id)
                    .copied()
                    .unwrap_or(if self.raw_groups {
                        [entity.id, 0]
                    } else {
                        [0, 0]
                    });
        }
    }

    /// The turns `dot`'s animation and sync started on, from last turn's
    /// dot of `id`; `start` for the first turn of a match.
    fn starts(&self, id: u32, dot: &Dot, start: bool) -> (u32, u32) {
        let mut starts = (dot.animation_turn, dot.sync_turn);
        // The hook only sees the state after each turn, so an animation
        // counts as started on the turn it first shows. The old match's
        // ids are unrelated to the new one's.
        if let Some(before) = self.dots.get(&id).filter(|_| !start) {
            if before.animation == dot.animation && before.once == dot.once {
                starts.0 = before.animation_turn;
            }
            if before.sync_repeat == dot.sync_repeat && before.sync_offset == dot.sync_offset {
                starts.1 = before.sync_turn;
            }
        }
        starts
    }

    /// Encodes a turn of `entities`, whose indices are this stream's, and
    /// `details`, likewise; `start` for the first turn of a match.
    /// Returns the visual and detail stream operations.
    fn encode_turn(
        &mut self,
        common: &Common,
        turn: u32,
        entities: &[Entity],
        details: &Details,
        ground: Option<&Ground>,
        start: bool,
    ) -> io::Result<(TrackOp, TrackOp)> {
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
                let next = self.range_sets.items.len() as u32 + 1;
                let index = *self.range_set_ids.entry(set.clone()).or_insert(next);
                if index == next {
                    self.range_sets.items.push(set);
                }
                Some((entity.id, index))
            })
            .collect();
        let dots: BTreeMap<u32, Dot> = entities
            .iter()
            .map(|entity| {
                let icon = self
                    .template_icons
                    .get(&entity.template)
                    .copied()
                    .unwrap_or(0);
                let player_color = self.player_color_templates.contains(&entity.template);
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
                self.place(&mut dot, entity);
                if let Some(chosen) = self.dot_choices.get(&entity.id) {
                    dot.chosen = *chosen;
                    dot.seed = 0;
                }
                (dot.animation_turn, dot.sync_turn) = self
                    .dot_starts
                    .get(&entity.id)
                    .copied()
                    .unwrap_or_else(|| self.starts(entity.id, &dot, start));
                (entity.id, dot)
            })
            .collect();
        let captures: BTreeMap<u32, Vec<u8>> = entities
            .iter()
            .map(|entity| (entity.id, quantize_capture(details, entity.id)))
            .filter(|(_, shares)| !shares.is_empty())
            .collect();
        // The old match's projectile ids are unrelated to the new one's.
        let mut shots = if start {
            BTreeMap::new()
        } else {
            self.shots.clone()
        };
        shots.retain(|_, shot| shot.last_turn >= turn);
        let mut removals: Vec<u32> = self
            .removals
            .iter()
            .copied()
            .filter(|id| shots.remove(id).is_some())
            .collect();
        removals.sort_unstable();
        let mut launches = Vec::with_capacity(self.launches.len());
        for launch in &self.launches {
            // Taken out in the turn that shot it: nothing to draw.
            if self.removals.contains(&launch.id) {
                continue;
            }
            let shooter = dots
                .get(&launch.shooter)
                .or_else(|| self.dots.get(&launch.shooter))
                .filter(|_| launch.shooter != 0);
            let extra = self
                .launch_extras
                .get(&launch.id)
                .copied()
                .unwrap_or_default();
            let shot = Shot::new(launch, turn, shooter, extra);
            shots.insert(launch.id, shot);
            launches.push((launch.id, shot));
        }
        launches.sort_unstable_by_key(|(id, _)| *id);
        let (ops, shown) = if start {
            let shown: BTreeMap<u32, Dot> = dots
                .iter()
                .map(|(id, dot)| (*id, arrive(ground, dot)))
                .collect();
            self.captures = captures;
            // A keyframe holds the whole revealed terrain, as a reveal
            // would bring it.
            self.reveal = None;
            // Nothing is paused as a match starts.
            self.paused.clear();
            let keyframe = encode_keyframe(common, self, turn, &self.tables(true), &shown, &shots);
            let visual = self.visual.restart(&keyframe);
            // The keyframe holds what the turn's delta would bring.
            self.detail_encoder.reset();
            self.detail_encoder
                .delta(turn, details, |id| shown.contains_key(&id));
            let detail_keyframe = self.detail_encoder.keyframe(turn, common.match_id, |id| {
                shown.get(&id).map(|dot| dot.template)
            });
            let detail = self.detail.restart(&detail_keyframe);
            ((visual, detail), shown)
        } else {
            let (delta, shown) = encode_delta(
                turn,
                &Tables {
                    icons: self.icons.unsent(),
                    templates: self.templates.unsent(),
                    strings: self.strings.unsent(),
                    variant_sets: self.variant_sets.unsent(),
                    variable_sets: self.variable_sets.unsent(),
                    range_sets: self.range_sets.unsent(),
                    choices: self.choices.unsent(),
                },
                self.players_changed.then_some(&self.players),
                self.territory_changed
                    .then_some(self.territory.as_ref())
                    .flatten(),
                (self.los != self.los_shown).then_some((&self.los_shown, &self.los)),
                self.reveal
                    .as_deref()
                    .map(|reveal| (self.terrain_revision, reveal)),
                ground,
                &self.dots,
                &dots,
                &self.sounds,
                &launches,
                &removals,
                &Status {
                    templates: &self.templates.items,
                    previous_captures: &self.captures,
                    captures: &captures,
                    economy: &self.economy,
                    economy_shown: &self.economy_shown,
                    renames: &details.renames,
                },
            );
            self.captures = captures;
            let detail_delta = self
                .detail_encoder
                .delta(turn, details, |id| dots.contains_key(&id));
            let next_start = common.first_turn + SEGMENT_TURNS * (self.visual.closed + 1);
            self.reveal = None;
            let (keyframe, detail_keyframe) = if turn >= next_start {
                (
                    Some(encode_keyframe(
                        common,
                        self,
                        turn,
                        &self.tables(true),
                        &shown,
                        &shots,
                    )),
                    Some(self.detail_encoder.keyframe(turn, common.match_id, |id| {
                        shown.get(&id).map(|dot| dot.template)
                    })),
                )
            } else {
                (None, None)
            };
            let visual = self.visual.push(&delta, keyframe.as_deref())?;
            let detail = self
                .detail
                .push(&detail_delta, detail_keyframe.as_deref())?;
            ((visual, detail), shown)
        };
        self.icons.sent = self.icons.items.len();
        self.templates.sent = self.templates.items.len();
        self.strings.sent = self.strings.items.len();
        self.variant_sets.sent = self.variant_sets.items.len();
        self.variable_sets.sent = self.variable_sets.items.len();
        self.range_sets.sent = self.range_sets.items.len();
        self.choices.sent = self.choices.items.len();
        self.players_changed = false;
        self.territory_changed = false;
        self.economy_shown = self.economy.clone();
        self.los_shown = self.los.clone();
        // A keyframe carries no sounds, so a new match's first turn drops
        // its own.
        self.sounds.clear();
        self.launches.clear();
        self.removals.clear();
        self.launch_extras.clear();
        self.shots = shots;
        self.dots = shown;
        Ok(ops)
    }

    /// Sets who has the game paused and returns the message that tells the
    /// viewers, to go into the open frame after the last turn.
    fn pause(&mut self, turn: u32, players: &[i8]) -> io::Result<TrackOp> {
        self.paused = players
            .iter()
            .filter_map(|player| u32::try_from(*player).ok())
            .collect();
        self.paused.sort_unstable();
        self.pause_message(turn)
    }

    /// Sets the pause ledger and returns the message that tells the
    /// viewers, as a pause message does.
    fn set_ledger(&mut self, turn: u32, entries: &[pause::Entry]) -> io::Result<TrackOp> {
        self.ledger = entries.to_vec();
        self.pause_message(turn)
    }

    /// Sets a seat's control and returns the message that tells the
    /// viewers, if it changed.
    fn set_control(&mut self, turn: u32, body: &[u8]) -> io::Result<Option<TrackOp>> {
        match self.detail_encoder.control(turn, body) {
            Some(message) => self.detail.push(&message, None).map(Some),
            None => Ok(None),
        }
    }

    fn pause_message(&mut self, turn: u32) -> io::Result<TrackOp> {
        let mut out = Writer::default();
        out.u8(MESSAGE_PAUSE);
        out.varint(u64::from(turn));
        out.varint(self.paused.len() as u64);
        for player in &self.paused {
            out.varint(u64::from(*player));
        }
        write_ledger(&mut out, &self.ledger);
        self.visual.push(&out.0, None)
    }

    fn flare_message(&mut self, turn: u32, sender: u8, x: f32, z: f32) -> io::Result<TrackOp> {
        let mut out = Writer::default();
        out.u8(MESSAGE_FLARE);
        out.varint(u64::from(turn));
        out.u8(sender);
        out.f32(x);
        out.f32(z);
        self.visual.push(&out.0, None)
    }

    fn notification_message(&mut self, turn: u32, note: &Note) -> io::Result<TrackOp> {
        let mut out = Writer::default();
        out.u8(MESSAGE_NOTIFICATION);
        out.varint(u64::from(turn));
        out.u8(note.kind);
        out.u8(note.attacker);
        out.u8(note.flags);
        out.varint(u64::from(note.target));
        out.f32(note.x);
        out.f32(note.z);
        self.visual.push(&out.0, None)
    }

    fn sim_time_message(&mut self, turn: u32, microseconds: u32) -> io::Result<TrackOp> {
        let mut out = Writer::default();
        out.u8(MESSAGE_SIM_TIME);
        out.varint(u64::from(turn));
        out.varint(u64::from(microseconds));
        self.visual.push(&out.0, None)
    }

    fn event_message(&mut self, turn: u32, json: &str) -> io::Result<TrackOp> {
        let mut out = Writer::default();
        out.u8(MESSAGE_EVENT);
        out.varint(u64::from(turn));
        out.varint(json.len() as u64);
        out.0.extend_from_slice(json.as_bytes());
        self.visual.push(&out.0, None)
    }

    /// The final keyframes of an ending match, and the tails of its frames.
    fn finish(&mut self, common: &Common, turn: u32) -> (Ended, Vec<u8>, Vec<u8>) {
        // A match that ends while paused is over, not paused.
        self.paused.clear();
        self.ledger.clear();
        let keyframe: Message = compress(&encode_keyframe(
            common,
            self,
            turn,
            &self.tables(true),
            &self.dots,
            &self.shots,
        ))
        .into();
        let detail_keyframe: Message =
            compress(&self.detail_encoder.keyframe(turn, common.match_id, |id| {
                self.dots.get(&id).map(|dot| dot.template)
            }))
            .into();
        let ended = Ended {
            at: Instant::now(),
            keyframe,
            detail_keyframe,
        };
        (ended, self.visual.finish(), self.detail.finish())
    }
}

/// What the registry knows of a hosted match for its lobby record.
#[derive(Clone, Default)]
pub struct MetaSource {
    pub host: String,
    pub map_name: Option<String>,
    /// The map preview's file stem.
    pub preview: Option<String>,
    /// Player numbers an AI plays; the feed does not say which are bots.
    pub ai_players: Vec<u32>,
}

/// Each player's name, civ and colour, gaia left out.
fn meta_players(state: &State) -> Vec<MetaPlayer> {
    let ai_players = state
        .meta_source
        .as_ref()
        .map_or(&[][..], |source| &source.ai_players[..]);
    state
        .observer
        .players
        .players
        .iter()
        .enumerate()
        .skip(1)
        .map(|(index, player)| MetaPlayer {
            name: player.name.clone(),
            civ: player.civ.clone(),
            color: player.color,
            ai: ai_players.contains(&(index as u32)),
        })
        .collect()
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
    /// The observers' encoding, whose tables are the feed's.
    observer: Stream,
    /// In play mode, a stream for each player a web visitor plays, filtered
    /// to what that player sees, shown at once and never stored, while
    /// `shown` keeps the delay for observers.
    seats: Vec<Seat>,
    /// The players a hosted game has seats for, set before the feed starts.
    seat_players: Vec<u32>,
    /// Explored mode (D56): the observers' stream, filtered to what any
    /// player explored. It replaces the observer stream's output, which
    /// is still encoded because the seats read its tables.
    explored: Option<Seat>,
    /// The terrain reveal as the observers have been shown it.
    explored_shown: Option<ShownReveal>,
    /// What the seats' water derives from the whole heightmap, once the
    /// terrain is known.
    seat_water: Option<Arc<SeatWater>>,
    /// The baked actors, for the variant choices the seats are sent.
    actors: Actors,
    /// Oldest first, each with when it is due.
    pending: VecDeque<(Instant, Release)>,
    shown: Shown,
    terrain: Option<(u64, TerrainFile)>,
    /// The feed's terrain, for the seats' revealed parts.
    terrain_source: Option<Arc<Terrain>>,
    /// The kind 18 and 19 JSON of the match, by the hash the detail
    /// stream names it by.
    data_files: DataFiles,
    ground: Option<Ground>,
    /// Every player's LOS, as the feed has it.
    los: Los,
    has_turn: bool,
    /// `settings.LockTeams` of the running match.
    lock_teams: bool,
    /// No players record of the running match has had its active players'
    /// alliances differ from their teams, so the summary may group by team
    /// even with the teams unlocked.
    teams_held: bool,
    /// Set when a new observer connects: its entity ids belong to another
    /// match, so its first turn must replace, not patch, what viewers have.
    new_feed: bool,
    /// Set from the end of a match until the next one's first turn.
    ended: bool,
    /// The engine sent its final capture. The players record that carries
    /// the states only comes every few turns, so a feed closed soon after
    /// the result would otherwise leave the match undecided.
    final_seen: bool,
    /// When the feed last connected or sent a turn, for the observer
    /// watchdog.
    feed_activity: Option<Instant>,
    /// A hosted hub's first match takes its registry UUID; any other match
    /// a new one.
    next_uuid: Option<MatchId>,
    /// What the registry knows and the feed does not, for the history's
    /// lobby record (M40).
    meta_source: Option<MetaSource>,
    /// Every match this hub encoded, so `/g/<uuid>` finds it while the hub
    /// holds it.
    uuids: Vec<(MatchId, u64)>,
    /// The registry dropped the hub; the publisher returns once nothing is
    /// queued, so the hub is freed when its last viewer closes.
    retired: bool,
    /// How often each player and each observer may flare, and the
    /// observers' shared budget.
    flares: limit::Limiter<flare::Sender>,
}

impl State {
    /// The observers' latest dots, territory and colours; gaia's units and
    /// anything off the minimap (colour 0) are left out, as on the minimap.
    fn thumb(&self) -> Thumb {
        // Explored mode shows what the observers' stream holds.
        let source = self.explored.as_ref().map_or(&self.observer, Seat::stream);
        let templates = &source.templates.items;
        let dots = source
            .dots
            .values()
            .filter(|dot| dot.color != 0 && dot.mirage == 0)
            .map(|dot| ThumbDot {
                x: dot.x,
                z: dot.z,
                color: dot.color & 0x00FF_FFFF,
                structure: templates
                    .get(dot.template as usize)
                    .is_some_and(|template| template.name.contains("structures/")),
            })
            .collect();
        Thumb {
            map_size: self.map_size,
            colors: source
                .players
                .players
                .iter()
                .map(|player| player.color)
                .collect(),
            territory: source.territory.clone(),
            dots,
            known: self
                .explored
                .as_ref()
                .map(|explored| Arc::new(explored.known_tiles().unwrap_or_default().to_vec())),
        }
    }

    fn common(&self) -> Common {
        Common {
            match_id: self.match_id,
            game_speed: self.game_speed,
            delay: self.delay,
            first_turn: self.first_turn,
            map_size: self.map_size,
            terrain_id: self.terrain.as_ref().map_or(0, |(id, _)| *id),
        }
    }
}

/// Who an event is for, from vanilla's chat formats: the indices of the
/// seats and whether the observers get it. An AI chat line has its
/// addressee command stripped and its channel added, as the engine sends
/// every client the whole line and each filters it. None drops the event.
fn route_event(state: &State, event: &mut serde_json::Value) -> Option<(Vec<usize>, bool)> {
    let players = &state.observer.players;
    let number = |value: &serde_json::Value, key: &str| value.get(key)?.as_i64();
    let first = event.get("players")?.get(0)?.as_i64()?;
    let seats_where = |keep: &dyn Fn(usize) -> bool| -> Vec<usize> {
        state
            .seats
            .iter()
            .enumerate()
            .filter(|(_, seat)| keep(seat.player as usize))
            .map(|(index, _)| index)
            .collect()
    };
    let ally =
        |a: i64, b: i64| a >= 0 && b >= 0 && seat::mutual_ally(players, a as usize, b as usize);
    let kind = event.get("type")?.as_str()?.to_string();
    match kind.as_str() {
        "defeat" | "won" => Some((seats_where(&|_| true), true)),
        "phase" => Some((
            seats_where(&|seat| seat as i64 == first || ally(first, seat as i64)),
            true,
        )),
        "tribute" => {
            let donator = number(event, "donator")?;
            let locked = state.lock_teams;
            Some((
                seats_where(&|seat| {
                    let seat = seat as i64;
                    seat == first
                        || seat == donator
                        || (locked && ally(seat, first) && ally(seat, donator))
                }),
                true,
            ))
        }
        "barter" => Some((Vec::new(), true)),
        "diplomacy" => {
            let target = number(event, "targetPlayer")?;
            Some((
                seats_where(&|seat| seat as i64 == first || seat as i64 == target),
                true,
            ))
        }
        // Only the requester is told, as `messages.js` acts on it for the
        // viewed player; observers have none.
        "spy-response" => Some((seats_where(&|seat| seat as i64 == first), false)),
        "aichat" => {
            let message = event.get("message")?.as_str()?.to_string();
            let enemy = |seat: usize| {
                first >= 0
                    && players
                        .players
                        .get(first as usize)
                        .and_then(|player| player.stances.get(seat))
                        == Some(&crate::chat::STANCE_ENEMY)
            };
            let (channel, text, seats, observers) = if let Some(rest) =
                message.strip_prefix("/allies ")
            {
                (
                    "allies",
                    rest.to_string(),
                    seats_where(&|seat| seat as i64 != first && ally(first, seat as i64)),
                    false,
                )
            } else if let Some(rest) = message.strip_prefix("/enemies ") {
                (
                    "enemies",
                    rest.to_string(),
                    seats_where(&|seat| seat as i64 != first && enemy(seat)),
                    false,
                )
            } else if let Some(rest) = message.strip_prefix("/observers ") {
                ("observers", rest.to_string(), Vec::new(), true)
            } else if let Some(rest) = message.strip_prefix("/msg ") {
                // As vanilla's matchUsername: the longest name that is
                // followed by a space.
                let (name, text) = players
                    .players
                    .iter()
                    .skip(1)
                    .filter(|player| !player.name.is_empty())
                    .filter_map(|player| {
                        let text = rest.strip_prefix(player.name.as_str())?.strip_prefix(' ')?;
                        Some((player.name.as_str(), text))
                    })
                    .max_by_key(|(name, _)| name.len())?;
                let target = players
                    .players
                    .iter()
                    .position(|player| player.name == name)?;
                (
                    "private",
                    text.to_string(),
                    seats_where(&|seat| seat == target),
                    false,
                )
            } else {
                ("everyone", message, seats_where(&|_| true), true)
            };
            let object = event.as_object_mut()?;
            object.insert("message".into(), text.into());
            object.insert("channel".into(), channel.into());
            Some((seats, observers))
        }
        _ => None,
    }
}

/// A flare needs a running match and a point on the map.
fn check_flare(state: &State, x: f32, z: f32) -> Result<(), &'static str> {
    if !state.has_turn || state.ended {
        return Err("no game running");
    }
    let map_size = state.map_size as f32;
    if !(x.is_finite() && z.is_finite()) || x < 0.0 || z < 0.0 || x > map_size || z > map_size {
        return Err("off the map");
    }
    Ok(())
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
            summaries: Vec::new(),
            thumb: None,
        }
    }
}

/// Which stream a request reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// `/`, delayed and stored.
    Observer,
    /// `/play`, the seat of this player.
    Seat(u32),
}

/// The history's copy of a closed segment, for a match no hub holds.
pub(crate) fn stored_segment(
    history: &Mutex<History>,
    match_id: u64,
    detail: bool,
    index: usize,
) -> Option<(Message, bool)> {
    let path = history
        .lock()
        .unwrap()
        .segment_path(match_id, detail, index)?;
    // Only closed segments are stored. The match may have been deleted
    // since the lookup; then the file is gone, as it would be after.
    let bytes = std::fs::read(path).ok()?;
    Some((bytes.into(), true))
}

pub(crate) fn stored_data(history: &Mutex<History>, hash: u64) -> Option<Arc<[u8]>> {
    let path = history.lock().unwrap().data_path(hash)?;
    Some(std::fs::read(path).ok()?.into())
}

pub(crate) fn stored_terrain(history: &Mutex<History>, id: u64) -> Option<TerrainFile> {
    let (raw, brotli) = history.lock().unwrap().terrain_paths(id)?;
    let file = TerrainFile {
        raw: std::fs::read(raw).ok()?.into(),
        brotli: Arc::new(OnceLock::new()),
    };
    if let Ok(bytes) = std::fs::read(brotli) {
        let _ = file.brotli.set(bytes.into());
    }
    Some(file)
}

pub(crate) fn stored_summary(
    history: &Mutex<History>,
    match_id: u64,
    turn: Option<u32>,
) -> Option<SummaryBody> {
    let path = history.lock().unwrap().summary_at(match_id, turn)?;
    let bytes: Arc<[u8]> = std::fs::read(path).ok()?.into();
    Some(SummaryBody {
        bytes,
        brotli: true,
    })
}

impl Hub {
    pub fn new(
        delay: Duration,
        history: Option<Arc<Mutex<History>>>,
        uuid: Option<MatchId>,
    ) -> Self {
        Hub {
            queued: Condvar::new(),
            history,
            observers: AtomicU32::new(0),
            state: Mutex::new(State {
                turn: 0,
                map_size: 0,
                game_speed: 1.0,
                match_id: 0,
                first_turn: 0,
                delay,
                observer: {
                    let mut observer = Stream::new();
                    observer.raw_groups = true;
                    observer
                },
                seats: Vec::new(),
                seat_players: Vec::new(),
                explored: None,
                explored_shown: None,
                seat_water: None,
                actors: Actors::new(),
                pending: VecDeque::new(),
                shown: Shown::new(),
                terrain: None,
                terrain_source: None,
                data_files: HashMap::new(),
                ground: None,
                los: Los::default(),
                has_turn: false,
                lock_teams: false,
                teams_held: true,
                new_feed: true,
                ended: false,
                final_seen: false,
                feed_activity: None,
                next_uuid: uuid,
                meta_source: None,
                uuids: Vec::new(),
                retired: false,
                flares: limit::Limiter::new(flare::SCOPE),
            }),
        }
    }

    /// Tells the seats' viewers their control: the seat's own flags and the
    /// human rows (`Play::tick_control`). A seat not in `bodies` keeps its
    /// control; the observers' stream never gets one.
    pub fn set_control(&self, bodies: &[(u32, Vec<u8>)]) {
        let mut state = self.state.lock().unwrap();
        if !state.has_turn || state.ended {
            return;
        }
        let turn = state.turn;
        for (player, body) in bodies {
            let Some(seat) = state.seats.iter_mut().find(|seat| seat.player == *player) else {
                continue;
            };
            if let Err(error) = seat.control(turn, body) {
                eprintln!("cannot encode a control: {error}");
            }
        }
    }

    /// Whether every active player who is not in `left`, AIs included, is a
    /// mutual ally of every other one (the claim-victory rule). Gaia is not
    /// a player here.
    pub fn claim_allies(&self, left: &[u32]) -> bool {
        let state = self.state.lock().unwrap();
        let players = &state.observer.players;
        let active: Vec<usize> = (1..players.players.len())
            .filter(|&index| {
                players.players[index].state == seat::PLAYER_ACTIVE
                    && !left.contains(&(index as u32))
            })
            .collect();
        active.iter().all(|&a| {
            active
                .iter()
                .all(|&b| a == b || seat::mutual_ally(players, a, b))
        })
    }

    /// The players of the next feed that web visitors play. Their streams
    /// exist from now, so a page can wait on one for the first turn.
    pub fn set_seats(&self, players: Vec<u32>) {
        let state = &mut *self.state.lock().unwrap();
        state.seats.retain(|seat| players.contains(&seat.player));
        for player in &players {
            if !state.seats.iter().any(|seat| seat.player == *player) {
                state.seats.push(Seat::new(*player));
            }
        }
        state.seat_players = players;
    }

    pub fn start_feed(&self) {
        let mut state = self.state.lock().unwrap();
        state.new_feed = true;
        state.feed_activity = Some(Instant::now());
        state.terrain = None;
        state.terrain_source = None;
        state.seat_water = None;
        state.ground = None;
        state.los = Los::default();
        // A rebake may have come in between matches.
        state.actors = Actors::new();
        let observer = &mut state.observer;
        observer.icons.clear();
        observer.templates.clear();
        observer.strings.clear();
        observer.variant_sets.clear();
        observer.variable_sets.clear();
        observer.range_sets.clear();
        observer.range_set_ids.clear();
        observer.template_icons.clear();
        observer.player_color_templates.clear();
        observer.control_groups.clear();
        observer.players = Players::default();
        observer.territory = None;
        observer.economy.clear();
        observer.sounds.clear();
        observer.launches.clear();
        observer.removals.clear();
        observer.los = Los::default();
        observer.los_shown = Los::default();
    }

    pub fn add_template(&self, index: u32, name: String) -> io::Result<()> {
        self.state.lock().unwrap().observer.templates.insert(
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
        let observer = &mut self.state.lock().unwrap().observer;
        let template = observer
            .templates
            .items
            .get_mut(index as usize)
            .filter(|_| index as usize >= observer.templates.sent)
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
            .observer
            .strings
            .insert(index, text, "string")
    }

    pub fn add_variant_set(&self, index: u32, pairs: VariantSet) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .observer
            .variant_sets
            .insert(index, pairs, "variant set")
    }

    pub fn add_variable_set(&self, index: u32, pairs: VariableSet) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .observer
            .variable_sets
            .insert(index, pairs, "variable set")
    }

    pub fn set_template_player_color(&self, template: u32) {
        self.state
            .lock()
            .unwrap()
            .observer
            .player_color_templates
            .insert(template);
    }

    pub fn set_players(&self, mut players: Players) {
        let mut state = self.state.lock().unwrap();
        players.lock_teams = state.lock_teams;
        if !seat::alliances_follow_teams(&players) {
            state.teams_held = false;
        }
        // The players record does not carry the time notes (kind 31 does).
        players.time_notes = state.observer.players.time_notes.clone();
        state.observer.set_players(players);
    }

    /// The engine's time notes, which the players record does not carry.
    /// The whole list of the engine's non-default control groups.
    pub fn set_control_groups(&self, groups: &[(u32, u32, u32)]) {
        let observer = &mut self.state.lock().unwrap().observer;
        observer.control_groups = groups
            .iter()
            .map(|(id, group, group2)| (*id, [*group, *group2]))
            .collect();
    }

    pub fn set_time_notifications(&self, notes: Vec<TimeNote>) {
        let observer = &mut self.state.lock().unwrap().observer;
        let mut players = observer.players.clone();
        players.time_notes = notes;
        observer.set_players(players);
    }

    /// Whether the match locks the teams (`settings.LockTeams`), which
    /// widens who is told of a tribute.
    pub fn set_lock_teams(&self, json: &[u8]) {
        let locked = serde_json::from_slice::<serde_json::Value>(json)
            .ok()
            .and_then(|value| value.pointer("/settings/LockTeams")?.as_bool())
            .unwrap_or(false);
        let mut state = self.state.lock().unwrap();
        state.lock_teams = locked;
        // The init attributes start a match, before its first players
        // record.
        state.teams_held = true;
        // The seats read the players through the observer, so its copy
        // carries the lock; the equality check flags it for the streams.
        if state.observer.players.lock_teams != locked {
            let mut players = state.observer.players.clone();
            players.lock_teams = locked;
            state.observer.set_players(players);
        }
    }

    pub fn teams_held(&self) -> bool {
        self.state.lock().unwrap().teams_held
    }

    pub fn observer_guard(self: &Arc<Self>) -> ObserverGuard {
        self.observers.fetch_add(1, Ordering::Relaxed);
        ObserverGuard(Arc::clone(self))
    }

    pub fn observer_count(&self) -> u32 {
        self.observers.load(Ordering::Relaxed)
    }

    /// Each player's name, civ and colour, gaia left out, for the lobby.
    pub fn lobby_players(&self) -> Vec<MetaPlayer> {
        let state = self.state.lock().unwrap();
        meta_players(&state)
    }

    /// The latest turn the feed gave, which viewers may not see yet.
    pub fn turn(&self) -> u32 {
        self.state.lock().unwrap().turn
    }

    /// Whether a turn of a running match has been encoded: before it there
    /// is no frame for a pause, after the end none.
    pub fn has_turn(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.has_turn && !state.ended
    }

    /// The feed's latest state of `player`, as `feed::Player::state`.
    pub fn player_state(&self, player: u32) -> Option<u8> {
        let state = self.state.lock().unwrap();
        let players = &state.observer.players.players;
        players.get(player as usize).map(|player| player.state)
    }

    /// Each player's state and stances from the feed's latest players record,
    /// indexed by player number (0 is gaia), for the chat's addressees (M31).
    pub fn diplomacy(&self) -> Vec<(u8, Vec<u8>)> {
        let state = self.state.lock().unwrap();
        state
            .observer
            .players
            .players
            .iter()
            .map(|player| (player.state, player.stances.clone()))
            .collect()
    }

    /// The player a defeated seat is shown the vision of (D47), from the
    /// feed's latest players record.
    pub fn team_eyes(&self, player: u32) -> Option<u32> {
        let state = self.state.lock().unwrap();
        seat::team_eyes(&state.observer.players, player as usize).map(|eyes| eyes as u32)
    }

    pub fn set_economy(&self, economy: &[Economy]) {
        self.state.lock().unwrap().observer.economy = economy.iter().map(Funds::new).collect();
    }

    pub fn set_territory(&self, territory: Territory) {
        self.state.lock().unwrap().observer.set_territory(territory);
    }

    pub fn set_sounds(&self, sounds: Vec<Sound>) {
        self.state.lock().unwrap().observer.sounds = sounds;
    }

    pub fn set_projectiles(&self, launches: Vec<Launch>, removals: Vec<u32>) {
        let observer = &mut self.state.lock().unwrap().observer;
        observer.launches = launches;
        observer.removals = removals;
    }

    /// Applies a kind 24 record's runs to every player's grid, which the
    /// observers' stream carries whole.
    pub fn set_los(&self, side: u32, players: Vec<Vec<LosRun>>) {
        let state = &mut *self.state.lock().unwrap();
        if state.los.side != side {
            state.los = Los {
                side,
                players: BTreeMap::new(),
            };
        }
        let vertices = side as usize * side as usize;
        for (index, runs) in players.into_iter().enumerate() {
            let grid = state
                .los
                .players
                .entry(index as u32 + 1)
                .or_insert_with(|| vec![0; vertices]);
            for run in runs {
                let start = run.start as usize;
                grid[start..start + run.states.len()].copy_from_slice(&run.states);
            }
        }
        state.observer.los = state.los.clone();
    }

    /// The players who have the game paused, as a kind 26 record says;
    /// `/` is shown it `delay` late, the seats at once.
    pub fn set_pause(&self, players: &[i8]) {
        let mut state = self.state.lock().unwrap();
        // A resume starts the watchdog's count afresh.
        state.feed_activity = Some(Instant::now());
        if !state.has_turn || state.ended {
            return;
        }
        let turn = state.turn;
        for seat in &mut state.seats {
            if let Err(error) = seat.pause(turn, players) {
                eprintln!("cannot encode a pause: {error}");
            }
        }
        if let Some(explored) = &mut state.explored {
            if let Err(error) = explored.pause(turn, players) {
                eprintln!("cannot encode a pause: {error}");
            }
            self.drain_explored(&mut state);
            return;
        }
        match state.observer.pause(turn, players) {
            Ok(op) => self.queue(&mut state, Release::Pause(op)),
            Err(error) => eprintln!("cannot encode a pause: {error}"),
        }
    }

    /// What the session GUI tells a player (an attack, a stopped
    /// auto-queue): at once to that player's seat only. Observers have no
    /// player, and it is never keyframed, as a flare.
    pub fn notify(&self, notes: &[Note]) {
        let mut state = self.state.lock().unwrap();
        if !state.has_turn || state.ended {
            return;
        }
        let turn = state.turn;
        for note in notes {
            for seat in &mut state.seats {
                if seat.player == u32::from(note.player)
                    && let Err(error) = seat.notification(turn, note)
                {
                    eprintln!("cannot encode a notification: {error}");
                }
            }
        }
    }

    /// The chat lines of the other typed notifications (defeat, phase,
    /// tribute, ...), as the engine's JSON array: at once to the seats each
    /// is for and, with the delay, to the observers. Never keyframed.
    pub fn notify_events(&self, json: &[u8]) {
        let Ok(serde_json::Value::Array(events)) = serde_json::from_slice(json) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if !state.has_turn || state.ended {
            return;
        }
        let turn = state.turn;
        for mut event in events {
            let Some((seats, observers)) = route_event(&state, &mut event) else {
                continue;
            };
            // A seat names entities by handle, and the bribed unit is
            // usually not seen yet, so each told seat gets its own copy.
            let entity = event.get("entity").and_then(serde_json::Value::as_u64);
            if event.get("type").and_then(serde_json::Value::as_str) == Some("spy-response")
                && let Some(entity) = entity.filter(|entity| *entity != 0)
            {
                for index in seats {
                    let handle = state.seats[index].handle_of(entity as u32);
                    let mut copy = event.clone();
                    copy["entity"] = handle.into();
                    if let Err(error) = state.seats[index].event(turn, &copy.to_string()) {
                        eprintln!("cannot encode an event: {error}");
                    }
                }
                continue;
            }
            self.send_event(&mut state, turn, &event.to_string(), &seats, observers);
        }
    }

    /// A line about the network (a seat left or rejoined, a kick or ban),
    /// for every seat at once and the observers after the delay.
    pub fn notify_network(&self, json: String) {
        let mut state = self.state.lock().unwrap();
        if !state.has_turn || state.ended {
            return;
        }
        let turn = state.turn;
        let seats: Vec<usize> = (0..state.seats.len()).collect();
        self.send_event(&mut state, turn, &json, &seats, true);
    }

    fn send_event(
        &self,
        state: &mut State,
        turn: u32,
        text: &str,
        seats: &[usize],
        observers: bool,
    ) {
        for index in seats {
            if let Err(error) = state.seats[*index].event(turn, text) {
                eprintln!("cannot encode an event: {error}");
            }
        }
        if !observers {
            return;
        }
        if let Some(explored) = &mut state.explored {
            if let Err(error) = explored.event(turn, text) {
                eprintln!("cannot encode an event: {error}");
            }
            self.drain_explored(state);
            return;
        }
        match state.observer.event_message(turn, text) {
            Ok(op) => self.queue(state, Release::Flare(op)),
            Err(error) => eprintln!("cannot encode an event: {error}"),
        }
    }

    /// How long the engine took per turn over the last second, for the
    /// frame stats. It is queued like the turns around it for the observers,
    /// so a viewer sees the cost of the turn on screen, and it is stored, so
    /// a replay shows it. The seats, who see their game at once, get it at
    /// once.
    pub fn set_sim_time(&self, microseconds: u32) {
        let mut state = self.state.lock().unwrap();
        if !state.has_turn || state.ended {
            return;
        }
        let turn = state.turn;
        for seat in &mut state.seats {
            if let Err(error) = seat.sim_time(turn, microseconds) {
                eprintln!("cannot encode a simulation time: {error}");
            }
        }
        if let Some(explored) = &mut state.explored {
            if let Err(error) = explored.sim_time(turn, microseconds) {
                eprintln!("cannot encode a simulation time: {error}");
            }
            self.drain_explored(&mut state);
            return;
        }
        match state.observer.sim_time_message(turn, microseconds) {
            Ok(op) => self.queue(&mut state, Release::Flare(op)),
            Err(error) => eprintln!("cannot encode a simulation time: {error}"),
        }
    }

    /// The pause ledger, as `Play`'s ledger reports it. Unlike `set_pause`
    /// it leaves `feed_activity` alone: no engine pauser changed.
    pub fn set_ledger(&self, entries: &[pause::Entry]) {
        let mut state = self.state.lock().unwrap();
        if !state.has_turn || state.ended {
            return;
        }
        let turn = state.turn;
        for seat in &mut state.seats {
            if let Err(error) = seat.ledger(turn, entries) {
                eprintln!("cannot encode a ledger: {error}");
            }
        }
        if let Some(explored) = &mut state.explored {
            if let Err(error) = explored.ledger(turn, entries) {
                eprintln!("cannot encode a ledger: {error}");
            }
            self.drain_explored(&mut state);
            return;
        }
        match state.observer.set_ledger(turn, entries) {
            Ok(op) => self.queue(&mut state, Release::Pause(op)),
            Err(error) => eprintln!("cannot encode a ledger: {error}"),
        }
    }

    /// A player's flare: at once to its mutual allies' seats and, with the
    /// delay, to the observers. It is never keyframed (D51).
    pub fn flare_from_seat(&self, player: u32, x: f32, z: f32) -> Result<(), &'static str> {
        let mut state = self.state.lock().unwrap();
        check_flare(&state, x, z)?;
        if !state.flares.allow(
            &[(flare::Sender::Seat(player), flare::MAXIMUM)],
            Instant::now(),
        ) {
            return Err("too many flares");
        }
        let turn = state.turn;
        let sender = player as u8;
        let allies: Vec<usize> = state
            .seats
            .iter()
            .enumerate()
            .filter(|(_, seat)| {
                seat.player != player
                    && seat::mutual_ally(
                        &state.observer.players,
                        player as usize,
                        seat.player as usize,
                    )
            })
            .map(|(index, _)| index)
            .collect();
        for index in allies {
            if let Err(error) = state.seats[index].flare(turn, sender, x, z) {
                eprintln!("cannot encode a flare: {error}");
            }
        }
        if let Some(explored) = &mut state.explored {
            if let Err(error) = explored.flare(turn, sender, x, z) {
                eprintln!("cannot encode a flare: {error}");
            }
            self.drain_explored(&mut state);
            return Ok(());
        }
        match state.observer.flare_message(turn, sender, x, z) {
            Ok(op) => self.queue(&mut state, Release::Flare(op)),
            Err(error) => eprintln!("cannot encode a flare: {error}"),
        }
        Ok(())
    }

    /// An observer's flare, for the other observers only, at once and
    /// never stored (D51).
    pub fn flare_from_observer(
        &self,
        visitor: &str,
        name: Option<&str>,
        x: f32,
        z: f32,
    ) -> Result<(), &'static str> {
        let mut state = self.state.lock().unwrap();
        check_flare(&state, x, z)?;
        if !state.flares.allow(
            &[
                (flare::Sender::Observers, flare::MAXIMUM),
                (flare::Sender::Visitor(visitor.to_string()), flare::MAXIMUM),
            ],
            Instant::now(),
        ) {
            return Err("too many flares");
        }
        state.shown.side(envelope_flare(x, z, name).into());
        Ok(())
    }

    pub fn set_template_icon(&self, template: u32, icon: Icon) {
        let observer = &mut self.state.lock().unwrap().observer;
        let index = observer.intern_icon(icon);
        observer.template_icons.insert(template, index);
    }

    pub fn set_terrain(&self, id: u64, bytes: Vec<u8>, terrain: Arc<Terrain>) {
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
        state.ground = Some(Ground::new(&terrain));
        state.terrain_source = Some(terrain);
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
    /// read from the history, while it keeps them. A seat gets only the
    /// files its stream named.
    pub fn data(&self, hash: u64, view: View) -> Option<Arc<[u8]>> {
        {
            let state = self.state.lock().unwrap();
            if let View::Seat(player) = view {
                let seat = state.seats.iter().find(|seat| seat.player == player)?;
                if !seat.named_data(hash) {
                    return None;
                }
                return state.data_files.get(&hash).map(Arc::clone);
            }
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
        stored_data(self.history.as_deref()?, hash)
    }

    /// A seat's terrain as revealed so far.
    pub fn seat_terrain(&self, player: u32) -> Option<Vec<u8>> {
        let state = self.state.lock().unwrap();
        let seat = state.seats.iter().find(|seat| seat.player == player)?;
        let source = state.terrain_source.as_deref()?;
        let water = state.seat_water.as_deref()?;
        Some(seat.terrain_resource(source, water))
    }

    /// The feed's id a seat's handle last named this match.
    pub fn seat_entity(&self, player: u32, handle: u32) -> Option<u32> {
        let state = self.state.lock().unwrap();
        state
            .seats
            .iter()
            .find(|seat| seat.player == player)?
            .entity(handle)
    }

    /// The seat's handles of the feed's ids, sorted; an id the seat does
    /// not hold this turn is left out.
    pub fn seat_handles(&self, player: u32, ids: &[u32]) -> Vec<u32> {
        let state = self.state.lock().unwrap();
        let Some(seat) = state.seats.iter().find(|seat| seat.player == player) else {
            return Vec::new();
        };
        let mut handles: Vec<u32> = ids.iter().filter_map(|id| seat.handle(*id)).collect();
        handles.sort_unstable();
        handles
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
        stored_terrain(self.history.as_deref()?, id)
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

    /// The engine's replay of a hosted hub's match, kept in the history
    /// once its run dir is pruned. A match with no turn has no history yet,
    /// and nothing to replay.
    pub fn store_replay(&self, files: &[(&str, Vec<u8>)]) {
        let state = self.state.lock().unwrap();
        let Some(&(_, match_id)) = state.uuids.last() else {
            return;
        };
        for (name, bytes) in files {
            self.store(&state, |history, protect| {
                history.write_replay(match_id, name, bytes, protect)
            });
        }
    }

    /// A keepalive: the engine is up while the game is paused.
    pub fn feed_alive(&self) {
        self.state.lock().unwrap().feed_activity = Some(Instant::now());
    }

    pub fn feed_activity(&self) -> Option<Instant> {
        self.state.lock().unwrap().feed_activity
    }

    /// A capture of the match being encoded; `/` is shown it with the
    /// turns, `delay` late. Ignored before the match's first turn.
    pub fn set_summary(self: &Arc<Self>, turn: u32, is_final: bool, full: Arc<[u8]>) {
        // Before the envelope is built: a capture outside a match costs
        // nothing.
        let teams_held = {
            let state = self.state.lock().unwrap();
            if !state.has_turn || state.ended {
                return;
            }
            state.teams_held
        };
        if is_final {
            self.state.lock().unwrap().final_seen = true;
        }
        let Some(raw) = summary_envelope(turn, is_final, teams_held, &full) else {
            return;
        };
        let raw: Arc<[u8]> = raw.into();
        let entry = {
            let mut state = self.state.lock().unwrap();
            if !state.has_turn || state.ended {
                return;
            }
            let entry = Arc::new(SummaryEntry {
                match_id: state.match_id,
                turn,
                inner: Mutex::new(EntryState {
                    raw: Some(Arc::clone(&raw)),
                    brotli: None,
                    released: false,
                    stored: self.history.is_none(),
                }),
            });
            self.queue(&mut state, Release::Summary(Arc::clone(&entry)));
            entry
        };
        let hub = Arc::clone(self);
        spawn_brotli(raw, move |brotli| {
            let write = {
                let mut inner = entry.inner.lock().unwrap();
                inner.brotli = Some(brotli);
                inner.raw = None;
                inner.released && !inner.stored
            };
            if !write {
                return;
            }
            let protect = {
                let state = hub.state.lock().unwrap();
                // A match the history has dropped since gets no file back.
                let previous = state
                    .shown
                    .previous
                    .as_ref()
                    .map_or(0, |archive| archive.match_id);
                if ![state.match_id, state.shown.match_id, previous].contains(&entry.match_id) {
                    return;
                }
                [
                    state.match_id,
                    state
                        .shown
                        .previous
                        .as_ref()
                        .map_or(0, |archive| archive.match_id),
                ]
            };
            store_summary(hub.history.as_deref(), &entry, &protect);
        });
    }

    /// The capture at or before `turn` of a match as `/` may see it: the
    /// shown or the previous match from memory, any other from the
    /// history. The history is written only as a capture is shown, so it
    /// holds nothing ahead of the stream.
    pub fn summary(&self, match_id: u64, turn: Option<u32>) -> Option<SummaryBody> {
        {
            let state = self.state.lock().unwrap();
            let previous = state
                .shown
                .previous
                .as_ref()
                .filter(|archive| archive.match_id == match_id)
                .and_then(|archive| summary_body(&archive.summaries, turn));
            if previous.is_some() {
                return previous;
            }
            if match_id == state.shown.match_id
                && state.shown.has_turn
                && let Some(body) = summary_body(&state.shown.summaries, turn)
            {
                return Some(body);
            }
        }
        stored_summary(self.history.as_deref()?, match_id, turn)
    }

    /// Called when the feed is gone. A feed that never sent a turn ends
    /// nothing.
    pub fn end_match(&self) {
        let state = &mut *self.state.lock().unwrap();
        if !state.has_turn || state.ended {
            return;
        }
        let common = state.common();
        let terrain = state.terrain.clone();
        let data_files = std::mem::take(&mut state.data_files);
        let turn = state.turn;
        // Exactly one End is queued, the explored stream's in its mode.
        let release = match &mut state.explored {
            Some(explored) => {
                explored.end(&common, turn, terrain, data_files);
                None
            }
            None => {
                let (ended, visual_tail, detail_tail) = state.observer.finish(&common, turn);
                Some(Release::End {
                    match_id: state.match_id,
                    visual_tail,
                    detail_tail,
                    ended,
                    terrain,
                    data_files,
                })
            }
        };
        for seat in &mut state.seats {
            seat.end(&common, turn, None, HashMap::new());
        }
        state.ended = true;
        self.store_meta(state, true);
        println!("match {:016x} ended at turn {}", state.match_id, state.turn);
        match release {
            Some(release) => self.queue(state, release),
            None => self.drain_explored(state),
        }
    }

    /// Queues what the explored stream made, with the observers' delay.
    fn drain_explored(&self, state: &mut State) {
        let Some(explored) = &mut state.explored else {
            return;
        };
        for release in std::mem::take(&mut explored.outbox) {
            self.queue(state, release);
        }
    }

    /// The host's observer mode (D56), set once before the feed starts.
    pub fn set_observer_mode(&self, explored: bool, ai_players: Vec<u32>) {
        let mut state = self.state.lock().unwrap();
        if state.has_turn {
            return;
        }
        state.explored = explored.then(|| {
            let mut seat = Seat::explored();
            seat.set_ai_players(ai_players);
            seat
        });
        state.explored_shown = explored.then(ShownReveal::new);
    }

    /// The partial terrain observers have been shown so far, while an
    /// explored-mode match runs; None once it ended, when the whole terrain
    /// is theirs, and for any other match.
    pub fn explored_terrain(&self, id: u64) -> Option<Message> {
        let state = &mut *self.state.lock().unwrap();
        if state.shown.ended.is_some() || !state.shown.has_turn {
            return None;
        }
        if state.terrain.as_ref().map(|(known, _)| *known) != Some(id) {
            return None;
        }
        let terrain = state.terrain_source.clone()?;
        let water = state.seat_water.clone()?;
        Some(state.explored_shown.as_mut()?.resource(&terrain, &water))
    }

    fn queue(&self, state: &mut State, release: Release) {
        if !state.shown.has_turn
            && matches!(release, Release::Turn { .. })
            && first_turn_due(&state.pending).is_none()
        {
            let _ = state.shown.visual.sender.send(envelope_starts(state.delay));
        }
        state
            .pending
            .push_back((Instant::now() + state.delay, release));
        self.queued.notify_one();
    }

    /// Advances the shadow of the explored reveal with a released turn.
    fn follow_reveal(&self, state: &mut State, start: bool, growth: Option<&seat::ExploredGrowth>) {
        let (Some(shadow), Some(terrain), Some(water)) = (
            state.explored_shown.as_mut(),
            state.terrain_source.as_deref(),
            state.seat_water.as_deref(),
        ) else {
            return;
        };
        if start {
            *shadow = ShownReveal::new();
        }
        if let Some(growth) = growth {
            shadow.grow(growth, terrain, water);
        }
    }

    /// Shows viewers what the encoder made once it is due, until `retire`.
    pub fn run_publisher(&self) {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.retired && state.pending.is_empty() {
                return;
            }
            let now = Instant::now();
            while let Some((due, _)) = state.pending.front()
                && *due <= now
            {
                let (_, release) = state.pending.pop_front().unwrap();
                if let Release::Turn {
                    start, explored, ..
                } = &release
                {
                    self.follow_reveal(&mut state, start.is_some(), explored.as_deref());
                }
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

    /// The shown thumbnail snapshot and the terrain under it; None until
    /// both exist.
    pub fn thumb(&self) -> Option<(Arc<Thumb>, Arc<Terrain>)> {
        let state = self.state.lock().unwrap();
        Some((
            Arc::clone(state.shown.thumb.as_ref()?),
            Arc::clone(state.terrain_source.as_ref()?),
        ))
    }

    /// The host's per-match delay (M38). Queued releases keep the due time
    /// they were queued with, so a later change would reorder them; it is
    /// therefore refused once anything is queued.
    pub fn set_delay(&self, delay: Duration) {
        let mut state = self.state.lock().unwrap();
        if state.pending.is_empty() && !state.has_turn {
            state.delay = delay;
        }
    }

    pub fn set_meta_source(&self, source: MetaSource) {
        self.state.lock().unwrap().meta_source = Some(source);
    }

    /// Writes the history's lobby record of the encoded match.
    fn store_meta(&self, state: &State, with_players: bool) {
        let Some(history) = &self.history else {
            return;
        };
        let source = state.meta_source.clone().unwrap_or_default();
        let players = if with_players {
            meta_players(state)
        } else {
            Vec::new()
        };
        // Player states are the feed's: 1 defeated, 2 won.
        let decided = state.final_seen
            || state
                .observer
                .players
                .players
                .iter()
                .skip(1)
                .any(|player| player.state != 0);
        let winners = state
            .observer
            .players
            .players
            .iter()
            .skip(1)
            .filter(|player| player.state == 2)
            .map(|player| player.name.clone())
            .collect();
        let meta = Meta {
            map_name: source.map_name,
            host: source.host,
            players,
            preview: source.preview,
            ai_known: true,
            // Only the end writes it, so a match cut short stays undecided.
            decided: with_players.then_some(decided),
            winners: if with_players { winners } else { Vec::new() },
            // At the start no turn has run, and `first_turn` is not set yet.
            duration_secs: if with_players {
                (f64::from(state.turn.saturating_sub(state.first_turn)) * 0.2) as u32
            } else {
                0
            },
        };
        let protect = [
            state.shown.match_id,
            state
                .shown
                .previous
                .as_ref()
                .map_or(0, |archive| archive.match_id),
        ];
        if let Err(error) = history
            .lock()
            .unwrap()
            .write_meta(state.match_id, &meta, &protect)
        {
            eprintln!(
                "cannot store match {:016x}'s record: {error}",
                state.match_id
            );
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
        let start = state.new_feed;
        if start {
            state.match_id = new_match_id();
            let uuid = state.next_uuid.take().unwrap_or_else(MatchId::new_v4);
            state.uuids.push((uuid, state.match_id));
            println!("match {:016x} is {uuid}", state.match_id);
            // A running match is not complete on disk; once ended it is the
            // newest stored match, so oldest-first eviction reaches it last
            // anyway.
            if let Some(history) = &self.history {
                // As `store`: the matches viewers may still ask for.
                let protect = [
                    state.shown.match_id,
                    state
                        .shown
                        .previous
                        .as_ref()
                        .map_or(0, |archive| archive.match_id),
                ];
                let mut history = history.lock().unwrap();
                history.set_live(state.match_id, true);
                if let Err(error) = history.start_match(state.match_id, uuid, &protect) {
                    eprintln!("cannot store match {:016x}: {error}", state.match_id);
                }
            }
            self.store_meta(state, false);
            state.ended = false;
            state.final_seen = false;
            state.first_turn = turn;
            let players = state.seat_players.clone();
            state.seats.retain(|seat| players.contains(&seat.player));
            for seat in &mut state.seats {
                seat.reset();
            }
            if let Some(explored) = &mut state.explored {
                explored.reset();
            }
        }
        let common = state.common();
        if (!state.seats.is_empty() || state.explored.is_some())
            && state.seat_water.is_none()
            && let Some(terrain) = &state.terrain_source
        {
            state.seat_water = Some(Arc::new(SeatWater::new(terrain)));
        }
        // The seats read the turn as the feed has it, before the
        // observers' encoding clears its sounds and projectiles.
        {
            let input = SeatInput {
                turn,
                entities,
                details,
                feed: &state.observer,
                los: &state.los,
                terrain: state.terrain_source.as_deref(),
                water: state.seat_water.as_deref(),
                ground: state.ground.as_ref(),
            };
            for seat in &mut state.seats {
                seat.publish(&common, &input, &mut state.actors, start)?;
            }
            if let Some(explored) = &mut state.explored {
                explored.publish(&common, &input, &mut state.actors, start)?;
            }
        }
        let (visual, detail) = state.observer.encode_turn(
            &common,
            turn,
            entities,
            details,
            state.ground.as_ref(),
            start,
        )?;
        if start {
            self.store_match_files(state);
        }
        state.turn = turn;
        state.has_turn = true;
        state.new_feed = false;
        state.feed_activity = Some(Instant::now());
        if state.explored.is_some() {
            // The full stream was needed for the seats' tables only.
            self.drain_explored(state);
        } else {
            let release = Release::Turn {
                start: start.then_some(state.match_id),
                visual,
                detail,
                explored: None,
            };
            self.queue(state, release);
        }
        if start || turn.is_multiple_of(THUMB_TURNS) {
            let thumb = Arc::new(state.thumb());
            self.queue(state, Release::Thumb(thumb));
        }
        Ok(())
    }

    /// A segment of the current or the previous match, and whether it is
    /// closed.
    /// `view`: which stream; a seat's is never stored.
    pub fn segment(&self, match_id: u64, index: usize, view: View) -> Option<(Message, bool)> {
        self.track_segment(match_id, index, false, view)
    }

    /// A detail stream segment, as `segment`.
    pub fn detail_segment(
        &self,
        match_id: u64,
        index: usize,
        view: View,
    ) -> Option<(Message, bool)> {
        self.track_segment(match_id, index, true, view)
    }

    fn track_segment(
        &self,
        match_id: u64,
        index: usize,
        detail: bool,
        view: View,
    ) -> Option<(Message, bool)> {
        match self.memory_segment(match_id, index, detail, view) {
            Lookup::Found(bytes, closed) => return Some((bytes, closed)),
            Lookup::OnDisk if view != View::Observer => return None,
            Lookup::OnDisk => {}
            Lookup::Missing => return None,
        }
        stored_segment(self.history.as_deref()?, match_id, detail, index)
    }

    fn memory_segment(&self, match_id: u64, index: usize, detail: bool, view: View) -> Lookup {
        let locked = self.state.lock().unwrap();
        let Some(state) = locked.view(view) else {
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

    pub fn match_of(&self, uuid: MatchId) -> Option<u64> {
        let state = self.state.lock().unwrap();
        state
            .uuids
            .iter()
            .find(|(known, _)| *known == uuid)
            .map(|(_, match_id)| *match_id)
    }

    pub fn uuid_of(&self, match_id: u64) -> Option<MatchId> {
        let state = self.state.lock().unwrap();
        state
            .uuids
            .iter()
            .find(|(_, known)| *known == match_id)
            .map(|(uuid, _)| *uuid)
    }

    /// Whether nothing of the feed is left to show: the end, if there was a
    /// match, has been released, so its history is complete. False while a
    /// feed still has to deliver its end, which `end_match` queues.
    pub fn settled(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.pending.is_empty() && (state.ended || !state.has_turn)
    }

    /// Ends the publisher thread, which holds the hub alive.
    pub fn retire(&self) {
        self.state.lock().unwrap().retired = true;
        self.queued.notify_one();
    }

    /// Whether observers have not yet reached the end of `match_id`. The
    /// second clause covers a broadcast match whose end is still in the
    /// delay after the encoder moved on.
    pub fn observers_running(&self, match_id: u64) -> bool {
        let state = self.state.lock().unwrap();
        let previous = state
            .shown
            .previous
            .as_ref()
            .map(|archive| archive.match_id);
        (state.match_id == match_id && previous != Some(match_id))
            || (state.shown.match_id == match_id
                && state.shown.has_turn
                && state.shown.ended.is_none())
    }

    /// The match being shown as `(id, segments, live)`.
    pub fn shown_info(&self) -> Option<(u64, usize, bool)> {
        let id = self.state.lock().unwrap().shown.match_id;
        if id == 0 {
            return None;
        }
        self.match_info(id)
            .map(|(segments, live)| (id, segments, live))
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
    /// `view` as for `segment`; None when there is no such seat.
    pub fn subscribe(&self, view: View) -> Option<(Vec<Message>, broadcast::Receiver<Message>)> {
        let locked = self.state.lock().unwrap();
        let state = locked.view(view)?;
        let first = if let Some(ended) = &state.ended {
            let since = ended.at.elapsed().as_millis() as u64;
            vec![
                envelope_join(&ended.keyframe, &[]).into(),
                envelope_ended(&encode_ended(state.match_id, since)).into(),
            ]
        } else if state.has_turn {
            vec![state.visual.join()]
        } else if let Some(due) = first_turn_due(&locked.pending).filter(|_| view == View::Observer)
        {
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
        view: View,
    ) -> Option<(Vec<Message>, broadcast::Receiver<Message>)> {
        let locked = self.state.lock().unwrap();
        let state = locked.view(view)?;
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
    /// The stream a seat or `/` sees.
    fn view(&self, view: View) -> Option<&Shown> {
        match view {
            View::Observer => Some(&self.shown),
            View::Seat(player) => self
                .seats
                .iter()
                .find(|seat| seat.player == player)
                .map(|seat| &seat.shown),
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
        Release::Turn { .. }
        | Release::Summary(_)
        | Release::Thumb(_)
        | Release::Pause(_)
        | Release::Flare(_)
        | Release::Detail(_) => None,
    })
}

static LAST_MATCH_ID: AtomicU64 = AtomicU64::new(0);

/// Distinct from every id of every hub even if the clock did not move, and
/// ordered by time, which `history.rs` relies on.
fn new_match_id() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64);
    let mut last = LAST_MATCH_ID.load(Ordering::SeqCst);
    loop {
        let id = now.max(last + 1);
        match LAST_MATCH_ID.compare_exchange(last, id, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return id,
            Err(seen) => last = seen,
        }
    }
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

/// An observer's flare, which only the observers' socket carries.
fn envelope_flare(x: f32, z: f32, name: Option<&str>) -> Vec<u8> {
    let name = name.unwrap_or("");
    let mut out = Writer::default();
    out.u8(ENVELOPE_FLARE);
    out.f32(x);
    out.f32(z);
    out.varint(name.len() as u64);
    out.0.extend_from_slice(name.as_bytes());
    out.0
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
    common: &Common,
    stream: &Stream,
    turn: u32,
    tables: &Tables,
    dots: &BTreeMap<u32, Dot>,
    shots: &BTreeMap<u32, Shot>,
) -> Vec<u8> {
    let mut out = Writer::default();
    out.u8(MESSAGE_KEYFRAME);
    out.varint(u64::from(turn));
    out.u8(WIRE_VERSION);
    out.u64(common.match_id);
    out.f32(common.game_speed);
    out.varint(common.delay.as_millis() as u64);
    out.varint(u64::from(common.first_turn));
    out.varint(u64::from(SEGMENT_TURNS));
    out.varint(u64::from(common.map_size));
    out.u64(common.terrain_id);
    out.varint(u64::from(stream.terrain_revision));
    write_icons(&mut out, tables.icons);
    write_players(&mut out, &stream.players);
    write_territory(&mut out, stream.territory.as_ref());
    write_los(&mut out, &stream.los);
    write_templates(&mut out, tables.templates);
    write_strings(&mut out, tables.strings);
    write_variant_sets(&mut out, tables.variant_sets);
    write_variable_sets(&mut out, tables.variable_sets);
    write_range_sets(&mut out, tables.range_sets);
    write_choices(&mut out, tables.choices);
    out.varint(stream.economy.len() as u64);
    for funds in &stream.economy {
        write_funds(&mut out, funds);
    }
    out.varint(dots.len() as u64);
    let mut last_id = 0;
    for (id, dot) in dots {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        let hitpoints = default_hitpoints(&stream.templates.items, dot.template);
        let capture = capture_of(&stream.captures, *id);
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
    out.varint(stream.paused.len() as u64);
    for player in &stream.paused {
        out.varint(u64::from(*player));
    }
    write_ledger(&mut out, &stream.ledger);
    out.0
}

fn write_ledger(out: &mut Writer, entries: &[pause::Entry]) {
    out.varint(entries.len() as u64);
    for entry in entries {
        out.varint(u64::from(entry.player));
        out.varint(u64::from(entry.budget_ms));
        out.u8(entry.flags);
        out.varint(u64::from(entry.countdown_ms));
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_delta(
    turn: u32,
    new_tables: &Tables,
    new_players: Option<&Players>,
    new_territory: Option<&Territory>,
    new_los: Option<(&Los, &Los)>,
    reveal: Option<(u32, &[u8])>,
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
    match new_los {
        Some((before, now)) => {
            out.u8(1);
            write_los_change(&mut out, before, now);
        }
        None => out.u8(0),
    }
    match reveal {
        Some((revision, reveal)) => {
            out.u8(1);
            out.varint(u64::from(revision));
            out.0.extend_from_slice(reveal);
        }
        None => out.u8(0),
    }
    write_templates(&mut out, new_tables.templates);
    write_strings(&mut out, new_tables.strings);
    write_variant_sets(&mut out, new_tables.variant_sets);
    write_variable_sets(&mut out, new_tables.variable_sets);
    write_range_sets(&mut out, new_tables.range_sets);
    write_choices(&mut out, new_tables.choices);
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
        write_launch(&mut out, shot, shooter, turn);
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
    out.u8(u8::from(funds.training_blocked));
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
        (
            CHANGED_MIRAGE,
            before.mirage != now.mirage || before.parent != now.parent,
        ),
        (CHANGED_CHOSEN, before.chosen != now.chosen),
        (CHANGED_VARIABLES, before.variables != now.variables),
        (CHANGED_RANGES, before.ranges != now.ranges),
        (CHANGED_HITPOINTS, before.hitpoints != now.hitpoints),
        (
            CHANGED_EXACT,
            before.exact != now.exact
                || before.fine != now.fine
                || before.exact_angle != now.exact_angle,
        ),
        (CHANGED_GROUPS, before.groups != now.groups),
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
    out.varint(u64::from(shot.chosen));
}

fn write_shot_flight(out: &mut Writer, shot: &Shot) {
    out.varint(u64::from(shot.speed));
    out.zigzag(i64::from(shot.gravity));
    out.varint(u64::from(shot.projectile_actor));
    out.varint(u64::from(shot.impact_actor));
    out.varint(u64::from(shot.impact_lifetime));
}

/// `shooter` is the shooter as viewers know it, if they do; `turn` is the
/// delta's.
fn write_launch(out: &mut Writer, shot: &Shot, shooter: Option<&Dot>, turn: u32) {
    out.varint(u64::from(turn - shot.launch_turn));
    out.varint(u64::from(shot.shooter));
    let origin = match shooter {
        Some(dot) => {
            out.varint(u64::from(shot.chosen));
            [dot.x, dot.y, dot.z]
        }
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
    if mask & CHANGED_MIRAGE != 0 {
        write_mirage(out, now);
    }
    if mask & CHANGED_CHOSEN != 0 {
        out.varint(u64::from(now.chosen));
    }
    if mask & CHANGED_EXACT != 0 {
        write_exact(out, now);
    }
    if mask & CHANGED_GROUPS != 0 {
        write_groups(out, now);
    }
}

/// Without `exact` set, a zero pose, which a viewer reads as none.
fn write_exact(out: &mut Writer, dot: &Dot) {
    out.u8(u8::from(dot.exact));
    if dot.exact {
        out.zigzag(i64::from(dot.fine[0]));
        out.zigzag(i64::from(dot.fine[1]));
        out.zigzag(i64::from(dot.exact_angle));
    }
}

fn write_groups(out: &mut Writer, dot: &Dot) {
    out.varint(u64::from(dot.groups[0]));
    out.varint(u64::from(dot.groups[1]));
}

fn write_mirage(out: &mut Writer, dot: &Dot) {
    out.varint(u64::from(dot.mirage));
    if dot.mirage != 0 {
        out.varint(u64::from(dot.parent));
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
    out.varint(players.time_notes.len() as u64);
    for note in &players.time_notes {
        out.varint(u64::from(note.end));
        out.varint(note.players.len() as u64);
        for player in &note.players {
            out.zigzag(i64::from(*player));
        }
        out.varint(note.json.len() as u64);
        out.0.extend_from_slice(note.json.as_bytes());
    }
    out.u8(u8::from(players.lock_teams));
    for player in &players.players {
        out.u8(u8::from(player.shared_los));
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

/// Each grid run-length coded, as a keyframe holds it: explored land is
/// a few large areas of one state.
fn write_los(out: &mut Writer, los: &Los) {
    out.varint(u64::from(los.side));
    out.varint(los.players.len() as u64);
    for (player, grid) in &los.players {
        out.varint(u64::from(*player));
        for run in grid.chunk_by(|a, b| a == b) {
            out.varint(run.len() as u64);
            out.u8(run[0]);
        }
    }
}

/// The runs of vertices that changed since `before`, as the hook sends
/// them: changes this close together go into one run.
fn write_los_change(out: &mut Writer, before: &Los, now: &Los) {
    const GAP: usize = 8;
    let players: Vec<(&u32, &Vec<u8>)> = now
        .players
        .iter()
        .filter(|(player, grid)| before.players.get(player) != Some(grid))
        .collect();
    out.varint(players.len() as u64);
    for (player, grid) in players {
        out.varint(u64::from(*player));
        let mut runs: Vec<(usize, usize)> = Vec::new();
        match before
            .players
            .get(player)
            .filter(|old| old.len() == grid.len())
        {
            None => runs.push((0, grid.len())),
            Some(old) => {
                for k in (0..grid.len()).filter(|k| old[*k] != grid[*k]) {
                    match runs.last_mut() {
                        Some(run) if k - run.1 <= GAP => run.1 = k + 1,
                        _ => runs.push((k, k + 1)),
                    }
                }
            }
        }
        out.varint(runs.len() as u64);
        let mut end = 0;
        for (start, stop) in runs {
            out.varint((start - end) as u64);
            out.varint((stop - start) as u64);
            for chunk in grid[start..stop].chunks(4) {
                let packed = chunk
                    .iter()
                    .enumerate()
                    .fold(0u8, |byte, (m, state)| byte | state << (2 * m));
                out.u8(packed);
            }
            end = stop;
        }
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
        for string in [
            details.supply_type,
            details.gather_rates,
            details.dropsite_types,
            details.attack_types,
        ] {
            out.varint(one_based(string));
        }
        out.varint(one_based(details.order_sounds));
        out.varint(one_based(details.garrison_classes));
        out.varint(one_based(details.turret_classes));
        out.varint(one_based(details.heal_classes));
        out.varint(one_based(details.unhealable_classes));
        out.varint(u64::from(details.garrison_size));
        write_obstruction(out, details);
    }
}

fn write_obstruction(out: &mut Writer, details: &TemplateDetails) {
    out.u8(details.placement_flags);
    let Some(obstruction) = &details.obstruction else {
        out.u8(0);
        return;
    };
    out.u8(match obstruction.kind {
        ObstructionKind::Static => 1,
        ObstructionKind::Unit => 2,
        ObstructionKind::Cluster => 3,
    });
    out.zigzag(i64::from(obstruction.width));
    out.zigzag(i64::from(obstruction.depth));
    out.u8(obstruction.flags);
    out.varint(obstruction.clusters.len() as u64);
    for shape in &obstruction.clusters {
        for value in shape {
            out.zigzag(i64::from(*value));
        }
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

fn write_choices(out: &mut Writer, choices: &[[Vec<u32>; 3]]) {
    out.varint(choices.len() as u64);
    for sets in choices {
        for set in sets {
            out.varint(set.len() as u64);
            for string in set {
                out.varint(u64::from(*string));
            }
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
    out.varint(u64::from(dot.chosen));
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
    write_mirage(out, dot);
    out.zigzag(i64::from(dot.step[0]));
    out.zigzag(i64::from(dot.step[1]));
    let mut motion = if dot.grounded { MOTION_GROUNDED } else { 0 };
    if dot.hitpoints != hitpoints {
        motion |= MOTION_HITPOINTS;
    }
    if !capture.is_empty() {
        motion |= MOTION_CAPTURE;
    }
    if dot.exact {
        motion |= MOTION_EXACT;
    }
    if dot.groups != [0, 0] {
        motion |= MOTION_GROUPS;
    }
    out.u8(motion);
    if motion & MOTION_HITPOINTS != 0 {
        out.u8(dot.hitpoints);
    }
    if motion & MOTION_CAPTURE != 0 {
        write_capture(out, capture);
    }
    if motion & MOTION_EXACT != 0 {
        write_exact(out, dot);
    }
    if motion & MOTION_GROUPS != 0 {
        write_groups(out, dot);
    }
}
