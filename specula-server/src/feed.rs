//! Decoder for the observer hook's feed, format v6. The writer is
//! `source/network/SpeculaFeed.cpp` in the `specula` branch of 0ad-specula.

use std::io;
use std::io::ErrorKind;
use std::io::Read;

const MAGIC: [u8; 4] = *b"SPCF";
const VERSION: u32 = 6;
const RECORD_TEMPLATE: u8 = 1;
const RECORD_TURN: u8 = 2;
const RECORD_MAP: u8 = 3;
const RECORD_TERRAIN: u8 = 4;
const RECORD_PLAYERS: u8 = 5;
const RECORD_STRING: u8 = 6;
const RECORD_VARIANT_SET: u8 = 7;
const ENTITY_SIZE: usize = 64;
const COLOR_MINIMAP: u8 = 1;
const HAS_VISUAL: u8 = 2;
const ANIMATION_ONCE: u8 = 4;
const NONE: u32 = 0xFFFF_FFFF;
const TEMPLATE_PLAYER_COLOR: u8 = 1;

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
}

/// What the entity's VisualActor shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Visual {
    /// Index of a string record.
    pub actor: Option<u32>,
    pub seed: u32,
    /// Index of a variant set record; `None` for no selections.
    pub variants: Option<u32>,
    /// Index of a string record.
    pub animation: Option<u32>,
    /// Plays once rather than loops.
    pub once: bool,
    /// Raw 16.16 fixed point; m/s for walk and run, else a multiplier.
    pub speed: i32,
    /// Raw 16.16 fixed point, in milliseconds; 0 when not synced.
    pub sync_repeat: i32,
    pub sync_offset: i32,
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
    Terrain(Terrain),
    Players(Vec<Player>),
    String {
        index: u32,
        text: String,
    },
    /// (key, value) string indices, sorted by key.
    VariantSet {
        index: u32,
        pairs: Vec<(u32, u32)>,
    },
}

/// Sent once per match; heights and tiles are row-major, x fastest.
pub struct Terrain {
    pub tiles_per_side: u32,
    /// Raw 16.16 fixed point, in metres.
    pub water_level: i32,
    /// In metres; water shallower than this is drawn as a ford.
    pub shallow_depth: f32,
    pub circular: bool,
    /// (tiles + 1)^2 vertices, in 1/92 m.
    pub heights: Vec<u16>,
    pub textures: Vec<String>,
    /// tiles^2 indices into `textures`; 0xFFFF for none.
    pub tiles: Vec<u16>,
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
            animation: optional(cursor.u32()?),
            once: flags & ANIMATION_ONCE != 0,
            speed: cursor.i32()?,
            sync_repeat: cursor.i32()?,
            sync_offset: cursor.i32()?,
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
            construction: cursor.i32()?,
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
    let vertices = (tiles_per_side as usize + 1).pow(2);
    let heights = (0..vertices)
        .map(|_| cursor.u16())
        .collect::<io::Result<_>>()?;
    let count = cursor.u32()?;
    let mut textures = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let length = cursor.u16()? as usize;
        let name = cursor.bytes(length)?;
        textures
            .push(String::from_utf8(name.to_vec()).map_err(|_| invalid("texture name not UTF-8"))?);
    }
    let tiles = (0..(tiles_per_side as usize).pow(2))
        .map(|_| cursor.u16())
        .collect::<io::Result<_>>()?;
    if !cursor.0.is_empty() {
        return Err(invalid("terrain record longer than its contents"));
    }
    Ok(Record::Terrain(Terrain {
        tiles_per_side,
        water_level,
        shallow_depth,
        circular,
        heights,
        textures,
        tiles,
    }))
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
        players.push(Player {
            name,
            color: u32::from_be_bytes([0, r, g, b]),
            team,
            state,
            stances,
        });
    }
    if !cursor.0.is_empty() {
        return Err(invalid("players record longer than its contents"));
    }
    Ok(Record::Players(players))
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
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message.into())
}
