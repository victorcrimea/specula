//! Decoder for specula-server's viewer stream, wire format v1; the layout is
//! documented in `specula-server/src/view.rs`.

use std::collections::BTreeMap;

const MESSAGE_KEYFRAME: u8 = 1;
const MESSAGE_DELTA: u8 = 2;
const CHANGED_POSITION: u8 = 1;
const CHANGED_COLOR: u8 = 2;
/// A move longer than this in one turn is a teleport (ungarrison, a
/// scripted jump), not walking, so the dot does not slide there. It is well
/// above cavalry at 2x game speed.
const SNAP_UNITS: f32 = 24.0 * 16.0;

#[derive(Clone, Copy)]
pub struct Dot {
    /// In 1/16 m.
    pub x: i32,
    pub z: i32,
    /// 0xFFRRGGBB, or 0 when the entity is not on the minimap.
    pub color: u32,
    /// Where the dot was on screen when the current turn arrived, in 1/16 m;
    /// the renderer moves it from here to (x, z) over one turn.
    pub from_x: f32,
    pub from_z: f32,
}

#[derive(Default)]
pub struct Scene {
    pub turn: u32,
    /// In metres; 0 until the first keyframe.
    pub map_size: u32,
    /// Names `/terrain/<id>`; 0 while the server has none.
    pub terrain_id: u64,
    pub dots: BTreeMap<u32, Dot>,
}

impl Scene {
    /// `progress` is how far the renderer has moved the dots towards the
    /// current turn, 0..=1.
    pub fn apply(&mut self, message: &[u8], progress: f32) -> Result<(), &'static str> {
        let (&kind, body) = message.split_first().ok_or("empty message")?;
        let mut cursor = Cursor(body);
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
        self.map_size = cursor.varint_u32()?;
        self.terrain_id = cursor.u64()?;
        self.dots.clear();
        let count = cursor.varint_u32()?;
        let mut id = 0;
        for _ in 0..count {
            id += cursor.varint_u32()?;
            let dot = cursor.dot()?;
            self.dots.insert(id, dot);
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
        }
        self.turn = cursor.varint_u32()?;
        let spawned = cursor.varint_u32()?;
        let destroyed = cursor.varint_u32()?;
        let changed = cursor.varint_u32()?;
        let mut id = 0;
        for _ in 0..spawned {
            id += cursor.varint_u32()?;
            let dot = cursor.dot()?;
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
            let mask = cursor.u8()?;
            let dot = self
                .dots
                .get_mut(&id)
                .ok_or("change to an unknown entity")?;
            if mask & CHANGED_POSITION != 0 {
                dot.x += cursor.zigzag()?;
                dot.z += cursor.zigzag()?;
                let distance = (dot.x as f32 - dot.from_x).hypot(dot.z as f32 - dot.from_z);
                if distance > SNAP_UNITS {
                    dot.from_x = dot.x as f32;
                    dot.from_z = dot.z as f32;
                }
            }
            if mask & CHANGED_COLOR != 0 {
                dot.color = cursor.u32()?;
            }
        }
        Ok(())
    }
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
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

    fn zigzag(&mut self) -> Result<i32, &'static str> {
        let raw = self.varint()?;
        let value = (raw >> 1) as i64 ^ -((raw & 1) as i64);
        i32::try_from(value).map_err(|_| "zigzag out of range")
    }

    /// A new dot appears in place rather than sliding in from anywhere.
    fn dot(&mut self) -> Result<Dot, &'static str> {
        let x = self.zigzag()?;
        let z = self.zigzag()?;
        Ok(Dot {
            x,
            z,
            color: self.u32()?,
            from_x: x as f32,
            from_z: z as f32,
        })
    }
}
