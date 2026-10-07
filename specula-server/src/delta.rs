//! Measures how many bytes a turn-to-turn delta stream needs. Each stage adds
//! one optimization to the one before, so the report shows what each buys:
//!
//! - `full`: the hook's snapshot, every entity every turn.
//! - `delta`: spawned, destroyed and changed entities only, fixed width.
//! - `varint`: sorted ids delta-coded, fields as zigzag varint differences.
//! - `quantized`: as `varint`, with position in 1/16 m and angle in 1/256
//!   turn. A change below one step is not sent.
//! - `brotli`: `quantized` through a streaming brotli context, flushed every
//!   turn and reset at every segment boundary, so a viewer can join at any
//!   segment.

use std::collections::BTreeMap;
use std::io;
use std::io::Write;

use crate::feed::Entity;
use crate::wire::Writer;
use crate::wire::diff;
use crate::wire::quantize_angle;
use crate::wire::quantize_position;

/// 25 turns of 200 ms, about 5 s per summary.
const WINDOW_TURNS: u32 = 25;
/// As the viewer stream's: 150 turns of 200 ms, a keyframe every 30 s.
const SEGMENT_TURNS: u32 = 150;
const BROTLI_QUALITY: u32 = 5;
const BROTLI_WINDOW_BITS: u32 = 22;
const TURN_LENGTH_SECS: f64 = 0.2;

const CHANGED_POSITION: u8 = 1;
const CHANGED_ANGLE: u8 = 2;
const CHANGED_OWNER: u8 = 4;
const CHANGED_TEMPLATE: u8 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Quantized {
    template: u32,
    x: i32,
    z: i32,
    angle: u8,
    owner: i32,
}

impl Quantized {
    fn new(entity: &Entity) -> Self {
        Quantized {
            template: entity.template,
            x: quantize_position(entity.x),
            z: quantize_position(entity.z),
            angle: quantize_angle(entity.angle),
            owner: entity.owner,
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Sizes {
    full: u64,
    delta: u64,
    varint: u64,
    quantized: u64,
    brotli: u64,
}

impl std::ops::AddAssign for Sizes {
    fn add_assign(&mut self, other: Sizes) {
        self.full += other.full;
        self.delta += other.delta;
        self.varint += other.varint;
        self.quantized += other.quantized;
        self.brotli += other.brotli;
    }
}

pub struct DeltaMeter {
    previous: Option<BTreeMap<u32, Entity>>,
    previous_quantized: BTreeMap<u32, Quantized>,
    brotli: brotli::CompressorWriter<Vec<u8>>,
    turns_in_segment: u32,
    window_first_turn: u32,
    window_turns: u32,
    window: Sizes,
    keyframe_quantized: u64,
    keyframe_brotli: u64,
}

impl DeltaMeter {
    pub fn new() -> Self {
        DeltaMeter {
            previous: None,
            previous_quantized: BTreeMap::new(),
            brotli: new_encoder(),
            turns_in_segment: 0,
            window_first_turn: 0,
            window_turns: 0,
            window: Sizes::default(),
            keyframe_quantized: 0,
            keyframe_brotli: 0,
        }
    }

    /// Returns a summary line when a window is complete.
    pub fn observe(&mut self, turn: u32, entities: &[Entity]) -> io::Result<Option<String>> {
        let current: BTreeMap<u32, Entity> =
            entities.iter().map(|entity| (entity.id, *entity)).collect();
        let current_quantized: BTreeMap<u32, Quantized> = current
            .iter()
            .map(|(id, entity)| (*id, Quantized::new(entity)))
            .collect();

        if self.turns_in_segment == 0 {
            // A new segment starts with a keyframe, and the brotli context
            // restarts with it: a viewer joining here needs nothing earlier.
            let keyframe = encode_keyframe(turn, &current_quantized);
            self.keyframe_quantized = keyframe.len() as u64;
            let mut compressed = new_encoder();
            compressed.write_all(&keyframe)?;
            self.keyframe_brotli = compressed.into_inner().len() as u64;
            self.brotli = new_encoder();
        }
        self.turns_in_segment = (self.turns_in_segment + 1) % SEGMENT_TURNS;

        // The first turn after joining has nothing to diff against; the
        // viewer gets it as a keyframe.
        let Some(previous) = self.previous.replace(current) else {
            self.previous_quantized = current_quantized;
            return Ok(None);
        };
        let current = self.previous.as_ref().unwrap();

        let quantized = encode_quantized(turn, &self.previous_quantized, &current_quantized);
        let before = self.brotli.get_ref().len();
        self.brotli.write_all(&quantized)?;
        self.brotli.flush()?;
        let brotli = (self.brotli.get_ref().len() - before) as u64;
        self.brotli.get_mut().clear();

        let sizes = Sizes {
            full: 8 + 24 * current.len() as u64,
            delta: encode_fixed(turn, &previous, current).len() as u64,
            varint: encode_varint(turn, &previous, current).len() as u64,
            quantized: quantized.len() as u64,
            brotli,
        };
        self.previous_quantized = current_quantized;

        if self.window_turns == 0 {
            self.window_first_turn = turn;
        }
        self.window_turns += 1;
        self.window += sizes;
        if self.window_turns < WINDOW_TURNS {
            return Ok(None);
        }
        let summary = self.summary(turn);
        self.window_turns = 0;
        self.window = Sizes::default();
        Ok(Some(summary))
    }

    fn summary(&self, last_turn: u32) -> String {
        let turns = u64::from(self.window_turns);
        let secs = f64::from(self.window_turns) * TURN_LENGTH_SECS;
        let rate = |bytes: u64| bytes as f64 / secs / 1024.0;
        let w = &self.window;
        format!(
            "turns {}-{} bytes/turn (KB/s): full {} ({:.1}), delta {} ({:.2}), varint {} ({:.2}), \
             quantized {} ({:.2}), brotli {} ({:.2}); keyframe quantized {} brotli {}",
            self.window_first_turn,
            last_turn,
            w.full / turns,
            rate(w.full),
            w.delta / turns,
            rate(w.delta),
            w.varint / turns,
            rate(w.varint),
            w.quantized / turns,
            rate(w.quantized),
            w.brotli / turns,
            rate(w.brotli),
            self.keyframe_quantized,
            self.keyframe_brotli,
        )
    }
}

fn new_encoder() -> brotli::CompressorWriter<Vec<u8>> {
    brotli::CompressorWriter::new(Vec::new(), 4096, BROTLI_QUALITY, BROTLI_WINDOW_BITS)
}

fn changed_mask(position: bool, angle: bool, owner: bool, template: bool) -> u8 {
    let mut mask = 0;
    if position {
        mask |= CHANGED_POSITION;
    }
    if angle {
        mask |= CHANGED_ANGLE;
    }
    if owner {
        mask |= CHANGED_OWNER;
    }
    if template {
        mask |= CHANGED_TEMPLATE;
    }
    mask
}

fn encode_fixed(
    turn: u32,
    previous: &BTreeMap<u32, Entity>,
    current: &BTreeMap<u32, Entity>,
) -> Vec<u8> {
    let diff = diff(previous, current);
    let mut out = Writer::default();
    out.u32(turn);
    out.u32(diff.spawned.len() as u32);
    out.u32(diff.destroyed.len() as u32);
    out.u32(diff.changed.len() as u32);
    for (id, entity) in &diff.spawned {
        out.u32(*id);
        out.u32(entity.template);
        out.i32(entity.x);
        out.i32(entity.z);
        out.i32(entity.angle);
        out.i32(entity.owner);
    }
    for id in &diff.destroyed {
        out.u32(*id);
    }
    for (id, before, now) in &diff.changed {
        let position = before.x != now.x || before.z != now.z;
        let mask = changed_mask(
            position,
            before.angle != now.angle,
            before.owner != now.owner,
            before.template != now.template,
        );
        out.u32(*id);
        out.u8(mask);
        if position {
            out.i32(now.x);
            out.i32(now.z);
        }
        if mask & CHANGED_ANGLE != 0 {
            out.i32(now.angle);
        }
        if mask & CHANGED_OWNER != 0 {
            out.i32(now.owner);
        }
        if mask & CHANGED_TEMPLATE != 0 {
            out.u32(now.template);
        }
    }
    out.0
}

fn encode_varint(
    turn: u32,
    previous: &BTreeMap<u32, Entity>,
    current: &BTreeMap<u32, Entity>,
) -> Vec<u8> {
    let diff = diff(previous, current);
    let mut out = Writer::default();
    out.varint(u64::from(turn));
    out.varint(diff.spawned.len() as u64);
    out.varint(diff.destroyed.len() as u64);
    out.varint(diff.changed.len() as u64);
    let mut last_id = 0;
    for (id, entity) in &diff.spawned {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        out.varint(u64::from(entity.template));
        out.zigzag(i64::from(entity.x));
        out.zigzag(i64::from(entity.z));
        out.zigzag(i64::from(entity.angle));
        out.zigzag(i64::from(entity.owner));
    }
    let mut last_id = 0;
    for id in &diff.destroyed {
        out.varint(u64::from(id - last_id));
        last_id = *id;
    }
    let mut last_id = 0;
    for (id, before, now) in &diff.changed {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        let position = before.x != now.x || before.z != now.z;
        let mask = changed_mask(
            position,
            before.angle != now.angle,
            before.owner != now.owner,
            before.template != now.template,
        );
        out.u8(mask);
        if position {
            out.zigzag(i64::from(now.x) - i64::from(before.x));
            out.zigzag(i64::from(now.z) - i64::from(before.z));
        }
        if mask & CHANGED_ANGLE != 0 {
            out.zigzag(i64::from(now.angle) - i64::from(before.angle));
        }
        if mask & CHANGED_OWNER != 0 {
            out.zigzag(i64::from(now.owner));
        }
        if mask & CHANGED_TEMPLATE != 0 {
            out.varint(u64::from(now.template));
        }
    }
    out.0
}

fn encode_quantized(
    turn: u32,
    previous: &BTreeMap<u32, Quantized>,
    current: &BTreeMap<u32, Quantized>,
) -> Vec<u8> {
    let diff = diff(previous, current);
    let mut out = Writer::default();
    out.varint(u64::from(turn));
    out.varint(diff.spawned.len() as u64);
    out.varint(diff.destroyed.len() as u64);
    out.varint(diff.changed.len() as u64);
    let mut last_id = 0;
    for (id, entity) in &diff.spawned {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_quantized(&mut out, entity);
    }
    let mut last_id = 0;
    for id in &diff.destroyed {
        out.varint(u64::from(id - last_id));
        last_id = *id;
    }
    let mut last_id = 0;
    for (id, before, now) in &diff.changed {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        let position = before.x != now.x || before.z != now.z;
        let mask = changed_mask(
            position,
            before.angle != now.angle,
            before.owner != now.owner,
            before.template != now.template,
        );
        out.u8(mask);
        if position {
            out.zigzag(i64::from(now.x) - i64::from(before.x));
            out.zigzag(i64::from(now.z) - i64::from(before.z));
        }
        if mask & CHANGED_ANGLE != 0 {
            // The shortest way round, so turning past north stays one byte.
            out.zigzag(i64::from(now.angle.wrapping_sub(before.angle) as i8));
        }
        if mask & CHANGED_OWNER != 0 {
            out.zigzag(i64::from(now.owner));
        }
        if mask & CHANGED_TEMPLATE != 0 {
            out.varint(u64::from(now.template));
        }
    }
    out.0
}

fn encode_keyframe(turn: u32, current: &BTreeMap<u32, Quantized>) -> Vec<u8> {
    let mut out = Writer::default();
    out.varint(u64::from(turn));
    out.varint(current.len() as u64);
    let mut last_id = 0;
    for (id, entity) in current {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_quantized(&mut out, entity);
    }
    out.0
}

fn write_quantized(out: &mut Writer, entity: &Quantized) {
    out.varint(u64::from(entity.template));
    out.zigzag(i64::from(entity.x));
    out.zigzag(i64::from(entity.z));
    out.u8(entity.angle);
    out.zigzag(i64::from(entity.owner));
}
