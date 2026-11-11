//! The detail stream (stream 2): what the selection panels show beyond the
//! status bars. The format is in the server's `detail.rs`; its keyframes
//! and deltas are messages of kind 1 and 2 as the visual stream's, carried
//! in segments and envelopes the same way, so `stream.rs` splits them.

use std::collections::BTreeMap;

use specula_assets::bytes::ByteReader;
use specula_assets::wire::DETAIL_VERSION;

use crate::TURN_MS;
use crate::stream::Writer;

const MESSAGE_KEYFRAME: u8 = 1;
const MESSAGE_DELTA: u8 = 2;
/// Archived matches keep it; it has no market or trade garrison group.
const DETAIL_VERSION_NO_TRADE_GAIN: u8 = 11;
/// Archived matches keep it; it also has no trade route group.
const DETAIL_VERSION_NO_TRADE_ROUTE: u8 = 10;
/// Archived matches keep it; its player table also has no trade state.
const DETAIL_VERSION_NO_TRADE: u8 = 9;
/// Archived matches keep it; it also cannot clear a group.
const DETAIL_VERSION_NO_CLEARED: u8 = 8;
/// Archived matches keep it; it has no stances, formations, commands,
/// upgrades or player table.
const DETAIL_VERSION_NO_COMMANDS: u8 = 7;
/// Archived matches keep it; it also has no buildable lists.
const DETAIL_VERSION_NO_BUILDABLE: u8 = 6;
/// Archived matches keep it; it also has no stats.
const DETAIL_VERSION_NO_STATS: u8 = 5;
/// Archived matches keep it; it also has no data table.
const DETAIL_VERSION_NO_DATA: u8 = 4;
/// Archived matches keep it; it also has no rally points.
const DETAIL_VERSION_NO_RALLY: u8 = 3;
/// Archived matches keep it; its lists are also of names only.
const DETAIL_VERSION_NAME_LISTS: u8 = 2;
/// Archived matches keep it; it also has no garrison owners.
const DETAIL_VERSION_NO_OWNERS: u8 = 1;
/// Slots of `Groups`, the hook's group bits.
pub const HEALTH: usize = 0;
pub const CAPTURE: usize = 1;
pub const SUPPLY: usize = 2;
pub const GATHERERS: usize = 3;
pub const CARRYING: usize = 4;
pub const EXPERIENCE: usize = 5;
pub const QUEUE: usize = 6;
pub const GARRISON: usize = 7;
pub const FOUNDATION: usize = 8;
pub const REPAIR: usize = 9;
pub const UNIT_AI: usize = 10;
pub const TRAINABLE: usize = 11;
pub const RESEARCHABLE: usize = 12;
pub const PROGRESS: usize = 13;
pub const HOLDER: usize = 14;
pub const TRADER: usize = 15;
pub const RALLY: usize = 16;
pub const STATS: usize = 17;
pub const BUILDABLE: usize = 18;
pub const STANCES: usize = 19;
pub const FORMATIONS: usize = 20;
pub const COMMANDS: usize = 21;
pub const UPGRADES: usize = 22;
pub const TRADE_ROUTE: usize = 23;
pub const MARKET: usize = 24;
pub const TRADE_GARRISON: usize = 25;
const SLOTS: usize = 26;
/// Rally positions come in 1/16 m.
pub const RALLY_UNITS_PER_METRE: f32 = 16.0;
/// `Group::Progress`'s pack or upgrade when not running.
pub const PROGRESS_NONE: u8 = 255;

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
    /// A trader's first and second markets, 0 for none, and its
    /// `GetTraderGainMultiplier`.
    TradeRoute {
        first: u32,
        second: u32,
        multiplier: f32,
    },
    /// A market's `GetInternationalBonus`.
    MarketBonus(f32),
    /// A trader's `GetGarrisonGainMultiplier`.
    TradeGarrison(f32),
    Rally {
        /// The flag's actor.
        marker: u32,
        points: Vec<RallyPoint>,
    },
    /// The hash of the entity's stats JSON, at `/data/<hash>`.
    Stats(u64),
    /// `UnitAI.GetSelectableStances`, 1-based strings.
    Stances(Vec<u32>),
    Formations {
        /// The formation template it is in, 1-based; the null formation's
        /// when in none.
        current: u32,
        /// The templates its UnitAI can use, 1-based.
        list: Vec<u32>,
    },
    Commands(Commands),
}

/// What the command row and the pack, gate and alert panels read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Commands {
    pub flags: u32,
    /// AlertRaiser's target classes, 1-based, 0 without one.
    pub alert_classes: u32,
    /// The ejectable occupants of a TurretHolder's points.
    pub turrets: u32,
}

impl Commands {
    pub const WORK_ORDERS: u32 = 1;
    pub const CAN_GUARD: u32 = 2;
    pub const GUARDING: u32 = 4;
    pub const CAN_PATROL: u32 = 8;
    pub const GATE_LOCKED: u32 = 16;
    pub const PACKED: u32 = 32;
    pub const SHARABLE: u32 = 64;
    pub const SHARED: u32 = 128;
    pub const EJECTABLE: u32 = 256;
}

/// What a player's panels read that is not an entity's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerGui {
    /// 1-based formation template strings the civ may use.
    pub formations: Vec<u32>,
    /// 1-based resource code, buy price, sell price.
    pub prices: Vec<(u32, f32, f32)>,
    /// Whether the player has a market, `canBarter`.
    pub can_barter: bool,
    /// 1-based resource code and percent, `GetTradingGoods`; empty for
    /// another player than a seat's own.
    pub trading_goods: Vec<(u32, u8)>,
    /// `GetTraderNumber`: land total, trading, garrisoned; ship total,
    /// trading.
    pub traders: [u16; 5],
}

/// A rally point and the line to it, in `RALLY_UNITS_PER_METRE`.
#[derive(Clone, Debug, PartialEq)]
pub struct RallyPoint {
    pub x: i32,
    pub z: i32,
    /// From the point back to the previous one or the footprint's edge.
    pub path: Vec<[i32; 2]>,
}

/// What a JSON of the data table is about; names and civs are 1-based
/// strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DataKey {
    /// `GetTemplateData` for a player.
    Template { name: u32, player: i32 },
    /// The session GUI's `GetTechnologyData` for a civ.
    Technology { name: u32, civ: u32 },
    /// A formation template's info JSON.
    Formation { name: u32 },
}

/// One item of a trainable or researchable list, read for the producer's
/// owner.
#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    /// 1-based string, 0 for an empty slot of the research row.
    pub name: u32,
    pub flags: u8,
    /// `canBeAddedCount`; `None` for no limit.
    pub can_be_added: Option<u32>,
    /// 1-based resource type string, cost in 1/100.
    pub costs: Vec<(u32, u32)>,
}

impl ListItem {
    pub const REQUIREMENTS_MET: u8 = 1;
    /// The panels hide it: no template data, or a technology the owner's
    /// civ cannot research.
    pub const HIDDEN: u8 = 2;
    pub const PAIR_TOP: u8 = 4;
    /// From an older hook or an archived match: only the name is known,
    /// and a pair is one "top bottom" string.
    pub const UNKNOWN: u8 = 8;
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
    /// runs. `turn` is fractional so the panel can tick between turns, and
    /// may be before the turn it was sent on, as a frame shows the turn
    /// before the last until it ends.
    pub fn remaining_ms(&self, index: usize, turn: f64) -> f64 {
        let Some(item) = self.items.get(index) else {
            return 0.0;
        };
        let sent = f64::from(item.remaining) * 100.0;
        if index > 0 || item.paused {
            return sent;
        }
        (sent - TURN_MS * (turn - f64::from(self.turn))).max(0.0)
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
    /// The keyframe's version, which its deltas are in too.
    pub version: u8,
    pub lists: Vec<Vec<ListItem>>,
    /// The hash of each JSON the tooltips read, at `/data/<hash>`.
    pub data: BTreeMap<DataKey, u64>,
    /// By player index; empty for a stream before detail version 8.
    pub players: Vec<PlayerGui>,
    /// By template index: the groups an entity of it has when its own
    /// leave them out.
    pub defaults: BTreeMap<u32, Groups>,
    /// By id, the groups sent for it.
    pub entities: BTreeMap<u32, Groups>,
    /// By id, the slots its template's defaults do not fill: a seat's
    /// player sees less of an enemy's entity than of its own of the same
    /// template. Every id here is in `entities`.
    pub cleared: BTreeMap<u32, u64>,
}

impl Details {
    pub fn apply(&mut self, message: &[u8]) -> Result<(), &'static str> {
        let (&kind, body) = message.split_first().ok_or("empty message")?;
        let mut cursor = ByteReader(body);
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

    fn apply_keyframe(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        let turn = cursor.varint_u32()?;
        let version = cursor.u8()?;
        if !matches!(
            version,
            DETAIL_VERSION
                | DETAIL_VERSION_NO_TRADE_GAIN
                | DETAIL_VERSION_NO_TRADE_ROUTE
                | DETAIL_VERSION_NO_TRADE
                | DETAIL_VERSION_NO_CLEARED
                | DETAIL_VERSION_NO_COMMANDS
                | DETAIL_VERSION_NO_BUILDABLE
                | DETAIL_VERSION_NO_STATS
                | DETAIL_VERSION_NO_DATA
                | DETAIL_VERSION_NO_RALLY
                | DETAIL_VERSION_NAME_LISTS
                | DETAIL_VERSION_NO_OWNERS
        ) {
            return Err("unknown detail version, reload the page");
        }
        let owners = version != DETAIL_VERSION_NO_OWNERS;
        *self = Details {
            known: true,
            turn,
            match_id: cursor.u64()?,
            version,
            ..Details::default()
        };
        self.read_lists(cursor)?;
        self.read_data(cursor)?;
        self.read_players(cursor)?;
        for _ in 0..cursor.varint()? {
            let template = cursor.varint_u32()?;
            let groups = read_groups(cursor, turn, false, owners)?;
            self.defaults.insert(template, groups);
        }
        let mut id = 0u32;
        for _ in 0..cursor.varint()? {
            id = next_id(id, cursor)?;
            let cleared = self.read_cleared(cursor)?;
            let groups = read_groups(cursor, turn, true, owners)?;
            self.entities.insert(id, groups);
            if cleared != 0 {
                self.cleared.insert(id, cleared);
            }
        }
        Ok(())
    }

    fn apply_delta(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        if !self.known {
            return Err("detail delta before a keyframe");
        }
        self.turn = cursor.varint_u32()?;
        self.read_lists(cursor)?;
        self.read_data(cursor)?;
        if self.version > DETAIL_VERSION_NO_COMMANDS && cursor.u8()? != 0 {
            self.read_players(cursor)?;
        }
        let owners = self.owners();
        let mut id = 0u32;
        for _ in 0..cursor.varint()? {
            id = next_id(id, cursor)?;
            let cleared = self.read_cleared(cursor)?;
            let changed = read_groups(cursor, self.turn, false, owners)?;
            let groups = self
                .entities
                .entry(id)
                .or_insert_with(|| std::array::from_fn(|_| None));
            let mut kept = self.cleared.get(&id).copied().unwrap_or(0) | cleared;
            for (slot, group) in changed.into_iter().enumerate() {
                if group.is_some() {
                    groups[slot] = group;
                    kept &= !(1 << slot);
                } else if cleared & 1 << slot != 0 {
                    groups[slot] = None;
                }
            }
            if kept == 0 {
                self.cleared.remove(&id);
            } else {
                self.cleared.insert(id, kept);
            }
        }
        let mut id = 0u32;
        for _ in 0..cursor.varint()? {
            id = next_id(id, cursor)?;
            self.entities.remove(&id);
            self.cleared.remove(&id);
        }
        Ok(())
    }

    /// An entity's cleared slots, which versions before 9 do not have.
    fn read_cleared(&self, cursor: &mut ByteReader) -> Result<u64, &'static str> {
        if self.version <= DETAIL_VERSION_NO_CLEARED {
            return Ok(0);
        }
        let cleared = cursor.varint()?;
        if cleared >> SLOTS != 0 {
            return Err("unknown detail group");
        }
        Ok(cleared)
    }

    /// Whether its garrisons carry owners, which a delta does not say.
    fn owners(&self) -> bool {
        self.version != DETAIL_VERSION_NO_OWNERS
    }

    fn read_data(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        if self.version < DETAIL_VERSION_NO_STATS {
            return Ok(());
        }
        for _ in 0..cursor.varint()? {
            let kind = cursor.u8()?;
            let name = cursor.varint_u32()?;
            let key = match kind {
                0 => DataKey::Template {
                    name,
                    player: cursor.zigzag()?,
                },
                1 => DataKey::Technology {
                    name,
                    civ: cursor.varint_u32()?,
                },
                2 => DataKey::Formation { name },
                _ => return Err("unknown data kind"),
            };
            self.data.insert(key, cursor.u64()?);
        }
        Ok(())
    }

    /// The player table, which versions before 8 do not have.
    fn read_players(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        if self.version <= DETAIL_VERSION_NO_COMMANDS {
            return Ok(());
        }
        self.players = (0..cursor.varint()?)
            .map(|_| {
                let formations = (0..cursor.varint()?)
                    .map(|_| cursor.varint_u32())
                    .collect::<Result<_, _>>()?;
                let prices = (0..cursor.varint()?)
                    .map(|_| Ok((cursor.varint_u32()?, cursor.f32()?, cursor.f32()?)))
                    .collect::<Result<_, &'static str>>()?;
                let mut player = PlayerGui {
                    formations,
                    prices,
                    ..PlayerGui::default()
                };
                if self.version > DETAIL_VERSION_NO_TRADE {
                    player.can_barter = cursor.u8()? != 0;
                    player.trading_goods = (0..cursor.varint()?)
                        .map(|_| Ok((cursor.varint_u32()?, cursor.u8()?)))
                        .collect::<Result<_, &'static str>>()?;
                    for count in &mut player.traders {
                        *count = u16::try_from(cursor.varint()?).unwrap_or(u16::MAX);
                    }
                }
                Ok(player)
            })
            .collect::<Result<_, &'static str>>()?;
        Ok(())
    }

    /// Whether its list items carry more than their names.
    fn full_lists(&self) -> bool {
        self.version >= DETAIL_VERSION_NO_RALLY
    }

    fn read_lists(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        for _ in 0..cursor.varint()? {
            let list = (0..cursor.varint()?)
                .map(|_| {
                    let name = cursor.varint_u32()?;
                    if !self.full_lists() {
                        return Ok(ListItem {
                            name,
                            flags: ListItem::UNKNOWN,
                            can_be_added: None,
                            costs: Vec::new(),
                        });
                    }
                    let flags = cursor.u8()?;
                    let can_be_added = cursor.varint_u32()?.checked_sub(1);
                    let costs = (0..cursor.varint()?)
                        .map(|_| Ok((cursor.varint_u32()?, cursor.varint_u32()?)))
                        .collect::<Result<_, &'static str>>()?;
                    Ok(ListItem {
                        name,
                        flags,
                        can_be_added,
                        costs,
                    })
                })
                .collect::<Result<_, &'static str>>()?;
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
        out.u8(self.version);
        out.u64(self.match_id);
        out.varint(self.lists.len() as u64);
        for list in &self.lists {
            out.varint(list.len() as u64);
            for item in list {
                out.varint(u64::from(item.name));
                if !self.full_lists() {
                    continue;
                }
                out.u8(item.flags);
                out.varint(item.can_be_added.map_or(0, |limit| u64::from(limit) + 1));
                out.varint(item.costs.len() as u64);
                for (resource, cost) in &item.costs {
                    out.varint(u64::from(*resource));
                    out.varint(u64::from(*cost));
                }
            }
        }
        if self.version >= DETAIL_VERSION_NO_STATS {
            out.varint(self.data.len() as u64);
            for (key, hash) in &self.data {
                match key {
                    DataKey::Template { name, player } => {
                        out.u8(0);
                        out.varint(u64::from(*name));
                        out.zigzag(*player);
                    }
                    DataKey::Technology { name, civ } => {
                        out.u8(1);
                        out.varint(u64::from(*name));
                        out.varint(u64::from(*civ));
                    }
                    DataKey::Formation { name } => {
                        out.u8(2);
                        out.varint(u64::from(*name));
                    }
                }
                out.u64(*hash);
            }
        }
        if self.version > DETAIL_VERSION_NO_COMMANDS {
            out.varint(self.players.len() as u64);
            for player in &self.players {
                out.varint(player.formations.len() as u64);
                for formation in &player.formations {
                    out.varint(u64::from(*formation));
                }
                out.varint(player.prices.len() as u64);
                for (resource, buy, sell) in &player.prices {
                    out.varint(u64::from(*resource));
                    out.f32(*buy);
                    out.f32(*sell);
                }
                if self.version > DETAIL_VERSION_NO_TRADE {
                    out.u8(u8::from(player.can_barter));
                    out.varint(player.trading_goods.len() as u64);
                    for (resource, percent) in &player.trading_goods {
                        out.varint(u64::from(*resource));
                        out.u8(*percent);
                    }
                    for count in player.traders {
                        out.varint(u64::from(count));
                    }
                }
            }
        }
        let owners = self.owners();
        out.varint(self.defaults.len() as u64);
        for (template, groups) in &self.defaults {
            out.varint(u64::from(*template));
            write_groups(&mut out, groups, None, owners);
        }
        out.varint(self.entities.len() as u64);
        let mut previous = 0u32;
        for (id, groups) in &self.entities {
            out.varint(u64::from(id - previous));
            previous = *id;
            if self.version > DETAIL_VERSION_NO_CLEARED {
                out.varint(self.cleared.get(id).copied().unwrap_or(0));
            }
            write_groups(&mut out, groups, Some(self.turn), owners);
        }
        out.0
    }

    /// Entity `id`'s group in `slot`, as `resolved` finds it, without
    /// copying the rest.
    pub fn group(&self, id: u32, template: Option<u32>, slot: usize) -> Option<&Group> {
        self.entities
            .get(&id)
            .and_then(|groups| groups[slot].as_ref())
            .or_else(|| {
                if self
                    .cleared
                    .get(&id)
                    .is_some_and(|cleared| cleared & 1 << slot != 0)
                {
                    return None;
                }
                self.defaults.get(&template?)?[slot].as_ref()
            })
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
            let cleared = self.cleared.get(&id).copied().unwrap_or(0);
            for (slot, group) in groups.iter_mut().enumerate() {
                if group.is_none() && cleared & 1 << slot == 0 {
                    group.clone_from(&defaults[slot]);
                }
            }
        }
        groups
    }
}

fn next_id(previous: u32, cursor: &mut ByteReader) -> Result<u32, &'static str> {
    previous
        .checked_add(cursor.varint_u32()?)
        .ok_or("id out of range")
}

/// The mask, then the groups; a keyframe's entities' reckoned groups are
/// each followed by the turns since they were sent.
fn read_groups(
    cursor: &mut ByteReader,
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
    cursor: &mut ByteReader,
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
        TRAINABLE | RESEARCHABLE | BUILDABLE | UPGRADES => Group::List(cursor.varint_u32()?),
        STANCES => Group::Stances(
            (0..cursor.varint()?)
                .map(|_| cursor.varint_u32())
                .collect::<Result<_, _>>()?,
        ),
        FORMATIONS => {
            let current = cursor.varint_u32()?;
            let list = (0..cursor.varint()?)
                .map(|_| cursor.varint_u32())
                .collect::<Result<_, _>>()?;
            Group::Formations { current, list }
        }
        COMMANDS => Group::Commands(Commands {
            flags: cursor.varint_u32()?,
            alert_classes: cursor.varint_u32()?,
            turrets: cursor.varint_u32()?,
        }),
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
        TRADE_ROUTE => Group::TradeRoute {
            first: cursor.varint_u32()?,
            second: cursor.varint_u32()?,
            multiplier: cursor.f32()?,
        },
        MARKET => Group::MarketBonus(cursor.f32()?),
        TRADE_GARRISON => Group::TradeGarrison(cursor.f32()?),
        STATS => Group::Stats(cursor.u64()?),
        RALLY => {
            let marker = cursor.varint_u32()?;
            let points = (0..cursor.varint()?)
                .map(|_| {
                    let x = cursor.zigzag()?;
                    let z = cursor.zigzag()?;
                    let mut previous = [x, z];
                    let path = (0..cursor.varint()?)
                        .map(|_| {
                            previous = [
                                previous[0].wrapping_add(cursor.zigzag()?),
                                previous[1].wrapping_add(cursor.zigzag()?),
                            ];
                            Ok(previous)
                        })
                        .collect::<Result<_, &'static str>>()?;
                    Ok(RallyPoint { x, z, path })
                })
                .collect::<Result<_, &'static str>>()?;
            Group::Rally { marker, points }
        }
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
        Group::TradeRoute {
            first,
            second,
            multiplier,
        } => {
            out.varint(u64::from(*first));
            out.varint(u64::from(*second));
            out.f32(*multiplier);
        }
        Group::MarketBonus(value) | Group::TradeGarrison(value) => out.f32(*value),
        Group::Stats(hash) => out.u64(*hash),
        Group::Stances(stances) => {
            out.varint(stances.len() as u64);
            for stance in stances {
                out.varint(u64::from(*stance));
            }
        }
        Group::Formations { current, list } => {
            out.varint(u64::from(*current));
            out.varint(list.len() as u64);
            for formation in list {
                out.varint(u64::from(*formation));
            }
        }
        Group::Commands(commands) => {
            out.varint(u64::from(commands.flags));
            out.varint(u64::from(commands.alert_classes));
            out.varint(u64::from(commands.turrets));
        }
        Group::Rally { marker, points } => {
            out.varint(u64::from(*marker));
            out.varint(points.len() as u64);
            for point in points {
                out.zigzag(point.x);
                out.zigzag(point.z);
                out.varint(point.path.len() as u64);
                let mut previous = [point.x, point.z];
                for at in &point.path {
                    out.zigzag(at[0].wrapping_sub(previous[0]));
                    out.zigzag(at[1].wrapping_sub(previous[1]));
                    previous = *at;
                }
            }
        }
    }
}
