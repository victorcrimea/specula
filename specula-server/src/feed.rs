//! Decoder for the observer hook's feed, format v3. The writer is
//! `source/network/SpeculaFeed.cpp` in the `specula` branch of 0ad-specula.

use std::io;
use std::io::ErrorKind;
use std::io::Read;

const MAGIC: [u8; 4] = *b"SPCF";
const VERSION: u32 = 3;
const RECORD_TEMPLATE: u8 = 1;
const RECORD_TURN: u8 = 2;
const RECORD_MAP: u8 = 3;
const RECORD_TERRAIN: u8 = 4;
const ENTITY_SIZE: usize = 28;
const COLOR_MINIMAP: u8 = 1;

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
}

/// A template's minimap icon, as the in-game minimap draws it.
pub struct Icon {
    /// Relative to art/textures/ui/session/icons/minimap/.
    pub name: String,
    /// In GUI pixels.
    pub size: f32,
}

pub enum Record {
    Template {
        index: u32,
        name: String,
        icon: Option<Icon>,
    },
    Turn {
        turn: u32,
        entities: Vec<Entity>,
    },
    /// Map size in metres.
    Map {
        size: u32,
    },
    Terrain(Terrain),
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
                return Cursor(&payload)
                    .u32()
                    .map(|size| Some(Record::Map { size }));
            }
            RECORD_TERRAIN => return decode_terrain(&payload).map(Some),
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
    let icon_name =
        String::from_utf8(cursor.0.to_vec()).map_err(|_| invalid("icon name not UTF-8"))?;
    let icon = (!icon_name.is_empty()).then_some(Icon {
        name: icon_name,
        size,
    });
    Ok(Record::Template { index, name, icon })
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
        entities.push(Entity {
            id: cursor.u32()?,
            template: cursor.u32()?,
            x: cursor.i32()?,
            z: cursor.i32()?,
            angle: cursor.i32()?,
            owner: cursor.i32()?,
            color: decode_color(cursor.u32()?),
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
