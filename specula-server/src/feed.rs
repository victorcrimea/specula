//! Decoder for the observer hook's feed, format v13. The writer is
//! `source/network/SpeculaFeed.cpp` in the `specula` branch of 0ad-specula.

use std::io;
use std::io::ErrorKind;
use std::io::Read;

const MAGIC: [u8; 4] = *b"SPCF";
const VERSION: u32 = 14;
const RECORD_TEMPLATE: u8 = 1;
const RECORD_TURN: u8 = 2;
const RECORD_MAP: u8 = 3;
const RECORD_TERRAIN: u8 = 4;
const RECORD_PLAYERS: u8 = 5;
const RECORD_STRING: u8 = 6;
const RECORD_VARIANT_SET: u8 = 7;
const RECORD_TERRITORY: u8 = 8;
const RECORD_SOUNDS: u8 = 9;
const RECORD_PROJECTILES: u8 = 10;
const RECORD_VARIABLE_SET: u8 = 16;
const ENTITY_SIZE: usize = 74;
const SOUND_SIZE: usize = 27;
const LAUNCH_SIZE: usize = 52;
const COLOR_MINIMAP: u8 = 1;
const HAS_VISUAL: u8 = 2;
const ANIMATION_ONCE: u8 = 4;
const NONE: u32 = 0xFFFF_FFFF;
const TEMPLATE_PLAYER_COLOR: u8 = 1;
const SOUND_ENTITY: u8 = 0;
const SOUND_POSITION: u8 = 1;
const SOUND_PLAYER: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Entity {
    pub id: u32,
    pub template: u32,
    /// Raw 16.16 fixed point, in metres.
    pub x: i32,
    pub z: i32,
    /// Raw 16.16 fixed point, in radians.
    pub angle: i32,
    pub owner: i32,
    /// 0xRRGGBB, or `None` when the entity is not on the minimap.
    pub color: Option<u32>,
    /// Raw 16.16 fixed point, in metres above the map's zero height.
    pub y: i32,
    /// `None` for an entity without a VisualActor.
    pub visual: Option<Visual>,
    /// Raw 16.16 fixed point, 0..=1 of the model risen out of the ground;
    /// 0 means not offset at all.
    pub construction: i32,
    /// Bit p is set when player p sees the entity, for p below 16.
    pub visibility: u16,
}

/// What the entity's VisualActor shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Visual {
    /// Index of a string record.
    pub actor: Option<u32>,
    pub seed: u32,
    /// Index of a variant set record; `None` for no selections.
    pub variants: Option<u32>,
    /// Index of a variable set record; `None` for no variables.
    pub variables: Option<u32>,
    /// Index of a string record.
    pub animation: Option<u32>,
    /// Plays once rather than loops.
    pub once: bool,
    /// Raw 16.16 fixed point; m/s for walk and run, else a multiplier.
    pub speed: i32,
    /// Raw 16.16 fixed point, in milliseconds; 0 when not synced.
    pub sync_repeat: i32,
    pub sync_offset: i32,
    /// Index of a string record: the sound group the animation plays at its
    /// sound or event point, relative to audio/.
    pub sound_group: Option<u32>,
}

/// A sound group the simulation played during a turn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Sound {
    /// Index of a string record, relative to audio/.
    pub group: u32,
    pub target: SoundTarget,
    /// Raw 16.16 fixed point, in metres; zero for a player's sound.
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Bit p is set when player p hears it; all bits unless it is an
    /// entity's.
    pub visibility: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SoundTarget {
    /// Played on an entity, with its owner as of the call.
    Entity {
        id: u32,
        owner: i32,
    },
    Position,
    /// Only for this player.
    Player(i32),
}

/// A `LaunchProjectileAtPoint` call.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Launch {
    pub id: u32,
    /// The entity that asked for its launch point right before, 0 for none.
    pub shooter: u32,
    /// Raw 16.16 fixed point, in metres; the simulation's launch point.
    pub launch: [i32; 3],
    pub target: [i32; 3],
    /// Raw 16.16 fixed point: horizontal m/s, and m/s^2.
    pub speed: i32,
    pub gravity: i32,
    /// Indices of string records, actor file names.
    pub actor: Option<u32>,
    pub impact_actor: Option<u32>,
    /// Raw 16.16 fixed point, in seconds.
    pub impact_lifetime: i32,
}

/// A template's minimap icon, as the in-game minimap draws it.
pub struct Icon {
    /// Relative to art/textures/ui/session/icons/minimap/.
    pub name: String,
    /// In GUI pixels.
    pub size: f32,
}

/// A player as `GuiInterface.GetSimulationState` reports it; index 0 is
/// gaia.
#[derive(Clone, PartialEq, Eq)]
pub struct Player {
    pub name: String,
    /// 0xRRGGBB, quantized like the minimap colour.
    pub color: u32,
    /// -1 for none.
    pub team: i32,
    /// 0 active, 1 defeated, 2 won.
    pub state: u8,
    /// Towards each player by index: 0 enemy, 1 ally, 2 neutral.
    pub stances: Vec<u8>,
    /// Civ code, as in simulation/data/civs/<civ>.json.
    pub civ: String,
    /// `BattleDetection` reports a battle.
    pub battle: bool,
}

pub enum Record {
    Template {
        index: u32,
        name: String,
        icon: Option<Icon>,
        /// The minimap colour is the owner's player colour.
        player_color: bool,
    },
    Turn {
        turn: u32,
        entities: Vec<Entity>,
    },
    Map {
        /// In metres.
        size: u32,
        /// The sim rate the match was set up with; 1.0 is normal speed.
        speed: f32,
    },
    Terrain(Box<Terrain>),
    Players(Vec<Player>),
    Territory(Territory),
    /// The sounds of the next turn record, in the order they were played.
    Sounds(Vec<Sound>),
    /// The projectiles of the next turn record, each list in the order of
    /// the calls.
    Projectiles {
        launches: Vec<Launch>,
        /// Ids hit and taken away by `RemoveProjectile`.
        removals: Vec<u32>,
    },
    String {
        index: u32,
        text: String,
    },
    /// (key, value) string indices, sorted by key.
    VariantSet {
        index: u32,
        pairs: Vec<(u32, u32)>,
    },
    /// (name string index, value), sorted by name.
    VariableSet {
        index: u32,
        pairs: Vec<(u32, f32)>,
    },
}

/// `ICmpTerritoryManager`'s grid, whenever it changes: row-major, x
/// fastest, a tile per 8 navcells; owner in 0x1F, connected 0x20, blinking
/// 0x40.
#[derive(Clone, PartialEq, Eq)]
pub struct Territory {
    pub width: u32,
    pub height: u32,
    pub tiles: Vec<u8>,
}

/// Sent once per match; heights and tiles are row-major, x fastest.
pub struct Terrain {
    pub tiles_per_side: u32,
    /// Raw 16.16 fixed point, in metres.
    pub water_level: i32,
    /// In metres; water shallower than this is drawn as a ford.
    pub shallow_depth: f32,
    pub circular: bool,
    pub light: Light,
    pub water: Water,
    /// The `art/textures/skies` set; empty for none.
    pub sky: String,
    /// (tiles + 1)^2 vertices, in 1/92 m.
    pub heights: Vec<u16>,
    pub textures: Vec<String>,
    /// tiles^2 indices into `textures`; 0xFFFF for none.
    pub tiles: Vec<u16>,
    /// tiles^2 blend priorities: a tile's texture is drawn over the
    /// neighbours of lower priority.
    pub priorities: Vec<u32>,
}

/// The map's `WaterManager` settings.
pub struct Water {
    pub color: [f32; 3],
    pub tint: [f32; 3],
    pub waviness: f32,
    pub murkiness: f32,
    /// Radians.
    pub wind_angle: f32,
    /// The `art/textures/animated/water` set of normal maps.
    pub kind: String,
}

/// The map's `CLightEnv`, as it lights the match in the game.
pub struct Light {
    /// Radians.
    pub elevation: f32,
    pub rotation: f32,
    pub sun_color: [f32; 3],
    pub ambient_color: [f32; 3],
    pub fog_color: [f32; 3],
    pub fog_factor: f32,
    pub fog_max: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub bloom: f32,
    pub post_effect: PostEffect,
}

/// The map's post effect, which the engine loads from
/// `shaders/effects/postproc/<name>.xml`.
#[derive(Clone, Copy)]
pub enum PostEffect {
    Default = 0,
    Hdr = 1,
    Dof = 2,
}

impl PostEffect {
    /// The engine has no other effect files, and draws none for a name it
    /// cannot load.
    fn from_name(name: &[u8]) -> Self {
        match name {
            b"hdr" => Self::Hdr,
            b"DOF" => Self::Dof,
            _ => Self::Default,
        }
    }
}

pub fn read_header(reader: &mut impl Read) -> io::Result<()> {
    let mut header = [0u8; 8];
    reader.read_exact(&mut header)?;
    if header[..4] != MAGIC {
        return Err(invalid("not a Specula feed"));
    }
    let version = u32::from_le_bytes(header[4..].try_into().unwrap());
    if version != VERSION {
        return Err(invalid(format!(
            "feed version {version}, expected {VERSION}"
        )));
    }
    Ok(())
}

/// `None` when the observer closed the feed between records.
pub fn read_record(reader: &mut impl Read) -> io::Result<Option<Record>> {
    loop {
        let mut head = [0u8; 5];
        match reader.read_exact(&mut head) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
        }
        let length = u32::from_le_bytes(head[1..].try_into().unwrap()) as usize;
        let mut payload = vec![0u8; length];
        reader.read_exact(&mut payload)?;
        match head[0] {
            RECORD_TEMPLATE => return decode_template(&payload).map(Some),
            RECORD_TURN => return decode_turn(&payload).map(Some),
            RECORD_MAP => {
                let mut cursor = Cursor(&payload);
                let size = cursor.u32()?;
                let speed = f32::from_bits(cursor.u32()?);
                return Ok(Some(Record::Map { size, speed }));
            }
            RECORD_TERRAIN => return decode_terrain(&payload).map(Some),
            RECORD_PLAYERS => return decode_players(&payload).map(Some),
            RECORD_STRING => {
                let mut cursor = Cursor(&payload);
                let index = cursor.u32()?;
                let text = String::from_utf8(cursor.0.to_vec())
                    .map_err(|_| invalid("string not UTF-8"))?;
                return Ok(Some(Record::String { index, text }));
            }
            RECORD_VARIANT_SET => return decode_variant_set(&payload).map(Some),
            RECORD_TERRITORY => return decode_territory(&payload).map(Some),
            RECORD_SOUNDS => return decode_sounds(&payload).map(Some),
            RECORD_PROJECTILES => return decode_projectiles(&payload).map(Some),
            RECORD_VARIABLE_SET => return decode_variable_set(&payload).map(Some),
            // Newer hooks may add kinds; the length prefix lets them be skipped.
            _ => continue,
        }
    }
}

fn decode_template(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let index = cursor.u32()?;
    let length = cursor.u16()? as usize;
    let name = String::from_utf8(cursor.bytes(length)?.to_vec())
        .map_err(|_| invalid("template name not UTF-8"))?;
    let size = f32::from_bits(cursor.u32()?);
    let flags = cursor.u8()?;
    let icon_name =
        String::from_utf8(cursor.0.to_vec()).map_err(|_| invalid("icon name not UTF-8"))?;
    let icon = (!icon_name.is_empty()).then_some(Icon {
        name: icon_name,
        size,
    });
    Ok(Record::Template {
        index,
        name,
        icon,
        player_color: flags & TEMPLATE_PLAYER_COLOR != 0,
    })
}

fn decode_turn(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let turn = cursor.u32()?;
    let count = cursor.u32()? as usize;
    if cursor.0.len() != count * ENTITY_SIZE {
        return Err(invalid(
            "turn record length does not match its entity count",
        ));
    }
    let mut entities = Vec::with_capacity(count);
    for _ in 0..count {
        let id = cursor.u32()?;
        let template = cursor.u32()?;
        let x = cursor.i32()?;
        let z = cursor.i32()?;
        let angle = cursor.i32()?;
        let owner = cursor.i32()?;
        let color = cursor.u32()?;
        let flags = (color >> 24) as u8;
        let y = cursor.i32()?;
        let visual = Visual {
            actor: optional(cursor.u32()?),
            seed: cursor.u32()?,
            variants: optional(cursor.u32()?),
            variables: optional(cursor.u32()?),
            animation: optional(cursor.u32()?),
            once: flags & ANIMATION_ONCE != 0,
            speed: cursor.i32()?,
            sync_repeat: cursor.i32()?,
            sync_offset: cursor.i32()?,
            sound_group: None,
        };
        let construction = cursor.i32()?;
        let visual = Visual {
            sound_group: optional(cursor.u32()?),
            ..visual
        };
        entities.push(Entity {
            id,
            template,
            x,
            z,
            angle,
            owner,
            color: decode_color(color),
            y,
            visual: (flags & HAS_VISUAL != 0).then_some(visual),
            construction,
            visibility: cursor.u16()?,
        });
    }
    Ok(Record::Turn { turn, entities })
}

fn decode_terrain(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let tiles_per_side = cursor.u32()?;
    let water_level = cursor.i32()?;
    let shallow_depth = f32::from_bits(cursor.u32()?);
    let circular = cursor.u8()? != 0;
    let mut floats = [0.0; 17];
    for value in &mut floats {
        *value = f32::from_bits(cursor.u32()?);
    }
    let [
        elevation,
        rotation,
        sun_r,
        sun_g,
        sun_b,
        ambient_r,
        ambient_g,
        ambient_b,
        fog_r,
        fog_g,
        fog_b,
        fog_factor,
        fog_max,
        brightness,
        contrast,
        saturation,
        bloom,
    ] = floats;
    let length = cursor.u16()? as usize;
    let light = Light {
        elevation,
        rotation,
        sun_color: [sun_r, sun_g, sun_b],
        ambient_color: [ambient_r, ambient_g, ambient_b],
        fog_color: [fog_r, fog_g, fog_b],
        fog_factor,
        fog_max,
        brightness,
        contrast,
        saturation,
        bloom,
        post_effect: PostEffect::from_name(cursor.bytes(length)?),
    };
    let mut floats = [0.0; 9];
    for value in &mut floats {
        *value = f32::from_bits(cursor.u32()?);
    }
    let [
        r,
        g,
        b,
        tint_r,
        tint_g,
        tint_b,
        waviness,
        murkiness,
        wind_angle,
    ] = floats;
    let water = Water {
        color: [r, g, b],
        tint: [tint_r, tint_g, tint_b],
        waviness,
        murkiness,
        wind_angle,
        kind: cursor.string("water type not UTF-8")?,
    };
    let sky = cursor.string("sky set not UTF-8")?;
    let vertices = (tiles_per_side as usize + 1).pow(2);
    let heights = (0..vertices)
        .map(|_| cursor.u16())
        .collect::<io::Result<_>>()?;
    let count = cursor.u32()?;
    let mut textures = Vec::with_capacity(count as usize);
    for _ in 0..count {
        textures.push(cursor.string("texture name not UTF-8")?);
    }
    let tiles = (0..(tiles_per_side as usize).pow(2))
        .map(|_| cursor.u16())
        .collect::<io::Result<_>>()?;
    let priorities = (0..(tiles_per_side as usize).pow(2))
        .map(|_| cursor.u32())
        .collect::<io::Result<_>>()?;
    if !cursor.0.is_empty() {
        return Err(invalid("terrain record longer than its contents"));
    }
    Ok(Record::Terrain(Box::new(Terrain {
        tiles_per_side,
        water_level,
        shallow_depth,
        circular,
        light,
        water,
        sky,
        heights,
        textures,
        tiles,
        priorities,
    })))
}

fn decode_players(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let count = cursor.u32()? as usize;
    let mut players = Vec::new();
    for _ in 0..count {
        let length = cursor.u16()? as usize;
        let name = String::from_utf8(cursor.bytes(length)?.to_vec())
            .map_err(|_| invalid("player name not UTF-8"))?;
        let [r, g, b] = cursor.bytes(3)?.try_into().unwrap();
        let team = cursor.i32()?;
        let state = cursor.u8()?;
        let stances = cursor.bytes(count)?.to_vec();
        let length = cursor.u16()? as usize;
        let civ = String::from_utf8(cursor.bytes(length)?.to_vec())
            .map_err(|_| invalid("civ not UTF-8"))?;
        let battle = cursor.u8()? != 0;
        players.push(Player {
            name,
            color: u32::from_be_bytes([0, r, g, b]),
            team,
            state,
            stances,
            civ,
            battle,
        });
    }
    if !cursor.0.is_empty() {
        return Err(invalid("players record longer than its contents"));
    }
    Ok(Record::Players(players))
}

fn decode_territory(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let width = cursor.u32()?;
    let height = cursor.u32()?;
    let tiles = cursor.bytes(width as usize * height as usize)?.to_vec();
    if !cursor.0.is_empty() {
        return Err(invalid("territory record longer than its contents"));
    }
    Ok(Record::Territory(Territory {
        width,
        height,
        tiles,
    }))
}

fn decode_sounds(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let count = cursor.u32()? as usize;
    if cursor.0.len() != count * SOUND_SIZE {
        return Err(invalid("sounds record length does not match its count"));
    }
    let mut sounds = Vec::with_capacity(count);
    for _ in 0..count {
        let group = cursor.u32()?;
        let target = cursor.u8()?;
        let id = cursor.u32()?;
        let player = cursor.i32()?;
        let target = match target {
            SOUND_ENTITY => SoundTarget::Entity { id, owner: player },
            SOUND_POSITION => SoundTarget::Position,
            SOUND_PLAYER => SoundTarget::Player(player),
            _ => return Err(invalid("unknown sound target")),
        };
        sounds.push(Sound {
            group,
            target,
            x: cursor.i32()?,
            y: cursor.i32()?,
            z: cursor.i32()?,
            visibility: cursor.u16()?,
        });
    }
    Ok(Record::Sounds(sounds))
}

fn decode_projectiles(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let count = cursor.u32()? as usize;
    if cursor.0.len() < count * LAUNCH_SIZE {
        return Err(invalid("projectiles record shorter than its launches"));
    }
    let mut launches = Vec::with_capacity(count);
    for _ in 0..count {
        launches.push(Launch {
            id: cursor.u32()?,
            shooter: cursor.u32()?,
            launch: [cursor.i32()?, cursor.i32()?, cursor.i32()?],
            target: [cursor.i32()?, cursor.i32()?, cursor.i32()?],
            speed: cursor.i32()?,
            gravity: cursor.i32()?,
            actor: optional(cursor.u32()?),
            impact_actor: optional(cursor.u32()?),
            impact_lifetime: cursor.i32()?,
        });
    }
    let count = cursor.u32()? as usize;
    if cursor.0.len() != count * 4 {
        return Err(invalid(
            "projectiles record length does not match its counts",
        ));
    }
    let removals = (0..count)
        .map(|_| cursor.u32())
        .collect::<io::Result<_>>()?;
    Ok(Record::Projectiles { launches, removals })
}

fn decode_variant_set(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let index = cursor.u32()?;
    let count = cursor.u32()? as usize;
    if cursor.0.len() != count * 8 {
        return Err(invalid(
            "variant set record length does not match its count",
        ));
    }
    let pairs = (0..count)
        .map(|_| Ok((cursor.u32()?, cursor.u32()?)))
        .collect::<io::Result<_>>()?;
    Ok(Record::VariantSet { index, pairs })
}

fn decode_variable_set(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let index = cursor.u32()?;
    let count = cursor.u32()? as usize;
    if cursor.0.len() != count * 8 {
        return Err(invalid(
            "variable set record length does not match its count",
        ));
    }
    let pairs = (0..count)
        .map(|_| Ok((cursor.u32()?, f32::from_bits(cursor.u32()?))))
        .collect::<io::Result<_>>()?;
    Ok(Record::VariableSet { index, pairs })
}

fn optional(index: u32) -> Option<u32> {
    (index != NONE).then_some(index)
}

/// The hook writes r, g, b, flags as bytes, so the flags are the top byte.
fn decode_color(raw: u32) -> Option<u32> {
    let flags = (raw >> 24) as u8;
    (flags & COLOR_MINIMAP != 0).then(|| {
        let [r, g, b, _] = raw.to_le_bytes();
        u32::from_be_bytes([0, r, g, b])
    })
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn bytes(&mut self, length: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < length {
            return Err(invalid("record truncated"));
        }
        let (bytes, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(bytes)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> io::Result<u32> {
        let (bytes, rest) = self
            .0
            .split_first_chunk::<4>()
            .ok_or_else(|| invalid("record truncated"))?;
        self.0 = rest;
        Ok(u32::from_le_bytes(*bytes))
    }

    fn i32(&mut self) -> io::Result<i32> {
        self.u32().map(|value| value as i32)
    }

    /// u16 length and UTF-8.
    fn string(&mut self, error: &str) -> io::Result<String> {
        let length = self.u16()? as usize;
        String::from_utf8(self.bytes(length)?.to_vec()).map_err(|_| invalid(error))
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message.into())
}
