//! The detail stream (stream 2, CONTROL.md 8.1): what the selection panels
//! show beyond the status bars, for every entity, sent only to viewers that
//! have something selected. Varints and zigzag as in `wire.rs`.
//!
//! - kind 1 keyframe: varint turn, u8 detail version (1; a viewer ignores
//!   the stream at any other), u64 match id (the visual stream's), list
//!   table, varint n, n x {varint template, varint mask, groups}: each
//!   template's defaults, then varint count, count x {varint id delta,
//!   varint mask, groups}. A keyframe leaves out a group equal to the
//!   default of the entity's template (the template of the visual stream's
//!   entity of that id) and an entity with nothing left. The groups of
//!   production queues, foundations and repairs are never defaults, and in
//!   a keyframe each is followed by varint turns since it was sent.
//! - kind 2 delta: varint turn, the lists added to the table, varint
//!   count, count x {varint id delta, varint mask, groups} of the groups
//!   that changed, varint count, count x id delta of the entities gone.
//!   Every turn of the visual stream has one, empty or not, so the two
//!   streams line up turn by turn.
//!
//! An entity's groups are kept until it is gone; one a delta does not list
//! is unchanged. A group a keyframe left out is sent in the delta of the
//! turn its entity leaves the visual stream (garrisoned), since a viewer
//! can no longer find its template; the defaults assume that entities of
//! one template have the same groups. The mask bits are the hook's kind 12 group bits
//! (`SpeculaFeed.h`), and each group is sent as the panels show it:
//! - 1 health: varint hitpoints, varint max, both rounded up;
//! - 2 capture: varint n, n x varint points by player index, rounded,
//!   varint max rounded up;
//! - 4 resource supply: varint amount, varint max, both rounded up;
//! - 8 gatherers: u8 count, u8 max;
//! - 16 carried: varint n, n x {varint resource type, varint amount
//!   rounded, varint capacity rounded up};
//! - 32 experience: varint experience rounded down, varint required
//!   rounded up;
//! - 64 production queue: u8 autoqueue, varint n, n x {varint item id, u8
//!   kind (0 unit, 1 technology), varint template or technology, varint
//!   count, u8 paused, varint missing population, varint time remaining,
//!   varint total time, both in 100 ms};
//! - 128 garrison: varint capacity, varint occupied slots, varint n, n x
//!   {zigzag id minus the one before (from 0), varint template};
//! - 256 foundation, 512 repair: u8 builders, varint seconds remaining in
//!   1/10;
//! - 1024 unit AI: u8 idle, varint stance, varint formation controller id
//!   (0: none);
//! - 2048 trainable, 4096 researchable: varint index into the list table;
//! - 8192 progress: u8 pack, u8 upgrade, each 255 when not running, else
//!   0..=254 of done, varint upgrade target;
//! - 16384 holder: varint id of the entity it is garrisoned in or a turret
//!   on, 0 for none;
//! - 32768 trader: varint goods type, varint total gain rounded.
//!
//! Strings (resource types, templates and technologies, stances, the
//! upgrade target, goods) are 1-based indices into the visual stream's
//! string table, 0 for none; templates in a garrison are 0-based indices
//! into its template table. A list is varint n, n x string, so the list
//! table is varint count, count x list; it only grows during a match.
//!
//! Reckoning: while the first item of a queue is not paused, its time
//! remaining goes down by 200 ms a turn from the turn it was sent; while a
//! foundation or repair has builders, its seconds remaining go down by 0.2
//! a turn. The server resends a group when the reckoning is more than
//! `SLACK_MS` off. Resource supplies, carried resources and experience are
//! sent at most every `SLOW_TURNS` turns (CONTROL.md 8.4).
//!
//! It is cut into segments and served as the visual stream is (`view.rs`),
//! at `/match/<match id>/detail/<n>` and over the `/detail` WebSocket.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use crate::feed::Building;
use crate::feed::Details;
use crate::feed::EntityDetails;
use crate::feed::Queue;
use crate::wire::Writer;

const MESSAGE_KEYFRAME: u8 = 1;
const MESSAGE_DELTA: u8 = 2;
const DETAIL_VERSION: u8 = 1;
const HEALTH: usize = 0;
const CAPTURE: usize = 1;
const SUPPLY: usize = 2;
const GATHERERS: usize = 3;
const CARRYING: usize = 4;
const EXPERIENCE: usize = 5;
const QUEUE: usize = 6;
const GARRISON: usize = 7;
const FOUNDATION: usize = 8;
const REPAIR: usize = 9;
const UNIT_AI: usize = 10;
const TRAINABLE: usize = 11;
const RESEARCHABLE: usize = 12;
const PROGRESS: usize = 13;
const HOLDER: usize = 14;
const TRADER: usize = 15;
const SLOTS: usize = 16;
const TURN_MS: f64 = 200.0;
/// Production and construction advance in 1 s timer steps, so a reckoning
/// that runs smoothly is up to 1 s off between steps; this is a step plus
/// a turn.
const SLACK_MS: f64 = 1200.0;
/// Once a second: the panels cannot show the difference.
const SLOW_TURNS: u32 = 5;
const PROGRESS_NONE: u8 = 255;

type Groups = [Option<Vec<u8>>; SLOTS];

fn is_slow(slot: usize) -> bool {
    matches!(slot, SUPPLY | CARRYING | EXPERIENCE)
}

fn is_reckoned(slot: usize) -> bool {
    matches!(slot, QUEUE | FOUNDATION | REPAIR)
}

fn one_based(index: Option<u32>) -> u64 {
    index.map_or(0, |index| u64::from(index) + 1)
}

fn up(value: f32) -> u64 {
    if value.is_finite() && value > 0.0 {
        value.ceil() as u64
    } else {
        0
    }
}

fn round(value: f32) -> u64 {
    if value.is_finite() && value > 0.0 {
        value.round() as u64
    } else {
        0
    }
}

fn down(value: f32) -> u64 {
    if value.is_finite() && value > 0.0 {
        value.floor() as u64
    } else {
        0
    }
}

fn progress_byte(progress: f32) -> u8 {
    if progress < 0.0 || !progress.is_finite() {
        PROGRESS_NONE
    } else {
        (progress * 254.0).round().clamp(0.0, 254.0) as u8
    }
}

/// The group in `slot` as the wire sends it, `None` when the entity does
/// not have it. Queue and build times are those of this turn.
fn encode_group(slot: usize, details: &EntityDetails) -> Option<Vec<u8>> {
    let mut out = Writer::default();
    match slot {
        HEALTH | SUPPLY => {
            let (value, max) = if slot == HEALTH {
                details.health?
            } else {
                details.supply?
            };
            out.varint(up(value));
            out.varint(up(max));
        }
        CAPTURE => {
            let capture = details.capture.as_ref()?;
            out.varint(capture.points.len() as u64);
            for points in &capture.points {
                out.varint(round(*points));
            }
            out.varint(up(capture.max));
        }
        GATHERERS => {
            let (count, max) = details.gatherers?;
            out.u8(count);
            out.u8(max);
        }
        CARRYING => {
            let carrying = details.carrying.as_ref()?;
            out.varint(carrying.len() as u64);
            for carried in carrying {
                out.varint(one_based(carried.kind));
                out.varint(round(carried.amount));
                out.varint(up(carried.capacity));
            }
        }
        EXPERIENCE => {
            let (experience, required) = details.experience?;
            out.varint(down(experience));
            out.varint(up(required));
        }
        QUEUE => write_queue(&mut out, details.queue.as_ref()?),
        GARRISON => {
            let garrison = details.garrison.as_ref()?;
            out.varint(u64::from(garrison.capacity));
            out.varint(u64::from(garrison.occupied));
            out.varint(garrison.entities.len() as u64);
            let mut previous = 0i64;
            for (id, template) in &garrison.entities {
                out.zigzag(i64::from(*id) - previous);
                previous = i64::from(*id);
                out.varint(u64::from(*template));
            }
        }
        FOUNDATION | REPAIR => {
            let building = if slot == FOUNDATION {
                details.foundation.as_ref()?
            } else {
                details.repair.as_ref()?
            };
            write_building(&mut out, building);
        }
        UNIT_AI => {
            // The FSM state changes with every order step and no panel
            // shows it; the idle filter and the stance do.
            let unit_ai = details.unit_ai.as_ref()?;
            out.u8(u8::from(unit_ai.idle));
            out.varint(one_based(unit_ai.stance));
            out.varint(u64::from(unit_ai.formation_controller));
        }
        TRAINABLE => out.varint(u64::from(details.trainable?)),
        RESEARCHABLE => out.varint(u64::from(details.researchable?)),
        PROGRESS => {
            let progress = details.progress.as_ref()?;
            out.u8(progress_byte(progress.pack));
            out.u8(progress_byte(progress.upgrade));
            out.varint(one_based(progress.upgrade_target));
        }
        HOLDER => out.varint(u64::from(details.holder?)),
        TRADER => {
            let trader = details.trader.as_ref()?;
            out.varint(one_based(trader.goods));
            out.varint(round(trader.gain));
        }
        _ => unreachable!(),
    }
    Some(out.0)
}

/// Everything but the times, which the client reckons.
fn queue_structure(queue: &Queue) -> Vec<u8> {
    let mut out = Writer::default();
    out.u8(u8::from(queue.autoqueue));
    out.varint(queue.items.len() as u64);
    for item in &queue.items {
        out.varint(u64::from(item.id));
        out.u8(u8::from(item.technology));
        out.varint(one_based(item.name));
        out.varint(u64::from(item.count));
        out.u8(u8::from(item.paused));
        out.varint(u64::from(item.missing_population));
    }
    out.0
}

fn write_queue(out: &mut Writer, queue: &Queue) {
    out.u8(u8::from(queue.autoqueue));
    out.varint(queue.items.len() as u64);
    for item in &queue.items {
        out.varint(u64::from(item.id));
        out.u8(u8::from(item.technology));
        out.varint(one_based(item.name));
        out.varint(u64::from(item.count));
        out.u8(u8::from(item.paused));
        out.varint(u64::from(item.missing_population));
        let remaining = item.time_remaining.max(0.0);
        // The total lets the client draw progress from the time it reckons.
        let total = if item.progress < 1.0 {
            remaining / (1.0 - item.progress.max(0.0))
        } else {
            remaining
        };
        out.varint(round(remaining / 100.0));
        out.varint(round(total / 100.0));
    }
}

fn write_building(out: &mut Writer, building: &Building) {
    out.u8(building.builders);
    out.varint(round(building.seconds_remaining * 10.0));
}

/// What a viewer reckons a group from: the first queue item's time
/// remaining in ms and whether it runs, or a building's seconds remaining
/// in ms and whether it has builders, as of `turn`.
#[derive(Clone, Copy)]
struct Base {
    ms: f64,
    running: bool,
    turn: u32,
}

impl Base {
    fn reckon(&self, turn: u32) -> f64 {
        if self.running {
            (self.ms - TURN_MS * f64::from(turn - self.turn)).max(0.0)
        } else {
            self.ms
        }
    }
}

fn queue_base(queue: &Queue, turn: u32) -> Base {
    let first = queue.items.first();
    Base {
        ms: first.map_or(0.0, |item| f64::from(item.time_remaining)),
        running: first.is_some_and(|item| !item.paused),
        turn,
    }
}

fn building_base(building: &Building, turn: u32) -> Base {
    Base {
        ms: f64::from(building.seconds_remaining) * 1000.0,
        running: building.builders > 0,
        turn,
    }
}

/// What viewers hold of one entity.
#[derive(Default)]
struct Sent {
    groups: Groups,
    /// The queue's structure as last sent, to tell a new item from time
    /// passing.
    queue_structure: Option<Vec<u8>>,
    /// By slot, for the reckoned ones.
    bases: [Option<Base>; SLOTS],
    /// The slots the last keyframe left out as its template's defaults,
    /// and no delta has sent since.
    defaulted: u32,
}

pub struct DetailEncoder {
    /// By id; an entity without any group has no entry.
    sent: BTreeMap<u32, Sent>,
    /// Entities whose slow groups changed since they were last sampled.
    pending_slow: BTreeSet<u32>,
    /// The list table: 1-based strings.
    lists: Vec<Vec<u64>>,
    lists_sent: usize,
}

impl DetailEncoder {
    pub fn new() -> Self {
        DetailEncoder {
            sent: BTreeMap::new(),
            pending_slow: BTreeSet::new(),
            lists: Vec::new(),
            lists_sent: 0,
        }
    }

    /// A new feed numbers its entities and lists afresh.
    pub fn reset(&mut self) {
        *self = DetailEncoder::new();
    }

    /// The turn's delta; afterwards `sent` is what a viewer holds.
    /// `in_visual` tells whether the visual stream has an entity of the id
    /// after this turn.
    pub fn delta(
        &mut self,
        turn: u32,
        details: &Details,
        in_visual: impl Fn(u32) -> bool,
    ) -> Vec<u8> {
        // The hook numbers its lists densely and sends each before its
        // first use.
        while let Some(list) = details.string_lists.get(&(self.lists.len() as u32)) {
            self.lists
                .push(list.iter().map(|string| one_based(*string)).collect());
        }
        let sample = turn.is_multiple_of(SLOW_TURNS);
        let mut candidates: BTreeSet<u32> = details.changed.iter().copied().collect();
        candidates.extend(
            self.sent
                .iter()
                .filter(|(_, sent)| sent.bases.iter().any(Option::is_some))
                .map(|(id, _)| *id),
        );
        if sample {
            candidates.extend(std::mem::take(&mut self.pending_slow));
        }
        // A viewer finds a group left out by the entity's template, which
        // it can no longer look up once the entity leaves the visual stream
        // (garrisoned), so those groups go out explicitly then.
        let leaving: BTreeMap<u32, u32> = self
            .sent
            .iter()
            .filter(|(id, sent)| sent.defaulted != 0 && !in_visual(**id))
            .map(|(id, sent)| (*id, sent.defaulted))
            .collect();
        candidates.extend(leaving.keys());
        let mut out = Writer::default();
        out.u8(MESSAGE_DELTA);
        out.varint(u64::from(turn));
        write_lists(&mut out, &self.lists[self.lists_sent..]);
        self.lists_sent = self.lists.len();
        let mut body = Writer::default();
        let mut count = 0u64;
        let mut previous = 0u32;
        for id in candidates {
            let Some(now) = details.entities.get(&id) else {
                continue;
            };
            let mut mask = 0u64;
            let mut groups = Writer::default();
            let forced = leaving.get(&id).copied().unwrap_or(0);
            for slot in 0..SLOTS {
                let force = forced & 1 << slot != 0;
                if let Some(group) = self.decide(id, slot, now, turn, sample, force) {
                    if let Some(sent) = self.sent.get_mut(&id) {
                        sent.defaulted &= !(1 << slot);
                    }
                    mask |= 1 << slot;
                    groups.0.extend_from_slice(&group);
                }
            }
            if mask == 0 {
                continue;
            }
            body.varint(u64::from(id - previous));
            previous = id;
            body.varint(mask);
            body.0.extend_from_slice(&groups.0);
            count += 1;
        }
        out.varint(count);
        out.0.extend_from_slice(&body.0);
        let mut gone: Vec<u32> = details
            .gone
            .iter()
            .copied()
            .filter(|id| self.sent.remove(id).is_some())
            .collect();
        gone.sort_unstable();
        for id in &gone {
            self.pending_slow.remove(id);
        }
        out.varint(gone.len() as u64);
        let mut previous = 0u32;
        for id in gone {
            out.varint(u64::from(id - previous));
            previous = id;
        }
        out.0
    }

    /// The group as it goes into the delta, or `None` when what the viewer
    /// holds, or reckons, is close enough and it is not `force`d.
    fn decide(
        &mut self,
        id: u32,
        slot: usize,
        now: &EntityDetails,
        turn: u32,
        sample: bool,
        force: bool,
    ) -> Option<Vec<u8>> {
        let sent = self.sent.entry(id).or_default();
        let base = match slot {
            QUEUE => now.queue.as_ref().map(|queue| queue_base(queue, turn)),
            FOUNDATION => now.foundation.as_ref().map(|b| building_base(b, turn)),
            REPAIR => now.repair.as_ref().map(|b| building_base(b, turn)),
            _ => None,
        };
        let group = encode_group(slot, now)?;
        if is_reckoned(slot) {
            let base = base?;
            let stale = match (sent.bases[slot], &sent.groups[slot]) {
                (Some(sent_base), Some(sent_group)) => {
                    let structure_changed = if slot == QUEUE {
                        let structure = now.queue.as_ref().map(queue_structure);
                        let changed = sent.queue_structure != structure;
                        sent.queue_structure = structure;
                        changed
                    } else {
                        // The builder count is the first byte.
                        sent_group.first() != group.first()
                    };
                    structure_changed
                        || sent_base.running != base.running
                        || (sent_base.reckon(turn) - base.ms).abs() > SLACK_MS
                }
                _ => {
                    if slot == QUEUE {
                        sent.queue_structure = now.queue.as_ref().map(queue_structure);
                    }
                    true
                }
            };
            if !stale {
                return None;
            }
            sent.bases[slot] = Some(base);
            sent.groups[slot] = Some(group.clone());
            return Some(group);
        }
        if force {
            sent.groups[slot] = Some(group.clone());
            return Some(group);
        }
        if sent.groups[slot].as_deref() == Some(group.as_slice()) {
            return None;
        }
        // A slow group a viewer has not got at all goes at once.
        if is_slow(slot) && !sample && sent.groups[slot].is_some() {
            self.pending_slow.insert(id);
            return None;
        }
        sent.groups[slot] = Some(group.clone());
        Some(group)
    }

    /// What a viewer holds after the last delta; `template_of` gives the
    /// template of the visual stream's entity of an id, for the defaults.
    pub fn keyframe(
        &mut self,
        turn: u32,
        match_id: u64,
        template_of: impl Fn(u32) -> Option<u32>,
    ) -> Vec<u8> {
        let templates: HashMap<u32, u32> = self
            .sent
            .keys()
            .filter_map(|id| Some((*id, template_of(*id)?)))
            .collect();
        let defaults = self.defaults(&templates);
        let mut out = Writer::default();
        out.u8(MESSAGE_KEYFRAME);
        out.varint(u64::from(turn));
        out.u8(DETAIL_VERSION);
        out.u64(match_id);
        write_lists(&mut out, &self.lists);
        out.varint(defaults.len() as u64);
        for (template, groups) in &defaults {
            out.varint(u64::from(*template));
            write_groups(&mut out, groups, None);
        }
        let mut body = Writer::default();
        let mut count = 0u64;
        let mut previous = 0u32;
        for (id, sent) in &mut self.sent {
            let template_defaults = templates
                .get(id)
                .and_then(|template| defaults.get(template));
            let mut groups: Groups = sent.groups.clone();
            sent.defaulted = 0;
            if let Some(template_defaults) = template_defaults {
                for slot in 0..SLOTS {
                    if groups[slot].is_some() && groups[slot] == template_defaults[slot] {
                        groups[slot] = None;
                        sent.defaulted |= 1 << slot;
                    }
                }
            }
            if groups.iter().all(Option::is_none) {
                continue;
            }
            body.varint(u64::from(id - previous));
            previous = *id;
            write_groups(&mut body, &groups, Some((&sent.bases, turn)));
            count += 1;
        }
        out.varint(count);
        out.0.extend_from_slice(&body.0);
        out.0
    }

    /// Each template's most common value of each group among its entities,
    /// the reckoned ones left out.
    fn defaults(&self, templates: &HashMap<u32, u32>) -> BTreeMap<u32, Groups> {
        let mut counts: HashMap<(u32, usize, &[u8]), u32> = HashMap::new();
        for (id, sent) in &self.sent {
            let Some(template) = templates.get(id) else {
                continue;
            };
            for (slot, group) in sent.groups.iter().enumerate() {
                if let Some(group) = group
                    && !is_reckoned(slot)
                {
                    *counts.entry((*template, slot, group)).or_default() += 1;
                }
            }
        }
        let mut best: HashMap<(u32, usize), (u32, &[u8])> = HashMap::new();
        for ((template, slot, group), count) in counts {
            let entry = best.entry((template, slot)).or_insert((0, group));
            // Ties go to the smaller bytes, so the table does not depend on
            // the hash order.
            if count > entry.0 || (count == entry.0 && group < entry.1) {
                *entry = (count, group);
            }
        }
        let mut defaults: BTreeMap<u32, Groups> = BTreeMap::new();
        for ((template, slot), (count, group)) in best {
            // A value only one entity has saves nothing.
            if count > 1 {
                defaults
                    .entry(template)
                    .or_insert_with(|| std::array::from_fn(|_| None))[slot] = Some(group.to_vec());
            }
        }
        defaults
    }
}

fn write_lists(out: &mut Writer, lists: &[Vec<u64>]) {
    out.varint(lists.len() as u64);
    for list in lists {
        out.varint(list.len() as u64);
        for string in list {
            out.varint(*string);
        }
    }
}

/// The mask, then the groups; with `bases`, each reckoned group is followed
/// by the turns since it was sent.
fn write_groups(out: &mut Writer, groups: &Groups, bases: Option<(&[Option<Base>; SLOTS], u32)>) {
    let mask = groups
        .iter()
        .enumerate()
        .filter(|(_, group)| group.is_some())
        .fold(0u64, |mask, (slot, _)| mask | 1 << slot);
    out.varint(mask);
    for (slot, group) in groups.iter().enumerate() {
        let Some(group) = group else {
            continue;
        };
        out.0.extend_from_slice(group);
        if let Some((bases, turn)) = bases
            && is_reckoned(slot)
        {
            let since = bases[slot].map_or(0, |base| turn - base.turn);
            out.varint(u64::from(since));
        }
    }
}
