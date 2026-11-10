//! Decoder for specula-server's viewer stream and match segments, wire
//! format v13; the layout is documented in `specula-server/src/view.rs`.

use std::collections::BTreeMap;
use std::rc::Rc;

use brotli_decompressor::BrotliDecompressStream;
use brotli_decompressor::BrotliResult;
use brotli_decompressor::BrotliState;
use brotli_decompressor::StandardAlloc;

use specula_assets::bytes::ByteReader;
use specula_assets::wire::TERRAIN_VERSION;
use specula_assets::wire::WIRE_VERSION;

use crate::detail::Details;

pub const ENVELOPE_JOIN: u8 = 1;
pub const ENVELOPE_FRAME: u8 = 2;
pub const ENVELOPE_MORE: u8 = 3;
pub const ENVELOPE_ENDED: u8 = 4;
pub const ENVELOPE_STARTS: u8 = 5;
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
/// `TemplateDetails::components` bit for Health.
pub const COMPONENT_HEALTH: u32 = 1;
pub const COMPONENT_CAPTURABLE: u32 = 2;
/// And for ResourceSupply.
pub const COMPONENT_RESOURCE_SUPPLY: u32 = 4;
/// And for RallyPoint.
pub const COMPONENT_RALLY_POINT: u32 = 1 << 20;
/// The rest of the hook's component bits, `DETAIL_INTERFACES` in order.
pub const COMPONENT_GARRISON_HOLDER: u32 = 1 << 4;
pub const COMPONENT_RESOURCE_GATHERER: u32 = 1 << 6;
pub const COMPONENT_FOUNDATION: u32 = 1 << 7;
pub const COMPONENT_REPAIRABLE: u32 = 1 << 8;
pub const COMPONENT_UNIT_AI: u32 = 1 << 9;
pub const COMPONENT_TRAINER: u32 = 1 << 10;
pub const COMPONENT_PACK: u32 = 1 << 12;
pub const COMPONENT_UPGRADE: u32 = 1 << 13;
pub const COMPONENT_GARRISONABLE: u32 = 1 << 15;
pub const COMPONENT_TURRETABLE: u32 = 1 << 16;
pub const COMPONENT_ATTACK: u32 = 1 << 17;
pub const COMPONENT_BUILDER: u32 = 1 << 21;
pub const COMPONENT_GATE: u32 = 1 << 22;
pub const COMPONENT_ALERT_RAISER: u32 = 1 << 23;
pub const COMPONENT_RESOURCE_DROPSITE: u32 = 1 << 24;
pub const COMPONENT_TURRET_HOLDER: u32 = 1 << 25;
pub const COMPONENT_MARKET: u32 = 1 << 26;
/// `TemplateDetails::flags`: Identity's Undeletable.
pub const TEMPLATE_UNDELETABLE: u32 = 2048;
/// And Identity's Controllable false.
pub const TEMPLATE_UNCONTROLLABLE: u32 = 4096;
/// And VisualActor's SilhouetteDisplay: drawn flat where an occluder hides
/// it.
pub const TEMPLATE_SILHOUETTE_DISPLAY: u32 = 8192;
/// And its SilhouetteOccluder: hides those.
pub const TEMPLATE_SILHOUETTE_OCCLUDER: u32 = 16384;
const SOUND_ENTITY: u8 = 0;
const SOUND_POSITION: u8 = 1;
const SOUND_PLAYER: u8 = 2;
const ANIMATION_ONCE: u8 = 1;
/// A move longer than this in one turn is a teleport (ungarrison, a
/// scripted jump), not walking, so the dot does not slide there. It is well
/// above cavalry at 2x game speed.
const SNAP_UNITS: f32 = 24.0 * 16.0;
/// The simulation's turn, in seconds.
const TURN_SECS: f32 = 0.2;
/// As specula-server keeps a projectile: `PROJECTILE_DECAY_TIME` and a
/// margin for the launch point drawn.
const PROJECTILE_KEEP_SECS: f32 = 35.0;

#[derive(Clone, Copy, Default)]
pub struct Dot {
    /// In 1/16 m.
    pub x: i32,
    pub z: i32,
    /// 0xFFRRGGBB, or 0 when the entity is not on the minimap.
    pub color: u32,
    /// 1-based index into `Scene::icons`, or 0 for none.
    pub icon: u32,
    /// Index into `Scene::players` when `color` is that player's colour,
    /// else 0.
    pub player: u32,
    /// Where the dot was on screen when the current turn arrived, in 1/16 m;
    /// the renderer moves it from here to (x, z) over one turn.
    pub from_x: f32,
    pub from_z: f32,
    /// Absolute height in 1/16 m.
    pub y: i32,
    /// Heading in 1/256 turn, clockwise from +z.
    pub angle: u8,
    /// Height and heading where they were drawn when the current turn
    /// arrived, like `from_x`; the heading in 1/256 turn, within half a
    /// turn of `angle` so it turns the short way.
    pub from_y: f32,
    pub from_angle: f32,
    /// Player index, -1 for none.
    pub owner: i32,
    /// Index into `Scene::templates`.
    pub template: u32,
    /// 1-based index into `Scene::strings`, or 0 for none.
    pub actor: u32,
    /// The actor's variation seed.
    pub seed: u32,
    /// 1-based index into `Scene::variant_sets`, or 0 for no selections.
    pub variants: u32,
    /// 1-based index into `Scene::variable_sets`, or 0 for none.
    pub variables: u32,
    /// 1-based index into `Scene::range_sets`, or 0 for none.
    pub ranges: u32,
    /// 1-based index into `Scene::strings`, or 0 for none.
    pub animation: u32,
    /// Plays once rather than loops.
    pub once: bool,
    /// In 1/256: m/s for walk and run, else a multiplier.
    pub speed: u32,
    /// The turn the animation was selected on.
    pub animation_turn: u32,
    /// The turn `speed` was last changed on, and the animation time in ms
    /// at a speed of 1 played up to it since `animation_turn`. The engine
    /// adds up time * speed each frame, so a new speed applies from now
    /// on, not to the time already played.
    pub speed_turn: u32,
    pub phase: f32,
    /// In ms; 0 when not synced.
    pub sync_repeat: u32,
    pub sync_offset: i32,
    /// The turn the sync was set on.
    pub sync_turn: u32,
    /// How far the model has risen out of the ground, in 1/255; 0 when it
    /// is not offset at all.
    pub construction: u8,
    /// 1-based index into `Scene::strings`, or 0 for none: the sound group
    /// the animation plays at its sound or event point.
    pub sound_group: u32,
    /// Bit p is set when player p sees the entity.
    pub visibility: u16,
    /// 0 without Health, else the hitpoints in 1/255 of the max, rounded
    /// up, at least 1.
    pub hitpoints: u8,
    /// What it moves by each turn until a delta changes it, in 1/16 m.
    pub step: [i32; 2],
    /// Its height is the ground's.
    pub grounded: bool,
}

/// The match's heightmap, which grounded entities take their height from.
pub struct Ground {
    /// The terrain resource it is from.
    pub id: u64,
    /// Vertices per side.
    vertices: i64,
    /// In 1/92 m, row-major, x fastest.
    heights: Vec<u16>,
}

impl Ground {
    pub fn new(id: u64, tiles: u32, heights: Vec<u16>) -> Result<Self, &'static str> {
        let vertices = i64::from(tiles) + 1;
        if tiles == 0 || heights.len() as i64 != vertices * vertices {
            return Err("heightmap of the wrong size");
        }
        Ok(Ground {
            id,
            vertices,
            heights,
        })
    }

    /// From a `/terrain/<id>` resource, read up to its heights.
    pub fn from_resource(id: u64, resource: &[u8]) -> Result<Self, &'static str> {
        let mut cursor = ByteReader(resource);
        if cursor.u8()? != TERRAIN_VERSION {
            return Err("unknown terrain version");
        }
        let tiles = cursor.u32()?;
        // Water level, shallow depth, circular, the light's seventeen f32,
        // the post effect and the water's nine f32.
        cursor.take(9 + 17 * 4 + 1 + 9 * 4)?;
        // The water type and the sky set.
        for _ in 0..2 {
            let length = cursor.take(2)?;
            cursor.take(usize::from(u16::from_le_bytes([length[0], length[1]])))?;
        }
        for _ in 0..cursor.u32()? {
            cursor.take(4)?;
            let length = cursor.take(2)?;
            cursor.take(usize::from(u16::from_le_bytes([length[0], length[1]])))?;
        }
        let vertices = tiles as usize + 1;
        let heights = cursor
            .take(vertices * vertices * 2)?
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Ground::new(id, tiles, heights)
    }

    /// `CTerrain::GetExactGroundLevelFixed` at a position in 1/16 m, in
    /// 1/16 m, exactly as specula-server reckons it.
    pub fn at(&self, x: i32, z: i32) -> i32 {
        const ONE: i64 = 1 << 16;
        // 16.16 metres over `TERRAIN_TILE_SIZE`, which truncates as C++ does.
        let x = i64::from(x) * 4096 / 4;
        let z = i64::from(z) * 4096 / 4;
        let round_to_zero = |value: i64| {
            if value > 0 {
                value >> 16
            } else {
                (value + ONE - 1) >> 16
            }
        };
        let last = self.vertices - 2;
        let xi = round_to_zero(x).clamp(0, last);
        let zi = round_to_zero(z).clamp(0, last);
        let xf = (x - xi * ONE).clamp(0, ONE);
        let zf = (z - zi * ONE).clamp(0, ONE);
        let height = |i: i64, j: i64| i64::from(self.heights[(j * self.vertices + i) as usize]);
        // Halved, as the engine does to stay within its fixed-point range;
        // the division below makes up for it.
        let xf0 = xf / 2;
        let xf1 = (ONE - xf) / 2;
        let near = xf1 * height(xi, zi) + xf0 * height(xi + 1, zi);
        let far = xf1 * height(xi, zi + 1) + xf0 * height(xi + 1, zi + 1);
        // `HEIGHT_UNITS_PER_METRE / 2`.
        let raw = (((ONE - zf) * near) >> 16) + ((zf * far) >> 16);
        let raw = raw / 46;
        (raw + 2048).div_euclid(4096) as i32
    }
}

/// What a delta sends for one entity.
struct Change {
    mask: u64,
    /// The change of the step.
    step: [i32; 2],
    /// Added to the height, with `CHANGED_HEIGHT`.
    dy: i32,
    /// The entity with the fields the mask sends replaced.
    values: Dot,
    /// The capture shares, with `CHANGED_CAPTURE`.
    capture: Vec<u8>,
}

/// A sound group the simulation played during a turn.
#[derive(Clone, Copy)]
pub struct Sound {
    /// Index into `Scene::strings`, a path relative to audio/.
    pub group: u32,
    pub target: SoundTarget,
}

#[derive(Clone, Copy)]
pub enum SoundTarget {
    /// Played on an entity; owner, position (1/16 m) and visibility as of
    /// the call.
    Entity {
        owner: i32,
        x: i32,
        z: i32,
        y: i32,
        visibility: u16,
    },
    /// Heard regardless of LOS.
    Position { x: i32, z: i32, y: i32 },
    /// Only for this player, not positioned.
    Player(i32),
}

/// A projectile, as `CCmpProjectileManager::LaunchProjectile` was given it.
#[derive(Clone, Copy, Default)]
pub struct Shot {
    pub launch_turn: u32,
    /// The entity that shot it, 0 for none; it may be gone.
    pub shooter: u32,
    /// The shooter's as of the launch, as in `Dot`.
    pub owner: i32,
    pub actor: u32,
    pub seed: u32,
    pub variants: u32,
    /// The simulation's launch point and the target, x, y, z in 1/16 m.
    pub launch: [i32; 3],
    pub target: [i32; 3],
    /// Horizontal, in 1/256 m/s.
    pub speed: u32,
    /// In 1/256 m/s^2.
    pub gravity: i32,
    /// 1-based indices into `Scene::strings`, or 0 for none; without a
    /// projectile actor the shooter's actor's `projectile` prop is drawn.
    pub projectile_actor: u32,
    pub impact_actor: u32,
    /// In ms.
    pub impact_lifetime: u32,
}

impl Shot {
    /// The last turn the server keeps it for.
    fn last_turn(&self) -> u32 {
        let dx = (self.target[0] - self.launch[0]) as f32 / 16.0;
        let dz = (self.target[2] - self.launch[2]) as f32 / 16.0;
        let speed = self.speed as f32 / 256.0;
        let flight = if speed > 0.0 {
            dx.hypot(dz) / speed
        } else {
            0.0
        };
        let secs = flight + PROJECTILE_KEEP_SECS + self.impact_lifetime as f32 / 1000.0;
        self.launch_turn
            .saturating_add((secs / TURN_SECS).ceil() as u32)
    }
}

/// (key, selection) indices into `Scene::strings`, sorted by key.
pub type VariantSet = Vec<(u32, u32)>;

/// (name index into `Scene::strings`, value), sorted by name: the entity
/// variables particle emitters read.
pub type VariableSet = Vec<(u32, f32)>;

/// A range circle the selection shows, as `RangeOverlayManager` hands it to
/// the renderer. The strings are 1-based indices into `Scene::strings`, 0
/// for none, relative to art/textures/selection/.
#[derive(Clone, Copy)]
pub struct RangeCircle {
    /// 0 attack, 1 aura, 2 heal.
    pub kind: u8,
    pub radius: f32,
    pub texture: u32,
    pub mask: u32,
    pub thickness: f32,
}

pub type RangeSet = Vec<RangeCircle>;

/// What a template gives the selection overlay, the status bars and the
/// panels, as the hook reads it from the template (`SpeculaFeed.h`, kind
/// 11). Strings are 1-based indices into `Scene::strings`, 0 for none.
#[derive(Clone, Copy)]
pub struct TemplateDetails {
    /// The hook's template flag bits.
    pub flags: u32,
    /// Relative to art/textures/selection/.
    pub overlay_texture: u32,
    pub overlay_mask: u32,
    pub line_thickness: f32,
    /// A circle's radius in both.
    pub overlay_width: f32,
    pub overlay_depth: f32,
    pub footprint_width: f32,
    pub footprint_depth: f32,
    pub footprint_height: f32,
    pub bar_width: f32,
    pub bar_height: f32,
    pub bar_offset: f32,
    pub generic_name: u32,
    pub specific_name: u32,
    /// Relative to art/textures/ui/session/portraits/.
    pub icon: u32,
    pub group_name: u32,
    pub rank: u32,
    /// Token lists, as the template writes them.
    pub classes: u32,
    pub visible_classes: u32,
    /// Relative to audio/.
    pub select_sound: u32,
    /// Identity's Lang and Phenotype tokens, for the select group's
    /// `{lang}` and `{phenotype}`.
    pub lang: u32,
    pub phenotypes: u32,
    /// Bit i for the i-th of the hook's detail components, Health first.
    pub components: u32,
    /// RallyPointRenderer's line, 1-based strings of full VFS paths.
    pub rally_texture: u32,
    pub rally_mask: u32,
    pub rally_thickness: f32,
    /// 0 flat, 1 round, 2 sharp, 3 square.
    pub rally_start_cap: u8,
    pub rally_end_cap: u8,
    /// What a right click's action is picked by, 1-based strings, 0 from a
    /// hook or a stored match without them: ResourceSupply's type (e.g.
    /// `wood.tree`), ResourceGatherer's rate names and ResourceDropsite's
    /// types as token lists, and the Attack types, each `Type` or
    /// `Type:Class,Class` with its RestrictedClasses.
    pub supply_type: u32,
    pub gather_rates: u32,
    pub dropsite_types: u32,
    pub attack_types: u32,
    /// The groups gui/session plays on an order, a 1-based string of
    /// `name=path` tokens, 0 from a hook or a stored match without them.
    pub order_sounds: u32,
}

/// A player's top panel numbers.
#[derive(Clone, Default)]
pub struct Economy {
    pub population: i32,
    pub population_limit: i32,
    pub population_max: i32,
    /// Sorted by code.
    pub resources: Vec<Resource>,
    /// 1-based index into `Scene::strings`, or 0 for none.
    pub phase: u32,
    /// Heroes and the like, as `panelEntities`, in their button order.
    pub panel_entities: Vec<u32>,
}

#[derive(Clone, Copy)]
pub struct Resource {
    /// 1-based index into `Scene::strings`, or 0 for none.
    pub code: u32,
    /// Rounded down, as the top panel shows it.
    pub amount: u32,
    pub gatherers: u32,
}

#[derive(Default)]
pub struct Scene {
    pub turn: u32,
    /// The last keyframe's, which its deltas are read by.
    wire_version: u8,
    /// Names the match's segments.
    pub match_id: u64,
    /// The sim rate the match was set up with; 1.0 is normal speed.
    pub game_speed: f32,
    /// How long after the game the server shows it; 0 for live.
    pub delay_ms: u32,
    /// The turn segment 0 starts at.
    pub first_turn: u32,
    pub segment_turns: u32,
    /// In metres; 0 until the first keyframe.
    pub map_size: u32,
    /// Names `/terrain/<id>`; 0 while the server has none.
    pub terrain_id: u64,
    pub icons: Vec<Icon>,
    /// Index 0 is gaia.
    pub players: Vec<Player>,
    /// When the ceasefire ends, in simulation ms (a turn is `TURN_MS`); 0
    /// for none. It travels with the player table.
    pub ceasefire_end: u32,
    /// Set whenever a message replaces `players`; the reader clears it.
    pub players_changed: bool,
    pub territory: Territory,
    /// Goes up whenever `territory` changes, so each view can tell on its
    /// own whether it has drawn the current one.
    pub territory_generation: u32,
    pub templates: Vec<String>,
    /// By template index; `None` for a template the hook had none for.
    pub template_details: Vec<Option<TemplateDetails>>,
    /// Actor file names, animation names, variant keys and selections, and
    /// the template details' strings.
    pub strings: Vec<String>,
    pub variant_sets: Vec<VariantSet>,
    pub variable_sets: Vec<VariableSet>,
    pub range_sets: Vec<RangeSet>,
    pub dots: BTreeMap<u32, Dot>,
    /// Each player's share of an entity's max capture points, in 1/255, by
    /// id; an entity without Capturable has no entry.
    pub captures: BTreeMap<u32, Vec<u8>>,
    /// By player index; index 0 is gaia.
    pub economy: Vec<Economy>,
    /// Set whenever a message changes `economy`; the reader clears it.
    pub economy_changed: bool,
    /// (old id, new id) of the entities the last delta replaced by others,
    /// for a selection to follow; a keyframe has none.
    pub renames: Vec<(u32, u32)>,
    /// The sounds of the last delta applied; a keyframe has none.
    pub sounds: Vec<Sound>,
    /// By id: in flight, or lying on the ground.
    pub shots: BTreeMap<u32, Shot>,
    /// The heightmap of `terrain_id`, which must be set before a keyframe
    /// naming it is applied.
    pub ground: Option<Rc<Ground>>,
    /// The detail stream's state at `turn`, while the viewer has something
    /// selected; the visual messages leave it alone.
    pub details: Details,
}

/// `ICmpTerritoryManager`'s grid, a tile per `territory::TILE_SIZE`.
#[derive(Default, PartialEq, Eq)]
pub struct Territory {
    pub width: u32,
    pub height: u32,
    /// Row-major, x fastest: owner in `territory::OWNER_MASK`, plus the
    /// connected and blinking bits.
    pub tiles: Vec<u8>,
}

pub struct Icon {
    /// An entry of the icon atlas.
    pub name: String,
    /// In CSS pixels.
    pub size: f32,
}

pub struct Player {
    pub name: String,
    /// 0xFFRRGGBB.
    pub color: u32,
    /// -1 for none.
    pub team: i32,
    pub state: PlayerState,
    /// Towards each player by index.
    pub stances: Vec<Stance>,
    /// Civ code, as in simulation/data/civs/.
    pub civ: String,
    /// `BattleDetection` reports a battle.
    pub battle: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlayerState {
    Active,
    Defeated,
    Won,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stance {
    Enemy,
    Ally,
    Neutral,
}

impl Scene {
    /// `progress` is how far the renderer has moved the dots towards the
    /// current turn, 0..=1.
    pub fn apply(&mut self, message: &[u8], progress: f32) -> Result<(), &'static str> {
        let (&kind, body) = message.split_first().ok_or("empty message")?;
        let mut cursor = ByteReader(body);
        self.sounds.clear();
        self.renames.clear();
        match kind {
            MESSAGE_KEYFRAME => self.apply_keyframe(&mut cursor)?,
            MESSAGE_DELTA => self.apply_delta(&mut cursor, progress)?,
            _ => return Err("unknown message kind"),
        }
        if !cursor.0.is_empty() {
            return Err("trailing bytes");
        }
        Ok(())
    }

    fn apply_keyframe(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        self.turn = cursor.varint_u32()?;
        self.wire_version = cursor.u8()?;
        // Stored matches from before the template order sounds still play.
        if self.wire_version != WIRE_VERSION && self.wire_version != WIRE_VERSION - 1 {
            return Err("unknown wire version, reload the page");
        }
        self.match_id = cursor.u64()?;
        self.game_speed = f32::from_bits(cursor.u32()?);
        self.delay_ms = cursor.varint_u32()?;
        self.first_turn = cursor.varint_u32()?;
        self.segment_turns = cursor.varint_u32()?;
        if self.segment_turns == 0 {
            return Err("segments of no turns");
        }
        self.map_size = cursor.varint_u32()?;
        self.terrain_id = cursor.u64()?;
        if self.terrain_id != 0
            && self
                .ground
                .as_ref()
                .is_none_or(|ground| ground.id != self.terrain_id)
        {
            return Err("keyframe before its terrain");
        }
        self.icons.clear();
        self.read_icons(cursor)?;
        self.read_players(cursor)?;
        self.read_territory(cursor)?;
        self.templates.clear();
        self.template_details.clear();
        self.strings.clear();
        self.variant_sets.clear();
        self.variable_sets.clear();
        self.range_sets.clear();
        self.read_tables(cursor)?;
        let count = cursor.varint_u32()?;
        self.economy = (0..count)
            .map(|_| cursor.economy())
            .collect::<Result<_, _>>()?;
        self.economy_changed = true;
        self.dots.clear();
        self.captures.clear();
        let count = cursor.varint_u32()?;
        let mut id = 0;
        for _ in 0..count {
            id += cursor.varint_u32()?;
            let (dot, capture) = cursor.dot(id, self.turn, &self.template_details)?;
            self.dots.insert(id, dot);
            if !capture.is_empty() {
                self.captures.insert(id, capture);
            }
        }
        self.shots.clear();
        let count = cursor.varint_u32()?;
        let mut id = 0;
        for _ in 0..count {
            id += cursor.varint_u32()?;
            let mut shot = cursor.shot(self.turn)?;
            shot.launch_turn = self
                .turn
                .checked_sub(cursor.varint_u32()?)
                .ok_or("projectile launched before the first turn")?;
            self.shots.insert(id, shot);
        }
        Ok(())
    }

    fn apply_delta(&mut self, cursor: &mut ByteReader, progress: f32) -> Result<(), &'static str> {
        if self.map_size == 0 {
            return Err("delta before the first keyframe");
        }
        // Start the next move from where each dot is drawn now, so a turn
        // that arrives early or late bends the motion instead of jumping it.
        for dot in self.dots.values_mut() {
            dot.from_x += (dot.x as f32 - dot.from_x) * progress;
            dot.from_z += (dot.z as f32 - dot.from_z) * progress;
            dot.from_y += (dot.y as f32 - dot.from_y) * progress;
            dot.from_angle += turn_towards(dot.from_angle, dot.angle) * progress;
        }
        self.turn = cursor.varint_u32()?;
        self.read_icons(cursor)?;
        if cursor.u8()? != 0 {
            self.read_players(cursor)?;
        }
        if cursor.u8()? != 0 {
            self.read_territory(cursor)?;
        }
        self.read_tables(cursor)?;
        if cursor.u8()? != 0 {
            for _ in 0..cursor.varint_u32()? {
                let player = cursor.varint()? as usize;
                let economy = cursor.economy()?;
                if player >= self.economy.len() {
                    self.economy.resize(player + 1, Economy::default());
                }
                self.economy[player] = economy;
            }
            self.economy_changed = true;
        }
        let spawned = cursor.varint_u32()?;
        let destroyed = cursor.varint_u32()?;
        let changed = cursor.varint_u32()?;
        let mut id = 0;
        let mut arrived = Vec::new();
        for _ in 0..spawned {
            id += cursor.varint_u32()?;
            arrived.push((id, cursor.dot(id, self.turn, &self.template_details)?));
        }
        let mut id = 0;
        let mut gone = BTreeMap::new();
        for _ in 0..destroyed {
            id += cursor.varint_u32()?;
            self.captures.remove(&id);
            if let Some(dot) = self.dots.remove(&id) {
                gone.insert(id, dot);
            }
        }
        let mut id = 0;
        let mut changes = BTreeMap::new();
        for _ in 0..changed {
            id += cursor.varint_u32()?;
            let dot = self.dots.get(&id).ok_or("change to an unknown entity")?;
            changes.insert(id, cursor.change(id, dot)?);
        }
        let ground = self.ground.as_deref();
        for (id, dot) in &mut self.dots {
            let change = changes.get(id);
            if change.is_some() || dot.step != [0, 0] {
                advance(dot, change, ground, self.turn);
            }
        }
        for (id, change) in changes {
            if change.mask & CHANGED_CAPTURE == 0 {
                continue;
            }
            if change.capture.is_empty() {
                self.captures.remove(&id);
            } else {
                self.captures.insert(id, change.capture);
            }
        }
        for (id, (dot, capture)) in arrived {
            self.dots.insert(id, dot);
            if !capture.is_empty() {
                self.captures.insert(id, capture);
            }
        }
        let known = |id: u32| self.dots.get(&id).or_else(|| gone.get(&id));
        let count = cursor.varint_u32()?;
        for _ in 0..count {
            let group = cursor.varint_u32()?;
            let target = match cursor.u8()? {
                SOUND_ENTITY => {
                    // The id seeds the engine's random gain and pitch; any
                    // random does here.
                    let (owner, x, z, y, visibility) = known(cursor.varint_u32()?)
                        .map_or((0, 0, 0, 0, 0), |dot| {
                            (dot.owner, dot.x, dot.z, dot.y, dot.visibility)
                        });
                    SoundTarget::Entity {
                        owner: owner + cursor.zigzag()?,
                        x: x + cursor.zigzag()?,
                        z: z + cursor.zigzag()?,
                        y: y + cursor.zigzag()?,
                        visibility: visibility ^ cursor.varint_u32()? as u16,
                    }
                }
                SOUND_POSITION => SoundTarget::Position {
                    x: cursor.zigzag()?,
                    z: cursor.zigzag()?,
                    y: cursor.zigzag()?,
                },
                SOUND_PLAYER => SoundTarget::Player(cursor.zigzag()?),
                _ => return Err("unknown sound target"),
            };
            self.sounds.push(Sound { group, target });
        }
        let turn = self.turn;
        let count = cursor.varint_u32()?;
        let mut id = 0;
        let mut launches = Vec::new();
        for _ in 0..count {
            id += cursor.varint_u32()?;
            launches.push((id, cursor.launch(turn, known)?));
        }
        self.shots.retain(|_, shot| shot.last_turn() >= turn);
        self.shots.extend(launches);
        let count = cursor.varint_u32()?;
        let mut id = 0;
        for _ in 0..count {
            id += cursor.varint_u32()?;
            self.shots.remove(&id);
        }
        let count = cursor.varint_u32()?;
        let mut old = 0;
        for _ in 0..count {
            old += cursor.varint_u32()?;
            let new = u32::try_from(i64::from(old) + cursor.zigzag_i64()?)
                .map_err(|_| "renamed id out of range")?;
            self.renames.push((old, new));
        }
        Ok(())
    }

    /// Templates, strings, variant, variable and range sets, appended to what the scene has.
    fn read_tables(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        let count = cursor.varint_u32()?;
        for _ in 0..count {
            self.templates.push(cursor.text()?);
            let details = match cursor.u8()? {
                0 => None,
                1 => Some(cursor.template_details(self.wire_version)?),
                _ => return Err("unknown template details flag"),
            };
            self.template_details.push(details);
        }
        let count = cursor.varint_u32()?;
        for _ in 0..count {
            self.strings.push(cursor.text()?);
        }
        let count = cursor.varint_u32()?;
        for _ in 0..count {
            let pairs = cursor.varint_u32()?;
            let set = (0..pairs)
                .map(|_| Ok((cursor.varint_u32()?, cursor.varint_u32()?)))
                .collect::<Result<_, &'static str>>()?;
            self.variant_sets.push(set);
        }
        let count = cursor.varint_u32()?;
        for _ in 0..count {
            let pairs = cursor.varint_u32()?;
            let set = (0..pairs)
                .map(|_| Ok((cursor.varint_u32()?, f32::from_bits(cursor.u32()?))))
                .collect::<Result<_, &'static str>>()?;
            self.variable_sets.push(set);
        }
        let count = cursor.varint_u32()?;
        for _ in 0..count {
            let circles = cursor.varint_u32()?;
            let set = (0..circles)
                .map(|_| {
                    Ok(RangeCircle {
                        kind: cursor.u8()?,
                        radius: f32::from_bits(cursor.u32()?),
                        texture: cursor.varint_u32()?,
                        mask: cursor.varint_u32()?,
                        thickness: f32::from_bits(cursor.u32()?),
                    })
                })
                .collect::<Result<_, &'static str>>()?;
            self.range_sets.push(set);
        }
        Ok(())
    }

    fn read_icons(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        let count = cursor.varint_u32()?;
        for _ in 0..count {
            let length = cursor.varint()? as usize;
            let name = std::str::from_utf8(cursor.take(length)?)
                .map_err(|_| "icon name not UTF-8")?
                .to_string();
            let size = f32::from_bits(cursor.u32()?);
            self.icons.push(Icon { name, size });
        }
        Ok(())
    }

    fn read_players(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        let count = cursor.varint()? as usize;
        self.players.clear();
        for _ in 0..count {
            let length = cursor.varint()? as usize;
            let name = std::str::from_utf8(cursor.take(length)?)
                .map_err(|_| "player name not UTF-8")?
                .to_string();
            let color = cursor.u32()?;
            let team = cursor.zigzag()?;
            let state = match cursor.u8()? {
                0 => PlayerState::Active,
                1 => PlayerState::Defeated,
                2 => PlayerState::Won,
                _ => return Err("unknown player state"),
            };
            let stances = cursor
                .take(count)?
                .iter()
                .map(|stance| match stance {
                    0 => Ok(Stance::Enemy),
                    1 => Ok(Stance::Ally),
                    2 => Ok(Stance::Neutral),
                    _ => Err("unknown stance"),
                })
                .collect::<Result<_, _>>()?;
            let length = cursor.varint()? as usize;
            let civ = std::str::from_utf8(cursor.take(length)?)
                .map_err(|_| "civ not UTF-8")?
                .to_string();
            let battle = cursor.u8()? != 0;
            self.players.push(Player {
                name,
                color,
                team,
                state,
                stances,
                civ,
                battle,
            });
        }
        self.ceasefire_end = cursor.varint()? as u32;
        self.players_changed = true;
        Ok(())
    }

    fn read_territory(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        let width = cursor.varint_u32()?;
        let height = cursor.varint_u32()?;
        let count = width as usize * height as usize;
        let mut tiles = Vec::with_capacity(count);
        while tiles.len() < count {
            let run = cursor.varint()? as usize;
            let tile = cursor.u8()?;
            if run == 0 || run > count - tiles.len() {
                return Err("territory run out of bounds");
            }
            tiles.resize(tiles.len() + run, tile);
        }
        let territory = Territory {
            width,
            height,
            tiles,
        };
        if territory != self.territory {
            self.territory = territory;
            self.territory_generation = self.territory_generation.wrapping_add(1);
        }
        Ok(())
    }

    /// A keyframe of the current state, as the server would send it, so a
    /// scene rebuilt from a segment can be handed on as one message.
    pub fn encode_keyframe(&self) -> Vec<u8> {
        let mut out = Writer::default();
        out.u8(MESSAGE_KEYFRAME);
        out.varint(u64::from(self.turn));
        out.u8(WIRE_VERSION);
        out.u64(self.match_id);
        out.u32(self.game_speed.to_bits());
        out.varint(u64::from(self.delay_ms));
        out.varint(u64::from(self.first_turn));
        out.varint(u64::from(self.segment_turns));
        out.varint(u64::from(self.map_size));
        out.u64(self.terrain_id);
        out.varint(self.icons.len() as u64);
        for icon in &self.icons {
            out.varint(icon.name.len() as u64);
            out.0.extend_from_slice(icon.name.as_bytes());
            out.u32(icon.size.to_bits());
        }
        out.varint(self.players.len() as u64);
        for player in &self.players {
            out.varint(player.name.len() as u64);
            out.0.extend_from_slice(player.name.as_bytes());
            out.u32(player.color);
            out.zigzag(player.team);
            out.u8(match player.state {
                PlayerState::Active => 0,
                PlayerState::Defeated => 1,
                PlayerState::Won => 2,
            });
            for stance in &player.stances {
                out.u8(match stance {
                    Stance::Enemy => 0,
                    Stance::Ally => 1,
                    Stance::Neutral => 2,
                });
            }
            out.varint(player.civ.len() as u64);
            out.0.extend_from_slice(player.civ.as_bytes());
            out.u8(u8::from(player.battle));
        }
        out.varint(u64::from(self.ceasefire_end));
        out.varint(u64::from(self.territory.width));
        out.varint(u64::from(self.territory.height));
        for run in self.territory.tiles.chunk_by(|a, b| a == b) {
            out.varint(run.len() as u64);
            out.u8(run[0]);
        }
        out.varint(self.templates.len() as u64);
        for (name, details) in self.templates.iter().zip(&self.template_details) {
            out.text(name);
            let Some(details) = details else {
                out.u8(0);
                continue;
            };
            out.u8(1);
            out.varint(u64::from(details.flags));
            out.varint(u64::from(details.overlay_texture));
            out.varint(u64::from(details.overlay_mask));
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
                out.u32(value.to_bits());
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
                out.varint(u64::from(string));
            }
            out.varint(u64::from(details.components));
            out.varint(u64::from(details.rally_texture));
            out.varint(u64::from(details.rally_mask));
            out.u32(details.rally_thickness.to_bits());
            out.u8(details.rally_start_cap);
            out.u8(details.rally_end_cap);
            for string in [
                details.supply_type,
                details.gather_rates,
                details.dropsite_types,
                details.attack_types,
            ] {
                out.varint(u64::from(string));
            }
            out.varint(u64::from(details.order_sounds));
        }
        out.varint(self.strings.len() as u64);
        for text in &self.strings {
            out.text(text);
        }
        out.varint(self.variant_sets.len() as u64);
        for set in &self.variant_sets {
            out.varint(set.len() as u64);
            for (key, selection) in set {
                out.varint(u64::from(*key));
                out.varint(u64::from(*selection));
            }
        }
        out.varint(self.variable_sets.len() as u64);
        for set in &self.variable_sets {
            out.varint(set.len() as u64);
            for (name, value) in set {
                out.varint(u64::from(*name));
                out.u32(value.to_bits());
            }
        }
        out.varint(self.range_sets.len() as u64);
        for set in &self.range_sets {
            out.varint(set.len() as u64);
            for circle in set {
                out.u8(circle.kind);
                out.u32(circle.radius.to_bits());
                out.varint(u64::from(circle.texture));
                out.varint(u64::from(circle.mask));
                out.u32(circle.thickness.to_bits());
            }
        }
        out.varint(self.economy.len() as u64);
        for economy in &self.economy {
            out.zigzag(economy.population);
            out.zigzag(economy.population_limit);
            out.zigzag(economy.population_max);
            out.varint(economy.resources.len() as u64);
            for resource in &economy.resources {
                out.varint(u64::from(resource.code));
                out.varint(u64::from(resource.amount));
                out.varint(u64::from(resource.gatherers));
            }
            out.varint(u64::from(economy.phase));
            out.varint(economy.panel_entities.len() as u64);
            let mut last_id = 0;
            for id in &economy.panel_entities {
                out.zigzag_i64(i64::from(*id) - i64::from(last_id));
                last_id = *id;
            }
        }
        out.varint(self.dots.len() as u64);
        let mut last_id = 0;
        for (id, dot) in &self.dots {
            out.varint(u64::from(id - last_id));
            last_id = *id;
            out.zigzag(dot.x);
            out.zigzag(dot.z);
            out.zigzag(dot.y);
            out.u8(dot.angle);
            out.u32(dot.color);
            out.varint(u64::from(dot.icon));
            out.varint(u64::from(dot.player));
            out.zigzag(dot.owner);
            out.varint(u64::from(dot.template));
            out.varint(u64::from(dot.actor));
            out.zigzag_i64(i64::from(dot.seed) - i64::from(*id));
            out.varint(u64::from(dot.variants));
            out.varint(u64::from(dot.variables));
            out.varint(u64::from(dot.ranges));
            out.varint(u64::from(dot.animation));
            out.u8(if dot.once { ANIMATION_ONCE } else { 0 });
            out.varint(u64::from(dot.sound_group));
            out.varint(u64::from(dot.speed));
            out.varint(u64::from(self.turn - dot.animation_turn));
            out.varint(u64::from(dot.sync_repeat));
            out.zigzag(dot.sync_offset);
            out.varint(u64::from(self.turn - dot.sync_turn));
            out.u8(dot.construction);
            out.varint(u64::from(dot.visibility));
            out.zigzag(dot.step[0]);
            out.zigzag(dot.step[1]);
            let capture = self.captures.get(id).map_or(&[][..], Vec::as_slice);
            let mut motion = if dot.grounded { MOTION_GROUNDED } else { 0 };
            if dot.hitpoints != default_hitpoints(&self.template_details, dot.template) {
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
                out.u8(capture.len() as u8);
                out.0.extend_from_slice(capture);
            }
        }
        out.varint(self.shots.len() as u64);
        let mut last_id = 0;
        for (id, shot) in &self.shots {
            out.varint(u64::from(id - last_id));
            last_id = *id;
            out.varint(u64::from(shot.shooter));
            out.zigzag(shot.owner);
            out.varint(u64::from(shot.actor));
            out.zigzag_i64(i64::from(shot.seed) - i64::from(shot.shooter));
            out.varint(u64::from(shot.variants));
            for point in [shot.launch, shot.target] {
                out.zigzag(point[0]);
                out.zigzag(point[2]);
                out.zigzag(point[1]);
            }
            out.varint(u64::from(shot.speed));
            out.zigzag(shot.gravity);
            out.varint(u64::from(shot.projectile_actor));
            out.varint(u64::from(shot.impact_actor));
            out.varint(u64::from(shot.impact_lifetime));
            out.varint(u64::from(self.turn - shot.launch_turn));
        }
        out.0
    }
}

/// A delta's update of one entity, as specula-server reckons it.
fn advance(dot: &mut Dot, change: Option<&Change>, ground: Option<&Ground>, turn: u32) {
    let mask = change.map_or(0, |change| change.mask);
    let speed = dot.speed;
    if let Some(change) = change.filter(|_| mask & CHANGED_POSITION != 0) {
        dot.step[0] += change.step[0];
        dot.step[1] += change.step[1];
    }
    let step = dot.step;
    dot.x += step[0];
    dot.z += step[1];
    if mask & CHANGED_GROUNDED != 0 {
        dot.grounded = true;
    }
    if let Some(change) = change.filter(|_| mask & CHANGED_HEIGHT != 0) {
        dot.grounded = false;
        dot.y += change.dy;
    }
    if dot.grounded
        && let Some(ground) = ground
    {
        dot.y = ground.at(dot.x, dot.z);
    }
    let moved = step != [0, 0];
    let angle = dot.angle;
    if moved {
        dot.angle = step_angle(step);
        dot.speed = step_speed(step);
        let distance = (dot.x as f32 - dot.from_x).hypot(dot.z as f32 - dot.from_z);
        if distance > SNAP_UNITS {
            dot.from_x = dot.x as f32;
            dot.from_z = dot.z as f32;
            dot.from_y = dot.y as f32;
        }
    }
    if let Some(change) = change {
        let values = &change.values;
        if mask & CHANGED_ANGLE != 0 {
            dot.angle = values.angle;
        }
        if mask & CHANGED_ANIMATION_SPEED != 0 {
            dot.speed = values.speed;
        }
        if mask & CHANGED_COLOR != 0 {
            dot.color = values.color;
        }
        if mask & CHANGED_ICON != 0 {
            dot.icon = values.icon;
        }
        if mask & CHANGED_PLAYER != 0 {
            dot.player = values.player;
        }
        if mask & CHANGED_ANIMATION != 0 {
            dot.animation = values.animation;
            dot.once = values.once;
            dot.sound_group = values.sound_group;
            dot.animation_turn = turn;
        }
        if mask & CHANGED_SYNC != 0 {
            dot.sync_repeat = values.sync_repeat;
            dot.sync_offset = values.sync_offset;
            dot.sync_turn = turn;
        }
        if mask & CHANGED_OWNER != 0 {
            dot.owner = values.owner;
        }
        if mask & CHANGED_MODEL != 0 {
            dot.template = values.template;
            dot.actor = values.actor;
            dot.seed = values.seed;
        }
        if mask & CHANGED_VARIANTS != 0 {
            dot.variants = values.variants;
        }
        if mask & CHANGED_CONSTRUCTION != 0 {
            dot.construction = values.construction;
        }
        if mask & CHANGED_VISIBILITY != 0 {
            dot.visibility = values.visibility;
        }
        if mask & CHANGED_VARIABLES != 0 {
            dot.variables = values.variables;
        }
        if mask & CHANGED_RANGES != 0 {
            dot.ranges = values.ranges;
        }
        if mask & CHANGED_HITPOINTS != 0 {
            dot.hitpoints = values.hitpoints;
        }
    }
    if mask & CHANGED_ANIMATION != 0 {
        dot.speed_turn = turn;
        dot.phase = 0.0;
    } else if dot.speed != speed {
        dot.phase +=
            turn.saturating_sub(dot.speed_turn) as f32 * TURN_SECS * 1000.0 * speed as f32 / 256.0;
        dot.speed_turn = turn;
    }
    if dot.angle != angle {
        // Keep the start within half a turn, so the heading turns the short
        // way.
        dot.from_angle = f32::from(dot.angle) - turn_towards(dot.from_angle, dot.angle);
    }
}

/// What an entity sent without hitpoints has: full with Health, else none.
fn default_hitpoints(details: &[Option<TemplateDetails>], template: u32) -> u8 {
    let health = details
        .get(template as usize)
        .copied()
        .flatten()
        .is_some_and(|details| details.components & COMPONENT_HEALTH != 0);
    if health { 255 } else { 0 }
}

/// In 1/256 m/s, from 1/16 m per 0.2 s turn. `sqrt` is correctly rounded
/// everywhere, so this is exactly the server's.
fn step_speed(step: [i32; 2]) -> u32 {
    let squared = i64::from(step[0]).pow(2) + i64::from(step[1]).pow(2);
    ((squared as f64).sqrt() * 80.0).round() as u32
}

/// In 1/256 turn, clockwise from +z.
fn step_angle(step: [i32; 2]) -> u8 {
    let turns = f64::from(step[0]).atan2(f64::from(step[1])) / std::f64::consts::TAU;
    (turns * 256.0).round().rem_euclid(256.0) as u8
}

/// The signed 1/256 turns from `from` to `to` the short way round.
pub fn turn_towards(from: f32, to: u8) -> f32 {
    (f32::from(to) - from + 128.0).rem_euclid(256.0) - 128.0
}

/// The turn a keyframe or delta is for, without decoding the rest.
pub fn message_turn(message: &[u8]) -> Result<u32, &'static str> {
    let (_, body) = message.split_first().ok_or("empty message")?;
    ByteReader(body).varint_u32()
}

/// The terrain a keyframe names, 0 for none, without decoding the rest.
pub fn keyframe_terrain(message: &[u8]) -> Result<u64, &'static str> {
    let (_, body) = message.split_first().ok_or("empty message")?;
    let mut cursor = ByteReader(body);
    // Turn, wire version, match id, game speed, delay, first turn, turns
    // per segment, map size.
    cursor.varint()?;
    cursor.u8()?;
    cursor.u64()?;
    cursor.u32()?;
    cursor.varint()?;
    cursor.varint()?;
    cursor.varint()?;
    cursor.varint()?;
    cursor.u64()
}

pub fn is_keyframe(message: &[u8]) -> bool {
    message.first() == Some(&MESSAGE_KEYFRAME)
}

pub fn is_ended(message: &[u8]) -> bool {
    message.first() == Some(&MESSAGE_ENDED)
}

/// An ended message's match id and ms since the match ended.
pub fn read_ended(message: &[u8]) -> Result<(u64, u64), &'static str> {
    let (_, body) = message.split_first().ok_or("empty message")?;
    let mut cursor = ByteReader(body);
    let match_id = cursor.u64()?;
    let since_ms = cursor.varint()?;
    if !cursor.0.is_empty() {
        return Err("trailing bytes");
    }
    Ok((match_id, since_ms))
}

/// Splits a segment into its messages, the keyframe first.
pub fn read_segment(segment: &[u8]) -> Result<Vec<Vec<u8>>, &'static str> {
    Ok(read_keyframe_and_frame(segment)?.1)
}

/// A starts envelope's ms until the server shows the first turn.
pub fn read_starts(body: &[u8]) -> Result<u32, &'static str> {
    ByteReader(body).varint_u32()
}

/// A join's messages, the keyframe first, and the decoder its frame goes
/// on in.
pub fn read_join(body: &[u8]) -> Result<(FrameDecoder, Vec<Vec<u8>>), &'static str> {
    let (frame, mut messages) = read_keyframe_and_frame(body)?;
    let keyframe_turn = message_turn(&messages[0])?;
    // The delta into the keyframe's turn is in the frame for playback
    // crossing into the segment; the keyframe already holds it.
    let mut kept = vec![std::mem::take(&mut messages[0])];
    for message in messages.into_iter().skip(1) {
        if message_turn(&message)? > keyframe_turn {
            kept.push(message);
        }
    }
    Ok((frame, kept))
}

/// varint n, n bytes of a compressed keyframe, then (part of) a frame.
fn read_keyframe_and_frame(bytes: &[u8]) -> Result<(FrameDecoder, Vec<Vec<u8>>), &'static str> {
    let mut cursor = ByteReader(bytes);
    let length = cursor.varint()? as usize;
    let keyframe = decompress(cursor.take(length)?)?;
    if !is_keyframe(&keyframe) {
        return Err("does not start with a keyframe");
    }
    let mut frame = FrameDecoder::new();
    let mut messages = vec![keyframe];
    messages.extend(frame.feed(cursor.0)?);
    Ok((frame, messages))
}

/// One brotli stream of {varint length, message} entries, fed as it
/// arrives.
pub struct FrameDecoder {
    state: Box<BrotliState<StandardAlloc, StandardAlloc, StandardAlloc>>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        FrameDecoder {
            state: Box::new(new_brotli_state()),
        }
    }

    /// The messages `chunk` holds. The server flushes after every entry,
    /// so a chunk that ends inside one is corrupt.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, &'static str> {
        let mut bytes = Vec::new();
        inflate(&mut self.state, chunk, &mut bytes)?;
        let mut cursor = ByteReader(&bytes);
        let mut messages = Vec::new();
        while !cursor.0.is_empty() {
            let length = cursor.varint()? as usize;
            messages.push(cursor.take(length)?.to_vec());
        }
        Ok(messages)
    }
}

/// One whole brotli stream.
fn decompress(compressed: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut state = new_brotli_state();
    let mut bytes = Vec::new();
    if !inflate(&mut state, compressed, &mut bytes)? {
        return Err("brotli stream cut short");
    }
    Ok(bytes)
}

fn new_brotli_state() -> BrotliState<StandardAlloc, StandardAlloc, StandardAlloc> {
    BrotliState::new(
        StandardAlloc::default(),
        StandardAlloc::default(),
        StandardAlloc::default(),
    )
}

/// Appends what `input` decodes to; true once the stream has ended, which
/// must be at the end of `input`.
fn inflate(
    state: &mut BrotliState<StandardAlloc, StandardAlloc, StandardAlloc>,
    input: &[u8],
    out: &mut Vec<u8>,
) -> Result<bool, &'static str> {
    let mut available_in = input.len();
    let mut input_offset = 0;
    let mut buffer = vec![0; 64 * 1024];
    let mut total_out = 0;
    loop {
        let mut available_out = buffer.len();
        let mut output_offset = 0;
        let result = BrotliDecompressStream(
            &mut available_in,
            &mut input_offset,
            input,
            &mut available_out,
            &mut output_offset,
            &mut buffer,
            &mut total_out,
            state,
        );
        out.extend_from_slice(&buffer[..output_offset]);
        match result {
            BrotliResult::NeedsMoreOutput => {}
            // With the input used up and the buffer full, the decoder says
            // it needs input even when it still holds decoded bytes.
            BrotliResult::NeedsMoreInput if output_offset == buffer.len() => {}
            BrotliResult::NeedsMoreInput => return Ok(false),
            BrotliResult::ResultSuccess if available_in == 0 => return Ok(true),
            BrotliResult::ResultSuccess => return Err("bytes after the brotli stream"),
            BrotliResult::ResultFailure => return Err("brotli corrupt"),
        }
    }
}

#[derive(Default)]
pub(crate) struct Writer(pub(crate) Vec<u8>);

impl Writer {
    pub(crate) fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn f32(&mut self, value: f32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push(value as u8 | 0x80);
            value >>= 7;
        }
        self.0.push(value as u8);
    }

    pub(crate) fn zigzag(&mut self, value: i32) {
        self.zigzag_i64(i64::from(value));
    }

    pub(crate) fn text(&mut self, text: &str) {
        self.varint(text.len() as u64);
        self.0.extend_from_slice(text.as_bytes());
    }

    pub(crate) fn zigzag_i64(&mut self, value: i64) {
        self.varint(((value << 1) ^ (value >> 63)) as u64);
    }
}

/// What only the visual stream records are read with.
trait Records {
    fn template_details(&mut self, version: u8) -> Result<TemplateDetails, &'static str>;
    fn seed(&mut self, id: u32) -> Result<u32, &'static str>;
    fn dot(
        &mut self,
        id: u32,
        turn: u32,
        details: &[Option<TemplateDetails>],
    ) -> Result<(Dot, Vec<u8>), &'static str>;
    fn capture(&mut self) -> Result<Vec<u8>, &'static str>;
    fn economy(&mut self) -> Result<Economy, &'static str>;
    fn change(&mut self, id: u32, dot: &Dot) -> Result<Change, &'static str>;
    fn shot(&mut self, turn: u32) -> Result<Shot, &'static str>;
    fn launch<'b>(
        &mut self,
        turn: u32,
        known: impl Fn(u32) -> Option<&'b Dot>,
    ) -> Result<Shot, &'static str>;
    fn shot_origin(&mut self, shooter: u32, turn: u32) -> Result<Shot, &'static str>;
    fn shot_flight(&mut self, shot: &mut Shot) -> Result<(), &'static str>;
    fn point(&mut self) -> Result<[i32; 3], &'static str>;
}

impl Records for ByteReader<'_> {
    fn template_details(&mut self, version: u8) -> Result<TemplateDetails, &'static str> {
        let mut details = TemplateDetails {
            flags: self.varint_u32()?,
            overlay_texture: self.varint_u32()?,
            overlay_mask: self.varint_u32()?,
            line_thickness: self.f32()?,
            overlay_width: self.f32()?,
            overlay_depth: self.f32()?,
            footprint_width: self.f32()?,
            footprint_depth: self.f32()?,
            footprint_height: self.f32()?,
            bar_width: self.f32()?,
            bar_height: self.f32()?,
            bar_offset: self.f32()?,
            generic_name: self.varint_u32()?,
            specific_name: self.varint_u32()?,
            icon: self.varint_u32()?,
            group_name: self.varint_u32()?,
            rank: self.varint_u32()?,
            classes: self.varint_u32()?,
            visible_classes: self.varint_u32()?,
            select_sound: self.varint_u32()?,
            lang: self.varint_u32()?,
            phenotypes: self.varint_u32()?,
            components: self.varint_u32()?,
            rally_texture: self.varint_u32()?,
            rally_mask: self.varint_u32()?,
            rally_thickness: self.f32()?,
            rally_start_cap: self.u8()?,
            rally_end_cap: self.u8()?,
            supply_type: 0,
            gather_rates: 0,
            dropsite_types: 0,
            attack_types: 0,
            order_sounds: 0,
        };
        if version >= WIRE_VERSION - 1 {
            details.supply_type = self.varint_u32()?;
            details.gather_rates = self.varint_u32()?;
            details.dropsite_types = self.varint_u32()?;
            details.attack_types = self.varint_u32()?;
        }
        if version == WIRE_VERSION {
            details.order_sounds = self.varint_u32()?;
        }
        Ok(details)
    }

    /// Sent as the difference from the entity id, which is the default.
    fn seed(&mut self, id: u32) -> Result<u32, &'static str> {
        u32::try_from(i64::from(id) + self.zigzag_i64()?).map_err(|_| "seed out of range")
    }

    /// The animation and sync turns are counted back from `turn`, the
    /// message's. A new dot appears in place rather than sliding in from
    /// anywhere. Returned with its capture shares.
    fn dot(
        &mut self,
        id: u32,
        turn: u32,
        details: &[Option<TemplateDetails>],
    ) -> Result<(Dot, Vec<u8>), &'static str> {
        let back = |cursor: &mut Self| {
            turn.checked_sub(cursor.varint_u32()?)
                .ok_or("turn before the match")
        };
        let x = self.zigzag()?;
        let z = self.zigzag()?;
        let y = self.zigzag()?;
        let angle = self.u8()?;
        let dot = Dot {
            x,
            z,
            y,
            angle,
            from_y: y as f32,
            from_angle: f32::from(angle),
            color: self.u32()?,
            icon: self.varint_u32()?,
            player: self.varint_u32()?,
            owner: self.zigzag()?,
            template: self.varint_u32()?,
            actor: self.varint_u32()?,
            seed: self.seed(id)?,
            variants: self.varint_u32()?,
            variables: self.varint_u32()?,
            ranges: self.varint_u32()?,
            animation: self.varint_u32()?,
            once: self.u8()? & ANIMATION_ONCE != 0,
            sound_group: self.varint_u32()?,
            speed: self.varint_u32()?,
            animation_turn: back(self)?,
            sync_repeat: self.varint_u32()?,
            sync_offset: self.zigzag()?,
            sync_turn: back(self)?,
            construction: self.u8()?,
            visibility: self.varint_u32()? as u16,
            step: [self.zigzag()?, self.zigzag()?],
            grounded: false,
            hitpoints: 0,
            from_x: x as f32,
            from_z: z as f32,
            speed_turn: 0,
            phase: 0.0,
        };
        let motion = self.u8()?;
        let hitpoints = if motion & MOTION_HITPOINTS != 0 {
            self.u8()?
        } else {
            default_hitpoints(details, dot.template)
        };
        let capture = if motion & MOTION_CAPTURE != 0 {
            self.capture()?
        } else {
            Vec::new()
        };
        let dot = Dot {
            speed_turn: dot.animation_turn,
            grounded: motion & MOTION_GROUNDED != 0,
            hitpoints,
            ..dot
        };
        Ok((dot, capture))
    }

    fn capture(&mut self) -> Result<Vec<u8>, &'static str> {
        let count = self.u8()?;
        Ok(self.take(usize::from(count))?.to_vec())
    }

    fn economy(&mut self) -> Result<Economy, &'static str> {
        let population = self.zigzag()?;
        let population_limit = self.zigzag()?;
        let population_max = self.zigzag()?;
        let resources = (0..self.varint_u32()?)
            .map(|_| {
                Ok(Resource {
                    code: self.varint_u32()?,
                    amount: self.varint_u32()?,
                    gatherers: self.varint_u32()?,
                })
            })
            .collect::<Result<_, &'static str>>()?;
        let phase = self.varint_u32()?;
        let mut last_id = 0i64;
        let panel_entities = (0..self.varint_u32()?)
            .map(|_| {
                last_id += self.zigzag_i64()?;
                u32::try_from(last_id).map_err(|_| "panel entity out of range")
            })
            .collect::<Result<_, _>>()?;
        Ok(Economy {
            population,
            population_limit,
            population_max,
            resources,
            phase,
            panel_entities,
        })
    }

    /// A changed entry after its id, for `dot` as the scene has it.
    fn change(&mut self, id: u32, dot: &Dot) -> Result<Change, &'static str> {
        let mask = self.varint()?;
        let mut change = Change {
            mask,
            step: [0, 0],
            dy: 0,
            values: *dot,
            capture: Vec::new(),
        };
        if mask & CHANGED_POSITION != 0 {
            change.step = [self.zigzag()?, self.zigzag()?];
        }
        if mask & CHANGED_HEIGHT != 0 {
            change.dy = self.zigzag()?;
        }
        let values = &mut change.values;
        if mask & CHANGED_COLOR != 0 {
            values.color = self.u32()?;
        }
        if mask & CHANGED_ICON != 0 {
            values.icon = self.varint_u32()?;
        }
        if mask & CHANGED_PLAYER != 0 {
            values.player = self.varint_u32()?;
        }
        if mask & CHANGED_ANGLE != 0 {
            values.angle = self.u8()?;
        }
        if mask & CHANGED_ANIMATION != 0 {
            values.animation = self.varint_u32()?;
            values.once = self.u8()? & ANIMATION_ONCE != 0;
            values.sound_group = self.varint_u32()?;
        }
        if mask & CHANGED_ANIMATION_SPEED != 0 {
            values.speed = self.varint_u32()?;
        }
        if mask & CHANGED_SYNC != 0 {
            values.sync_repeat = self.varint_u32()?;
            values.sync_offset = self.zigzag()?;
        }
        if mask & CHANGED_OWNER != 0 {
            values.owner = self.zigzag()?;
        }
        if mask & CHANGED_MODEL != 0 {
            values.template = self.varint_u32()?;
            values.actor = self.varint_u32()?;
            values.seed = self.seed(id)?;
        }
        if mask & CHANGED_VARIANTS != 0 {
            values.variants = self.varint_u32()?;
        }
        if mask & CHANGED_CONSTRUCTION != 0 {
            values.construction = self.u8()?;
        }
        if mask & CHANGED_VISIBILITY != 0 {
            values.visibility = self.varint_u32()? as u16;
        }
        if mask & CHANGED_VARIABLES != 0 {
            values.variables = self.varint_u32()?;
        }
        if mask & CHANGED_RANGES != 0 {
            values.ranges = self.varint_u32()?;
        }
        if mask & CHANGED_HITPOINTS != 0 {
            values.hitpoints = self.u8()?;
        }
        if mask & CHANGED_CAPTURE != 0 {
            change.capture = self.capture()?;
        }
        Ok(change)
    }

    /// A projectile launched on `turn`, as a keyframe has it.
    fn shot(&mut self, turn: u32) -> Result<Shot, &'static str> {
        let shooter = self.varint_u32()?;
        let mut shot = self.shot_origin(shooter, turn)?;
        shot.launch = self.point()?;
        shot.target = self.point()?;
        self.shot_flight(&mut shot)?;
        Ok(shot)
    }

    /// A projectile launched on `turn`, as a delta has it: relative to its
    /// shooter if `known` has it.
    fn launch<'b>(
        &mut self,
        turn: u32,
        known: impl Fn(u32) -> Option<&'b Dot>,
    ) -> Result<Shot, &'static str> {
        let shooter_id = self.varint_u32()?;
        let shooter = known(shooter_id).filter(|_| shooter_id != 0);
        let (mut shot, origin) = match shooter {
            Some(dot) => (
                Shot {
                    launch_turn: turn,
                    shooter: shooter_id,
                    owner: dot.owner,
                    actor: dot.actor,
                    seed: dot.seed,
                    variants: dot.variants,
                    ..Shot::default()
                },
                [dot.x, dot.y, dot.z],
            ),
            None => (self.shot_origin(shooter_id, turn)?, [0; 3]),
        };
        let launch = self.point()?;
        let target = self.point()?;
        for axis in 0..3 {
            shot.launch[axis] = origin[axis] + launch[axis];
            shot.target[axis] = shot.launch[axis] + target[axis];
        }
        self.shot_flight(&mut shot)?;
        Ok(shot)
    }

    /// The shooter's fields as sent, the rest left to fill in.
    fn shot_origin(&mut self, shooter: u32, turn: u32) -> Result<Shot, &'static str> {
        Ok(Shot {
            launch_turn: turn,
            shooter,
            owner: self.zigzag()?,
            actor: self.varint_u32()?,
            seed: self.seed(shooter)?,
            variants: self.varint_u32()?,
            ..Shot::default()
        })
    }

    fn shot_flight(&mut self, shot: &mut Shot) -> Result<(), &'static str> {
        shot.speed = self.varint_u32()?;
        shot.gravity = self.zigzag()?;
        shot.projectile_actor = self.varint_u32()?;
        shot.impact_actor = self.varint_u32()?;
        shot.impact_lifetime = self.varint_u32()?;
        Ok(())
    }

    /// Sent as x, z, y; returned as x, y, z.
    fn point(&mut self) -> Result<[i32; 3], &'static str> {
        let x = self.zigzag()?;
        let z = self.zigzag()?;
        let y = self.zigzag()?;
        Ok([x, y, z])
    }
}
