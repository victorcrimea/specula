//! The detail stream (stream 2): what the selection panels show beyond the
//! status bars. The format is in the server's `detail.rs`; its keyframes
//! and deltas are messages of kind 1 and 2 as the visual stream's, carried
//! in segments and envelopes the same way, so `stream.rs` splits them.

use std::collections::BTreeMap;

use crate::TURN_MS;
use crate::stream::Cursor;
use crate::stream::Writer;

const MESSAGE_KEYFRAME: u8 = 1;
const MESSAGE_DELTA: u8 = 2;
const DETAIL_VERSION: u8 = 2;
/// Archived matches keep it; it has no garrison owners.
const DETAIL_VERSION_NO_OWNERS: u8 = 1;
/// Slots of `Groups`, the hook's group bits.
pub const HEALTH: usize = 0;
pub const CAPTURE: usize = 1;
pub const SUPPLY: usize = 2;
pub const GATHERERS: usize = 3;
pub const CARRYING: usize = 4;
pub const EXPERIENCE: usize = 5;
const QUEUE: usize = 6;
pub const GARRISON: usize = 7;
pub const FOUNDATION: usize = 8;
pub const REPAIR: usize = 9;
const UNIT_AI: usize = 10;
const TRAINABLE: usize = 11;
const RESEARCHABLE: usize = 12;
const PROGRESS: usize = 13;
const HOLDER: usize = 14;
pub const TRADER: usize = 15;
const SLOTS: usize = 16;

/// By group bit; `None` is a group not sent.
pub type Groups = [Option<Group>; SLOTS];

/// One group as sent. Strings are 1-based indices into `Scene::strings`,
/// 0 for none.
#[derive(Clone, Debug, PartialEq)]
pub enum Group {
    Health {
        hitpoints: u32,
        max: u32,
    },
    Capture {
        /// By player index.
        points: Vec<u32>,
        max: u32,
    },
    Supply {
        amount: u32,
        max: u32,
    },
    Gatherers {
        count: u8,
        max: u8,
    },
    Carrying(Vec<Carried>),
    Experience {
        experience: u32,
        required: u32,
    },
    Queue(Queue),
    Garrison {
        capacity: u32,
        occupied: u32,
        entities: Vec<Garrisoned>,
    },
    /// A foundation or a repair.
    Building(Building),
    UnitAi {
        idle: bool,
        stance: u32,
        /// 0 for none.
        formation_controller: u32,
    },
    /// An index into `Details::lists`.
    List(u32),
    Progress {
        /// 0..=254 of done, 255 when not running.
        pack: u8,
        upgrade: u8,
        upgrade_target: u32,
    },
    /// 0 for none.
    Holder(u32),
    Trader {
        goods: u32,
        gain: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Garrisoned {
    pub id: u32,
    /// 0-based index into `Scene::templates`.
    pub template: u32,
    /// `None` for none, or from a stream without owners.
    pub owner: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Carried {
    pub kind: u32,
    pub amount: u32,
    pub capacity: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Queue {
    pub autoqueue: bool,
    pub items: Vec<QueueItem>,
    /// The turn it was sent on, which the first item's time is reckoned
    /// from.
    pub turn: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueueItem {
    pub id: u32,
    pub technology: bool,
    pub name: u32,
    pub count: u32,
    pub paused: bool,
    pub missing_population: u32,
    /// In 100 ms, as of `Queue::turn`.
    pub remaining: u32,
    pub total: u32,
}

impl Queue {
    /// Item `index`'s time remaining at `turn`, in ms: only the first item
    /// runs.
    #[expect(dead_code, reason = "the details panel (C24) reads them")]
    pub fn remaining_ms(&self, index: usize, turn: u32) -> f64 {
        let Some(item) = self.items.get(index) else {
            return 0.0;
        };
        let sent = f64::from(item.remaining) * 100.0;
        if index > 0 || item.paused {
            return sent;
        }
        (sent - TURN_MS * f64::from(turn.saturating_sub(self.turn))).max(0.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Building {
    pub builders: u8,
    /// In 1/10 s, as of `turn`.
    pub remaining: u32,
    pub turn: u32,
}

impl Building {
    /// At `turn`; it goes down only while someone builds.
    pub fn seconds_remaining(&self, turn: u32) -> f64 {
        let sent = f64::from(self.remaining) / 10.0;
        if self.builders == 0 {
            return sent;
        }
        (sent - TURN_MS / 1000.0 * f64::from(turn.saturating_sub(self.turn))).max(0.0)
    }
}

fn is_reckoned(slot: usize) -> bool {
    matches!(slot, QUEUE | FOUNDATION | REPAIR)
}

/// The detail state at one turn.
#[derive(Default)]
pub struct Details {
    /// False until a keyframe is applied.
    pub known: bool,
    pub turn: u32,
    pub match_id: u64,
    /// Whether its garrisons carry owners, which a delta does not say.
    pub owners: bool,
    /// Lists of 1-based strings.
    pub lists: Vec<Vec<u32>>,
    /// By template index: the groups an entity of it has when its own
    /// leave them out.
    pub defaults: BTreeMap<u32, Groups>,
    /// By id, the groups sent for it.
    pub entities: BTreeMap<u32, Groups>,
}

impl Details {
    pub fn apply(&mut self, message: &[u8]) -> Result<(), &'static str> {
        let (&kind, body) = message.split_first().ok_or("empty message")?;
        let mut cursor = Cursor(body);
        match kind {
            MESSAGE_KEYFRAME => self.apply_keyframe(&mut cursor)?,
            MESSAGE_DELTA => self.apply_delta(&mut cursor)?,
            _ => return Err("unknown detail message kind"),
        }
        if !cursor.0.is_empty() {
            return Err("trailing bytes");
        }
        Ok(())
    }

    fn apply_keyframe(&mut self, cursor: &mut Cursor) -> Result<(), &'static str> {
        let turn = cursor.varint_u32()?;
        let owners = match cursor.u8()? {
            DETAIL_VERSION => true,
            DETAIL_VERSION_NO_OWNERS => false,
            _ => return Err("unknown detail version, reload the page"),
        };
        *self = Details {
            known: true,
            turn,
            match_id: cursor.u64()?,
            owners,
            ..Details::default()
        };
        self.read_lists(cursor)?;
        for _ in 0..cursor.varint()? {
            let template = cursor.varint_u32()?;
            let groups = read_groups(cursor, turn, false, owners)?;
            self.defaults.insert(template, groups);
        }
        let mut id = 0u32;
        for _ in 0..cursor.varint()? {
            id = next_id(id, cursor)?;
            let groups = read_groups(cursor, turn, true, owners)?;
            self.entities.insert(id, groups);
        }
        Ok(())
    }

    fn apply_delta(&mut self, cursor: &mut Cursor) -> Result<(), &'static str> {
        if !self.known {
            return Err("detail delta before a keyframe");
        }
        self.turn = cursor.varint_u32()?;
        self.read_lists(cursor)?;
        let mut id = 0u32;
        for _ in 0..cursor.varint()? {
            id = next_id(id, cursor)?;
            let changed = read_groups(cursor, self.turn, false, self.owners)?;
            let groups = self
                .entities
                .entry(id)
                .or_insert_with(|| std::array::from_fn(|_| None));
            for (slot, group) in changed.into_iter().enumerate() {
                if group.is_some() {
                    groups[slot] = group;
                }
            }
        }
        let mut id = 0u32;
        for _ in 0..cursor.varint()? {
            id = next_id(id, cursor)?;
            self.entities.remove(&id);
        }
        Ok(())
    }

    fn read_lists(&mut self, cursor: &mut Cursor) -> Result<(), &'static str> {
        for _ in 0..cursor.varint()? {
            let list = (0..cursor.varint()?)
                .map(|_| cursor.varint_u32())
                .collect::<Result<_, _>>()?;
            self.lists.push(list);
        }
        Ok(())
    }

    /// The state as a keyframe, which applied to an empty state gives this
    /// one back; for the worker, which hands the page a rebuilt state.
    pub fn encode_keyframe(&self) -> Vec<u8> {
        let mut out = Writer::default();
        out.u8(MESSAGE_KEYFRAME);
        out.varint(u64::from(self.turn));
        // The deltas that follow it are still in its source's version.
        out.u8(if self.owners {
            DETAIL_VERSION
        } else {
            DETAIL_VERSION_NO_OWNERS
        });
        out.u64(self.match_id);
        out.varint(self.lists.len() as u64);
        for list in &self.lists {
            out.varint(list.len() as u64);
            for string in list {
                out.varint(u64::from(*string));
            }
        }
        out.varint(self.defaults.len() as u64);
        for (template, groups) in &self.defaults {
            out.varint(u64::from(*template));
            write_groups(&mut out, groups, None, self.owners);
        }
        out.varint(self.entities.len() as u64);
        let mut previous = 0u32;
        for (id, groups) in &self.entities {
            out.varint(u64::from(id - previous));
            previous = *id;
            write_groups(&mut out, groups, Some(self.turn), self.owners);
        }
        out.0
    }

    /// Entity `id`'s groups, those it was not sent taken from its
    /// template's defaults; `template` is that of the visual stream's
    /// entity of the id, `None` when there is none.
    pub fn resolved(&self, id: u32, template: Option<u32>) -> Groups {
        let mut groups: Groups = self
            .entities
            .get(&id)
            .cloned()
            .unwrap_or_else(|| std::array::from_fn(|_| None));
        if let Some(defaults) = template.and_then(|template| self.defaults.get(&template)) {
            for (slot, group) in groups.iter_mut().enumerate() {
                if group.is_none() {
                    group.clone_from(&defaults[slot]);
                }
            }
        }
        groups
    }
}

fn next_id(previous: u32, cursor: &mut Cursor) -> Result<u32, &'static str> {
    previous
        .checked_add(cursor.varint_u32()?)
        .ok_or("id out of range")
}

/// The mask, then the groups; a keyframe's entities' reckoned groups are
/// each followed by the turns since they were sent.
fn read_groups(
    cursor: &mut Cursor,
    turn: u32,
    keyframe_entity: bool,
    owners: bool,
) -> Result<Groups, &'static str> {
    let mask = cursor.varint()?;
    if mask >> SLOTS != 0 {
        return Err("unknown detail group");
    }
    let mut groups: Groups = std::array::from_fn(|_| None);
    for (slot, group) in groups.iter_mut().enumerate() {
        if mask & 1 << slot == 0 {
            continue;
        }
        let mut read = read_group(cursor, slot, turn, owners)?;
        if keyframe_entity && is_reckoned(slot) {
            let since = cursor.varint_u32()?;
            let sent = turn
                .checked_sub(since)
                .ok_or("reckoned group sent before the match")?;
            match &mut read {
                Group::Queue(queue) => queue.turn = sent,
                Group::Building(building) => building.turn = sent,
                _ => {}
            }
        }
        *group = Some(read);
    }
    Ok(groups)
}

fn read_group(
    cursor: &mut Cursor,
    slot: usize,
    turn: u32,
    owners: bool,
) -> Result<Group, &'static str> {
    Ok(match slot {
        HEALTH => Group::Health {
            hitpoints: cursor.varint_u32()?,
            max: cursor.varint_u32()?,
        },
        CAPTURE => {
            let points = (0..cursor.varint()?)
                .map(|_| cursor.varint_u32())
                .collect::<Result<_, _>>()?;
            Group::Capture {
                points,
                max: cursor.varint_u32()?,
            }
        }
        SUPPLY => Group::Supply {
            amount: cursor.varint_u32()?,
            max: cursor.varint_u32()?,
        },
        GATHERERS => Group::Gatherers {
            count: cursor.u8()?,
            max: cursor.u8()?,
        },
        CARRYING => Group::Carrying(
            (0..cursor.varint()?)
                .map(|_| {
                    Ok(Carried {
                        kind: cursor.varint_u32()?,
                        amount: cursor.varint_u32()?,
                        capacity: cursor.varint_u32()?,
                    })
                })
                .collect::<Result<_, &'static str>>()?,
        ),
        EXPERIENCE => Group::Experience {
            experience: cursor.varint_u32()?,
            required: cursor.varint_u32()?,
        },
        QUEUE => {
            let autoqueue = cursor.u8()? != 0;
            let items = (0..cursor.varint()?)
                .map(|_| {
                    Ok(QueueItem {
                        id: cursor.varint_u32()?,
                        technology: cursor.u8()? != 0,
                        name: cursor.varint_u32()?,
                        count: cursor.varint_u32()?,
                        paused: cursor.u8()? != 0,
                        missing_population: cursor.varint_u32()?,
                        remaining: cursor.varint_u32()?,
                        total: cursor.varint_u32()?,
                    })
                })
                .collect::<Result<_, &'static str>>()?;
            Group::Queue(Queue {
                autoqueue,
                items,
                turn,
            })
        }
        GARRISON => {
            let capacity = cursor.varint_u32()?;
            let occupied = cursor.varint_u32()?;
            let mut previous = 0i64;
            let entities = (0..cursor.varint()?)
                .map(|_| {
                    previous += cursor.zigzag_i64()?;
                    let id = u32::try_from(previous).map_err(|_| "id out of range")?;
                    let template = cursor.varint_u32()?;
                    let owner = if owners {
                        Some(cursor.zigzag()?).filter(|owner| *owner >= 0)
                    } else {
                        None
                    };
                    Ok(Garrisoned {
                        id,
                        template,
                        owner,
                    })
                })
                .collect::<Result<_, &'static str>>()?;
            Group::Garrison {
                capacity,
                occupied,
                entities,
            }
        }
        FOUNDATION | REPAIR => Group::Building(Building {
            builders: cursor.u8()?,
            remaining: cursor.varint_u32()?,
            turn,
        }),
        UNIT_AI => Group::UnitAi {
            idle: cursor.u8()? != 0,
            stance: cursor.varint_u32()?,
            formation_controller: cursor.varint_u32()?,
        },
        TRAINABLE | RESEARCHABLE => Group::List(cursor.varint_u32()?),
        PROGRESS => Group::Progress {
            pack: cursor.u8()?,
            upgrade: cursor.u8()?,
            upgrade_target: cursor.varint_u32()?,
        },
        HOLDER => Group::Holder(cursor.varint_u32()?),
        TRADER => Group::Trader {
            goods: cursor.varint_u32()?,
            gain: cursor.varint_u32()?,
        },
        _ => return Err("unknown detail group"),
    })
}

/// As `read_groups` reads them; `keyframe_turn` is set for a keyframe's
/// entities.
fn write_groups(out: &mut Writer, groups: &Groups, keyframe_turn: Option<u32>, owners: bool) {
    let mask = groups
        .iter()
        .enumerate()
        .filter(|(_, group)| group.is_some())
        .fold(0u64, |mask, (slot, _)| mask | 1 << slot);
    out.varint(mask);
    for group in groups.iter().flatten() {
        write_group(out, group, owners);
        if let Some(turn) = keyframe_turn {
            match group {
                Group::Queue(queue) => out.varint(u64::from(turn - queue.turn)),
                Group::Building(building) => out.varint(u64::from(turn - building.turn)),
                _ => {}
            }
        }
    }
}

fn write_group(out: &mut Writer, group: &Group, owners: bool) {
    match group {
        Group::Health { hitpoints, max } => {
            out.varint(u64::from(*hitpoints));
            out.varint(u64::from(*max));
        }
        Group::Capture { points, max } => {
            out.varint(points.len() as u64);
            for points in points {
                out.varint(u64::from(*points));
            }
            out.varint(u64::from(*max));
        }
        Group::Supply { amount, max } => {
            out.varint(u64::from(*amount));
            out.varint(u64::from(*max));
        }
        Group::Gatherers { count, max } => {
            out.u8(*count);
            out.u8(*max);
        }
        Group::Carrying(carrying) => {
            out.varint(carrying.len() as u64);
            for carried in carrying {
                out.varint(u64::from(carried.kind));
                out.varint(u64::from(carried.amount));
                out.varint(u64::from(carried.capacity));
            }
        }
        Group::Experience {
            experience,
            required,
        } => {
            out.varint(u64::from(*experience));
            out.varint(u64::from(*required));
        }
        Group::Queue(queue) => {
            out.u8(u8::from(queue.autoqueue));
            out.varint(queue.items.len() as u64);
            for item in &queue.items {
                out.varint(u64::from(item.id));
                out.u8(u8::from(item.technology));
                out.varint(u64::from(item.name));
                out.varint(u64::from(item.count));
                out.u8(u8::from(item.paused));
                out.varint(u64::from(item.missing_population));
                out.varint(u64::from(item.remaining));
                out.varint(u64::from(item.total));
            }
        }
        Group::Garrison {
            capacity,
            occupied,
            entities,
        } => {
            out.varint(u64::from(*capacity));
            out.varint(u64::from(*occupied));
            out.varint(entities.len() as u64);
            let mut previous = 0i64;
            for entity in entities {
                out.zigzag_i64(i64::from(entity.id) - previous);
                previous = i64::from(entity.id);
                out.varint(u64::from(entity.template));
                if owners {
                    out.zigzag(entity.owner.unwrap_or(-1));
                }
            }
        }
        Group::Building(building) => {
            out.u8(building.builders);
            out.varint(u64::from(building.remaining));
        }
        Group::UnitAi {
            idle,
            stance,
            formation_controller,
        } => {
            out.u8(u8::from(*idle));
            out.varint(u64::from(*stance));
            out.varint(u64::from(*formation_controller));
        }
        Group::List(index) | Group::Holder(index) => out.varint(u64::from(*index)),
        Group::Progress {
            pack,
            upgrade,
            upgrade_target,
        } => {
            out.u8(*pack);
            out.u8(*upgrade);
            out.varint(u64::from(*upgrade_target));
        }
        Group::Trader { goods, gain } => {
            out.varint(u64::from(*goods));
            out.varint(u64::from(*gain));
        }
    }
}
