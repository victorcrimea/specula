//! Byte-level helpers shared by the delta meter and the viewer stream.

use std::collections::BTreeMap;

/// 16.16 fixed point: 4096 raw units are 1/16 m.
const POSITION_STEP: i32 = 4096;

/// Raw 16.16 metres to 1/16 m, rounded to the nearest step.
pub fn quantize_position(raw: i32) -> i32 {
    (raw + POSITION_STEP / 2).div_euclid(POSITION_STEP)
}

pub struct Diff<'a, T> {
    pub spawned: Vec<(u32, &'a T)>,
    pub destroyed: Vec<u32>,
    pub changed: Vec<(u32, &'a T, &'a T)>,
}

pub fn diff<'a, T: PartialEq>(
    previous: &'a BTreeMap<u32, T>,
    current: &'a BTreeMap<u32, T>,
) -> Diff<'a, T> {
    let mut result = Diff {
        spawned: Vec::new(),
        destroyed: Vec::new(),
        changed: Vec::new(),
    };
    for (id, now) in current {
        match previous.get(id) {
            None => result.spawned.push((*id, now)),
            Some(before) if before != now => result.changed.push((*id, before, now)),
            Some(_) => {}
        }
    }
    result.destroyed = previous
        .keys()
        .filter(|id| !current.contains_key(id))
        .copied()
        .collect();
    result
}

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    pub fn u16(&mut self, value: u16) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn i32(&mut self, value: i32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn f32(&mut self, value: f32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push(value as u8 | 0x80);
            value >>= 7;
        }
        self.0.push(value as u8);
    }

    pub fn zigzag(&mut self, value: i64) {
        self.varint(((value << 1) ^ (value >> 63)) as u64);
    }
}
