//! Decoder for specula-server's viewer stream and match segments, wire
//! format v10; the layout is documented in `specula-server/src/view.rs`.

use std::collections::BTreeMap;
use std::io::Read;
use std::rc::Rc;

use ruzstd::decoding::StreamingDecoder;

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
    /// 1-based index into `Scene::strings`, or 0 for none.
    pub animation: u32,
    /// Plays once rather than loops.
    pub once: bool,
    /// In 1/256: m/s for walk and run, else a multiplier.
    pub speed: u32,
    /// The turn the animation was selected on.
    pub animation_turn: u32,
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
        let mut cursor = Cursor(resource);
        if cursor.u8()? != 1 {
            return Err("unknown terrain version");
        }
        let tiles = cursor.u32()?;
        // Water level, shallow depth and circular.
        cursor.take(9)?;
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

#[derive(Default)]
pub struct Scene {
    pub turn: u32,
    /// Names the match's segments.
    pub match_id: u64,
    /// The sim rate the match was set up with; 1.0 is normal speed.
    pub game_speed: f32,
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
    /// Set whenever a message replaces `players`; the reader clears it.
    pub players_changed: bool,
    pub territory: Territory,
    /// Goes up whenever `territory` changes, so each view can tell on its
    /// own whether it has drawn the current one.
    pub territory_generation: u32,
    pub templates: Vec<String>,
    /// Actor file names, animation names, variant keys and selections.
    pub strings: Vec<String>,
    pub variant_sets: Vec<VariantSet>,
    pub dots: BTreeMap<u32, Dot>,
    /// The sounds of the last delta applied; a keyframe has none.
    pub sounds: Vec<Sound>,
    /// By id: in flight, or lying on the ground.
    pub shots: BTreeMap<u32, Shot>,
    /// The heightmap of `terrain_id`, which must be set before a keyframe
    /// naming it is applied.
    pub ground: Option<Rc<Ground>>,
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
        let mut cursor = Cursor(body);
        self.sounds.clear();
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

    fn apply_keyframe(&mut self, cursor: &mut Cursor) -> Result<(), &'static str> {
        self.turn = cursor.varint_u32()?;
        self.match_id = cursor.u64()?;
        self.game_speed = f32::from_bits(cursor.u32()?);
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
        self.strings.clear();
        self.variant_sets.clear();
        self.read_tables(cursor)?;
        self.dots.clear();
        let count = cursor.varint_u32()?;
        let mut id = 0;
        for _ in 0..count {
            id += cursor.varint_u32()?;
            let dot = cursor.dot(id, self.turn)?;
            self.dots.insert(id, dot);
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

    fn apply_delta(&mut self, cursor: &mut Cursor, progress: f32) -> Result<(), &'static str> {
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
        let spawned = cursor.varint_u32()?;
        let destroyed = cursor.varint_u32()?;
        let changed = cursor.varint_u32()?;
        let mut id = 0;
        let mut arrived = Vec::new();
        for _ in 0..spawned {
            id += cursor.varint_u32()?;
            arrived.push((id, cursor.dot(id, self.turn)?));
        }
        let mut id = 0;
        let mut gone = BTreeMap::new();
        for _ in 0..destroyed {
            id += cursor.varint_u32()?;
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
        self.dots.extend(arrived);
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
        Ok(())
    }

    /// Templates, strings and variant sets, appended to what the scene has.
    fn read_tables(&mut self, cursor: &mut Cursor) -> Result<(), &'static str> {
        for table in [&mut self.templates, &mut self.strings] {
            let count = cursor.varint_u32()?;
            for _ in 0..count {
                let length = cursor.varint()? as usize;
                let text =
                    std::str::from_utf8(cursor.take(length)?).map_err(|_| "string not UTF-8")?;
                table.push(text.to_string());
            }
        }
        let count = cursor.varint_u32()?;
        for _ in 0..count {
            let pairs = cursor.varint_u32()?;
            let set = (0..pairs)
                .map(|_| Ok((cursor.varint_u32()?, cursor.varint_u32()?)))
                .collect::<Result<_, &'static str>>()?;
            self.variant_sets.push(set);
        }
        Ok(())
    }

    fn read_icons(&mut self, cursor: &mut Cursor) -> Result<(), &'static str> {
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

    fn read_players(&mut self, cursor: &mut Cursor) -> Result<(), &'static str> {
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
        self.players_changed = true;
        Ok(())
    }

    fn read_territory(&mut self, cursor: &mut Cursor) -> Result<(), &'static str> {
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
        out.u64(self.match_id);
        out.u32(self.game_speed.to_bits());
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
        out.varint(u64::from(self.territory.width));
        out.varint(u64::from(self.territory.height));
        for run in self.territory.tiles.chunk_by(|a, b| a == b) {
            out.varint(run.len() as u64);
            out.u8(run[0]);
        }
        for table in [&self.templates, &self.strings] {
            out.varint(table.len() as u64);
            for text in table {
                out.varint(text.len() as u64);
                out.0.extend_from_slice(text.as_bytes());
            }
        }
        out.varint(self.variant_sets.len() as u64);
        for set in &self.variant_sets {
            out.varint(set.len() as u64);
            for (key, selection) in set {
                out.varint(u64::from(*key));
                out.varint(u64::from(*selection));
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
            out.u8(if dot.grounded { MOTION_GROUNDED } else { 0 });
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
    }
    if dot.angle != angle {
        // Keep the start within half a turn, so the heading turns the short
        // way.
        dot.from_angle = f32::from(dot.angle) - turn_towards(dot.from_angle, dot.angle);
    }
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
    Cursor(body).varint_u32()
}

/// The terrain a keyframe names, 0 for none, without decoding the rest.
pub fn keyframe_terrain(message: &[u8]) -> Result<u64, &'static str> {
    let (_, body) = message.split_first().ok_or("empty message")?;
    let mut cursor = Cursor(body);
    // Turn, match id, game speed, first turn, turns per segment, map size.
    cursor.varint()?;
    cursor.u64()?;
    cursor.u32()?;
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
    let mut cursor = Cursor(body);
    let match_id = cursor.u64()?;
    let since_ms = cursor.varint()?;
    if !cursor.0.is_empty() {
        return Err("trailing bytes");
    }
    Ok((match_id, since_ms))
}

/// Splits a zstd-compressed segment into its messages.
pub fn read_segment(compressed: &[u8]) -> Result<Vec<Vec<u8>>, &'static str> {
    let mut decoder = StreamingDecoder::new(compressed).map_err(|_| "segment not zstd")?;
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .map_err(|_| "segment zstd corrupt")?;
    let mut cursor = Cursor(&bytes);
    let mut messages = Vec::new();
    while !cursor.0.is_empty() {
        let length = cursor.varint()? as usize;
        messages.push(cursor.take(length)?.to_vec());
    }
    if !messages.first().is_some_and(|first| is_keyframe(first)) {
        return Err("segment does not start with a keyframe");
    }
    Ok(messages)
}

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push(value as u8 | 0x80);
            value >>= 7;
        }
        self.0.push(value as u8);
    }

    fn zigzag(&mut self, value: i32) {
        self.zigzag_i64(i64::from(value));
    }

    fn zigzag_i64(&mut self, value: i64) {
        self.varint(((value << 1) ^ (value >> 63)) as u64);
    }
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], &'static str> {
        if self.0.len() < length {
            return Err("message truncated");
        }
        let (bytes, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, &'static str> {
        let (&value, rest) = self.0.split_first().ok_or("message truncated")?;
        self.0 = rest;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, &'static str> {
        let (bytes, rest) = self.0.split_first_chunk::<4>().ok_or("message truncated")?;
        self.0 = rest;
        Ok(u32::from_le_bytes(*bytes))
    }

    fn u64(&mut self) -> Result<u64, &'static str> {
        let (bytes, rest) = self.0.split_first_chunk::<8>().ok_or("message truncated")?;
        self.0 = rest;
        Ok(u64::from_le_bytes(*bytes))
    }

    fn varint(&mut self) -> Result<u64, &'static str> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("varint too long")
    }

    fn varint_u32(&mut self) -> Result<u32, &'static str> {
        u32::try_from(self.varint()?).map_err(|_| "varint out of range")
    }

    fn zigzag_i64(&mut self) -> Result<i64, &'static str> {
        let raw = self.varint()?;
        Ok((raw >> 1) as i64 ^ -((raw & 1) as i64))
    }

    fn zigzag(&mut self) -> Result<i32, &'static str> {
        i32::try_from(self.zigzag_i64()?).map_err(|_| "zigzag out of range")
    }

    /// Sent as the difference from the entity id, which is the default.
    fn seed(&mut self, id: u32) -> Result<u32, &'static str> {
        u32::try_from(i64::from(id) + self.zigzag_i64()?).map_err(|_| "seed out of range")
    }

    /// The animation and sync turns are counted back from `turn`, the
    /// message's. A new dot appears in place rather than sliding in from
    /// anywhere.
    fn dot(&mut self, id: u32, turn: u32) -> Result<Dot, &'static str> {
        let back = |cursor: &mut Self| {
            turn.checked_sub(cursor.varint_u32()?)
                .ok_or("turn before the match")
        };
        let x = self.zigzag()?;
        let z = self.zigzag()?;
        let y = self.zigzag()?;
        let angle = self.u8()?;
        Ok(Dot {
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
            grounded: self.u8()? & MOTION_GROUNDED != 0,
            from_x: x as f32,
            from_z: z as f32,
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
