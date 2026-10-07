//! Decoder for specula-server's viewer stream and match segments, wire
//! format v9; the layout is documented in `specula-server/src/view.rs`.

use std::collections::BTreeMap;
use std::io::Read;

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
#[derive(Clone, Copy)]
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
        for _ in 0..spawned {
            id += cursor.varint_u32()?;
            let dot = cursor.dot(id, self.turn)?;
            self.dots.insert(id, dot);
        }
        let mut id = 0;
        for _ in 0..destroyed {
            id += cursor.varint_u32()?;
            self.dots.remove(&id);
        }
        let mut id = 0;
        for _ in 0..changed {
            id += cursor.varint_u32()?;
            let mask = cursor.varint()?;
            let dot = self
                .dots
                .get_mut(&id)
                .ok_or("change to an unknown entity")?;
            if mask & CHANGED_POSITION != 0 {
                dot.x += cursor.zigzag()?;
                dot.z += cursor.zigzag()?;
                dot.y += cursor.zigzag()?;
                let distance = (dot.x as f32 - dot.from_x).hypot(dot.z as f32 - dot.from_z);
                if distance > SNAP_UNITS {
                    dot.from_x = dot.x as f32;
                    dot.from_z = dot.z as f32;
                    dot.from_y = dot.y as f32;
                }
            }
            if mask & CHANGED_COLOR != 0 {
                dot.color = cursor.u32()?;
            }
            if mask & CHANGED_ICON != 0 {
                dot.icon = cursor.varint_u32()?;
            }
            if mask & CHANGED_PLAYER != 0 {
                dot.player = cursor.varint_u32()?;
            }
            if mask & CHANGED_ANGLE != 0 {
                dot.angle = cursor.u8()?;
                // Keep the start within half a turn, so the heading turns
                // the short way.
                dot.from_angle = f32::from(dot.angle) - turn_towards(dot.from_angle, dot.angle);
            }
            if mask & CHANGED_ANIMATION != 0 {
                dot.animation = cursor.varint_u32()?;
                dot.once = cursor.u8()? & ANIMATION_ONCE != 0;
                dot.sound_group = cursor.varint_u32()?;
                dot.animation_turn = self.turn;
            }
            if mask & CHANGED_ANIMATION_SPEED != 0 {
                dot.speed = cursor.varint_u32()?;
            }
            if mask & CHANGED_SYNC != 0 {
                dot.sync_repeat = cursor.varint_u32()?;
                dot.sync_offset = cursor.zigzag()?;
                dot.sync_turn = self.turn;
            }
            if mask & CHANGED_OWNER != 0 {
                dot.owner = cursor.zigzag()?;
            }
            if mask & CHANGED_MODEL != 0 {
                dot.template = cursor.varint_u32()?;
                dot.actor = cursor.varint_u32()?;
                dot.seed = cursor.seed(id)?;
            }
            if mask & CHANGED_VARIANTS != 0 {
                dot.variants = cursor.varint_u32()?;
            }
            if mask & CHANGED_CONSTRUCTION != 0 {
                dot.construction = cursor.u8()?;
            }
            if mask & CHANGED_VISIBILITY != 0 {
                dot.visibility = cursor.varint_u32()? as u16;
            }
        }
        let count = cursor.varint_u32()?;
        for _ in 0..count {
            let group = cursor.varint_u32()?;
            let target = match cursor.u8()? {
                SOUND_ENTITY => {
                    // The id seeds the engine's random gain and pitch; any
                    // random does here.
                    cursor.varint_u32()?;
                    SoundTarget::Entity {
                        owner: cursor.zigzag()?,
                        x: cursor.zigzag()?,
                        z: cursor.zigzag()?,
                        y: cursor.zigzag()?,
                        visibility: cursor.varint_u32()? as u16,
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
        self.shots.retain(|_, shot| shot.last_turn() >= turn);
        let count = cursor.varint_u32()?;
        let mut id = 0;
        for _ in 0..count {
            id += cursor.varint_u32()?;
            let shot = cursor.shot(turn)?;
            self.shots.insert(id, shot);
        }
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

/// The signed 1/256 turns from `from` to `to` the short way round.
pub fn turn_towards(from: f32, to: u8) -> f32 {
    (f32::from(to) - from + 128.0).rem_euclid(256.0) - 128.0
}

/// The turn a keyframe or delta is for, without decoding the rest.
pub fn message_turn(message: &[u8]) -> Result<u32, &'static str> {
    let (_, body) = message.split_first().ok_or("empty message")?;
    Cursor(body).varint_u32()
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
            from_x: x as f32,
            from_z: z as f32,
        })
    }

    /// A projectile launched on `turn`.
    fn shot(&mut self, turn: u32) -> Result<Shot, &'static str> {
        let shooter = self.varint_u32()?;
        let owner = self.zigzag()?;
        let actor = self.varint_u32()?;
        let seed = self.seed(shooter)?;
        let variants = self.varint_u32()?;
        let mut point = || -> Result<[i32; 3], &'static str> {
            let x = self.zigzag()?;
            let z = self.zigzag()?;
            let y = self.zigzag()?;
            Ok([x, y, z])
        };
        let launch = point()?;
        let target = point()?;
        Ok(Shot {
            launch_turn: turn,
            shooter,
            owner,
            actor,
            seed,
            variants,
            launch,
            target,
            speed: self.varint_u32()?,
            gravity: self.zigzag()?,
            projectile_actor: self.varint_u32()?,
            impact_actor: self.varint_u32()?,
            impact_lifetime: self.varint_u32()?,
        })
    }
}
