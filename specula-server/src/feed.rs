//! Decoder for the observer hook's feed, format v15 (and v14, which had
//! one visibility bit per player, no mirages, no LOS and no shared LOS
//! flag). The writer is `source/network/SpeculaFeed.cpp` in the `specula`
//! branch of 0ad-specula.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::io::ErrorKind;
use std::io::Read;
use std::sync::Arc;

const MAGIC: [u8; 4] = *b"SPCF";
const VERSION: u32 = 15;
const OLDEST_VERSION: u32 = 14;
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
const RECORD_TEMPLATE_DETAILS: u8 = 11;
const RECORD_DETAILS: u8 = 12;
const RECORD_STRING_LIST: u8 = 13;
const RECORD_RENAMES: u8 = 14;
const RECORD_ECONOMY: u8 = 15;
const RECORD_VARIABLE_SET: u8 = 16;
const RECORD_PRODUCTION_LIST: u8 = 17;
const RECORD_TEMPLATE_DATA: u8 = 18;
const RECORD_TECHNOLOGY_DATA: u8 = 19;
const RECORD_STATS: u8 = 20;
const RECORD_PLAYER_GUI: u8 = 21;
const RECORD_FORMATION_DATA: u8 = 22;
const RECORD_QUERY_REPLY: u8 = 23;
const RECORD_LOS: u8 = 24;
const RECORD_SUMMARY: u8 = 25;
const RECORD_PAUSE: u8 = 26;
const RECORD_KEEPALIVE: u8 = 27;
const ENTITY_SIZE: usize = 84;
const ENTITY_SIZE_V14: usize = 74;
const SOUND_SIZE: usize = 29;
const SOUND_SIZE_V14: usize = 27;
const LAUNCH_SIZE: usize = 52;
const COLOR_MINIMAP: u8 = 1;
const HAS_VISUAL: u8 = 2;
const ANIMATION_ONCE: u8 = 4;
const NONE: u32 = 0xFFFF_FFFF;
const TEMPLATE_PLAYER_COLOR: u8 = 1;
const SOUND_ENTITY: u8 = 0;
const SOUND_POSITION: u8 = 1;
const SOUND_PLAYER: u8 = 2;
const DETAIL_HEALTH: u32 = 1;
const DETAIL_CAPTURE: u32 = 2;
const DETAIL_SUPPLY: u32 = 4;
const DETAIL_GATHERERS: u32 = 8;
const DETAIL_CARRYING: u32 = 16;
const DETAIL_EXPERIENCE: u32 = 32;
const DETAIL_QUEUE: u32 = 64;
const DETAIL_GARRISON: u32 = 128;
const DETAIL_FOUNDATION: u32 = 256;
const DETAIL_REPAIR: u32 = 512;
const DETAIL_UNIT_AI: u32 = 1024;
const DETAIL_TRAINABLE: u32 = 2048;
const DETAIL_RESEARCHABLE: u32 = 4096;
const DETAIL_PROGRESS: u32 = 8192;
const DETAIL_HOLDER: u32 = 16384;
const DETAIL_TRADER: u32 = 32768;
const DETAIL_RANGES: u32 = 65536;
const DETAIL_GARRISON_OWNERS: u32 = 131072;
const DETAIL_TRAINABLE_ITEMS: u32 = 262144;
const DETAIL_RESEARCHABLE_ITEMS: u32 = 524288;
const DETAIL_RALLY: u32 = 1048576;
const DETAIL_STATS: u32 = 2097152;
const DETAIL_BUILDABLE_ITEMS: u32 = 4194304;
const DETAIL_STANCES: u32 = 8388608;
const DETAIL_FORMATIONS: u32 = 16777216;
const DETAIL_COMMANDS: u32 = 33554432;
const DETAIL_UPGRADES: u32 = 67108864;
const DETAIL_TRADE_ROUTE: u32 = 134217728;
const DETAIL_MARKET: u32 = 268435456;
const DETAIL_TRADE_GARRISON: u32 = 536870912;
const NO_LIMIT: u16 = 0xFFFF;
const QUEUE_UNIT: u8 = 0;
const QUEUE_TECHNOLOGY: u8 = 1;

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
    /// Bits 2p and 2p+1 are `GetLosVisibility` for player p below 16 (see
    /// `HIDDEN`, 1 fogged, `VISIBLE`).
    pub visibility: u32,
    /// Set for a mirage (`Fogging.js`), which only its player sees.
    pub mirage: Option<Mirage>,
}

/// `LosVisibility`; 1 is FOGGED.
pub const HIDDEN: u32 = 0;
pub const VISIBLE: u32 = 2;

/// Player p's `LosVisibility` in a visibility mask.
pub fn visibility_of(mask: u32, player: usize) -> u32 {
    if player < 16 {
        (mask >> (2 * player)) & 3
    } else {
        HIDDEN
    }
}

/// What one player last saw of an entity in the fog.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Mirage {
    pub player: i32,
    /// `None` once the entity it stands for is gone.
    pub parent: Option<u32>,
}

/// A v14 mask had a bit per player for VISIBLE.
fn widen_visibility(bits: u16) -> u32 {
    (0..16)
        .filter(|player| bits & (1 << player) != 0)
        .fold(0, |mask, player| mask | VISIBLE << (2 * player))
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
    /// The entity's visibility mask as of the call (VISIBLE for all
    /// unless it is an entity's); player p hears it when it is VISIBLE.
    pub visibility: u32,
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
#[derive(Clone)]
pub struct Icon {
    /// Relative to art/textures/ui/session/icons/minimap/.
    pub name: String,
    /// In GUI pixels.
    pub size: f32,
}

/// The players record: the players and the ceasefire, which
/// `GetSimulationState` reports beside them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Players {
    pub players: Vec<Player>,
    /// When the ceasefire ends, in simulation milliseconds; 0 for none or
    /// a hook that does not send it.
    pub ceasefire_end: u32,
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
    /// `Diplomacy.HasSharedLos`; false from a v14 hook.
    pub shared_los: bool,
}

/// What a template gives the selection overlay, the status bars and the
/// panels, read from the template as is. Strings are string record indices.
#[derive(Clone)]
pub struct TemplateDetails {
    /// The template flag bits of `SpeculaFeed.h`, kind 11.
    pub flags: u16,
    /// Relative to art/textures/selection/.
    pub overlay_texture: Option<u32>,
    pub overlay_mask: Option<u32>,
    pub line_thickness: f32,
    /// A circle's radius in both.
    pub overlay_width: f32,
    pub overlay_depth: f32,
    pub footprint_width: f32,
    pub footprint_depth: f32,
    pub footprint_height: f32,
    pub bar_width: f32,
    pub bar_height: f32,
    pub bar_offset: f32,
    pub generic_name: Option<u32>,
    pub specific_name: Option<u32>,
    /// Relative to art/textures/ui/session/portraits/.
    pub icon: Option<u32>,
    pub group_name: Option<u32>,
    pub rank: Option<u32>,
    /// Token lists, as the template writes them.
    pub classes: Option<u32>,
    pub visible_classes: Option<u32>,
    /// Relative to audio/.
    pub select_sound: Option<u32>,
    /// Identity's Lang and Phenotype tokens, for the select group's
    /// `{lang}` and `{phenotype}`.
    pub lang: Option<u32>,
    pub phenotypes: Option<u32>,
    /// Bit i for the i-th of Health, Capturable, ResourceSupply,
    /// ProductionQueue, GarrisonHolder, Promotion, ResourceGatherer,
    /// Foundation, Repairable, UnitAI, Trainer, Researcher, Pack, Upgrade,
    /// Trader, Garrisonable, Turretable, Attack, Auras, Heal, RallyPoint,
    /// Builder, Gate, AlertRaiser, ResourceDropsite, TurretHolder, Market.
    pub components: u32,
    /// RallyPointRenderer's line, full VFS paths.
    pub rally_texture: Option<u32>,
    pub rally_mask: Option<u32>,
    pub rally_thickness: f32,
    /// 0 flat, 1 round, 2 sharp, 3 square.
    pub rally_start_cap: u8,
    pub rally_end_cap: u8,
    /// What a right click's action is picked by: ResourceSupply's type
    /// (e.g. `wood.tree`), ResourceGatherer's rate names and
    /// ResourceDropsite's types as token lists, and the Attack types, each
    /// `Type` or `Type:Class,Class` with its RestrictedClasses.
    pub supply_type: Option<u32>,
    pub gather_rates: Option<u32>,
    pub dropsite_types: Option<u32>,
    pub attack_types: Option<u32>,
    /// The groups gui/session plays on an order, as `name=path` tokens.
    pub order_sounds: Option<u32>,
    /// GarrisonHolder's List, and TurretHolder's points' AllowedClasses
    /// with `;` between points, what the garrison and occupy-turret
    /// actions match.
    pub garrison_classes: Option<u32>,
    pub turret_classes: Option<u32>,
}

/// One item of a trainable or researchable list, read for the producer's
/// owner as the session GUI reads it.
#[derive(Clone, PartialEq)]
pub struct ProductionItem {
    /// A template or technology; none for an empty slot of the research
    /// row.
    pub name: Option<u32>,
    pub flags: u8,
    /// `canBeAddedCount`; `None` for no limit.
    pub can_be_added: Option<u16>,
    /// Resource code string, amount.
    pub costs: Vec<(Option<u32>, f32)>,
}

impl ProductionItem {
    /// From an older hook, which sent only the names, and a pair as one
    /// "top bottom" string.
    pub const UNKNOWN: u8 = 8;
    /// The hook's: 1 requirements met, 2 hidden, 4 the top
    /// of a pair, as the detail stream sends them.
    const HOOK_FLAGS: u8 = 7;
}

/// An entity's kind 12 groups; `None` is a group it does not have, or, in
/// a record, one that did not change.
#[derive(Clone, Default)]
pub struct EntityDetails {
    /// Hitpoints, max.
    pub health: Option<(f32, f32)>,
    pub capture: Option<Capture>,
    /// Amount, max.
    pub supply: Option<(f32, f32)>,
    /// Gatherers, max.
    pub gatherers: Option<(u8, u8)>,
    pub carrying: Option<Vec<Carried>>,
    /// Experience, required.
    pub experience: Option<(f32, f32)>,
    pub queue: Option<Queue>,
    pub garrison: Option<Garrison>,
    pub foundation: Option<Building>,
    pub repair: Option<Building>,
    pub unit_ai: Option<UnitAi>,
    /// Production list indices.
    pub trainable: Option<u32>,
    pub researchable: Option<u32>,
    pub buildable: Option<u32>,
    /// A production list of the upgrades, as for a trainable list.
    pub upgrades: Option<u32>,
    /// String record indices of `UnitAI.GetSelectableStances`.
    pub stances: Option<Vec<Option<u32>>>,
    pub formations: Option<Formations>,
    pub commands: Option<Commands>,
    pub progress: Option<Progress>,
    /// The entity it is garrisoned in or a turret on, 0 for none.
    pub holder: Option<u32>,
    pub trader: Option<Trader>,
    pub trade_route: Option<TradeRoute>,
    /// A market's `GetInternationalBonus`.
    pub market_bonus: Option<f32>,
    /// A trader's `GetGarrisonGainMultiplier`, for those that have one.
    pub trade_garrison: Option<f32>,
    pub ranges: Option<Vec<RangeOverlay>>,
    /// The owner of each entity of `garrison`, in its order, -1 for none.
    /// Older hooks do not send it.
    pub garrison_owners: Option<Vec<i8>>,
    pub rally: Option<Rally>,
    /// The kind 20 stats index as decoded, then, once applied, the hash
    /// `Details::set_stats` gave that index.
    pub stats: Option<u64>,
}

impl EntityDetails {
    /// Takes the groups `changed` carries.
    fn merge(&mut self, changed: Self) {
        fn take<T>(slot: &mut Option<T>, value: Option<T>) {
            if value.is_some() {
                *slot = value;
            }
        }
        take(&mut self.health, changed.health);
        take(&mut self.capture, changed.capture);
        take(&mut self.supply, changed.supply);
        take(&mut self.gatherers, changed.gatherers);
        take(&mut self.carrying, changed.carrying);
        take(&mut self.experience, changed.experience);
        take(&mut self.queue, changed.queue);
        take(&mut self.garrison, changed.garrison);
        take(&mut self.foundation, changed.foundation);
        take(&mut self.repair, changed.repair);
        take(&mut self.unit_ai, changed.unit_ai);
        take(&mut self.trainable, changed.trainable);
        take(&mut self.researchable, changed.researchable);
        take(&mut self.buildable, changed.buildable);
        take(&mut self.upgrades, changed.upgrades);
        take(&mut self.stances, changed.stances);
        take(&mut self.formations, changed.formations);
        take(&mut self.commands, changed.commands);
        take(&mut self.progress, changed.progress);
        take(&mut self.holder, changed.holder);
        take(&mut self.trader, changed.trader);
        take(&mut self.trade_route, changed.trade_route);
        take(&mut self.market_bonus, changed.market_bonus);
        take(&mut self.trade_garrison, changed.trade_garrison);
        take(&mut self.ranges, changed.ranges);
        take(&mut self.garrison_owners, changed.garrison_owners);
        take(&mut self.rally, changed.rally);
        take(&mut self.stats, changed.stats);
    }
}

#[derive(Clone)]
pub struct Capture {
    /// By player index.
    pub points: Vec<f32>,
    pub max: f32,
}

#[derive(Clone)]
pub struct Carried {
    /// String record index.
    pub kind: Option<u32>,
    pub amount: f32,
    pub capacity: f32,
}

#[derive(Clone)]
pub struct Queue {
    pub autoqueue: bool,
    pub items: Vec<QueueItem>,
}

#[derive(Clone)]
pub struct QueueItem {
    pub id: u32,
    pub technology: bool,
    /// String record index: the unit template or the technology.
    pub name: Option<u32>,
    pub count: u16,
    /// 0..=1.
    pub progress: f32,
    /// In milliseconds.
    pub time_remaining: f32,
    pub paused: bool,
    pub missing_population: u16,
}

#[derive(Clone)]
pub struct Garrison {
    pub capacity: u16,
    pub occupied: u16,
    /// (id, template index).
    pub entities: Vec<(u32, u32)>,
}

/// A foundation or a repair.
#[derive(Clone)]
pub struct Building {
    pub builders: u8,
    pub seconds_remaining: f32,
}

#[derive(Clone)]
pub struct UnitAi {
    /// String record indices.
    #[expect(
        dead_code,
        reason = "changes with every order step and no panel shows it, so the detail stream leaves it out"
    )]
    pub state: Option<u32>,
    pub idle: bool,
    pub stance: Option<u32>,
    pub formation_controller: u32,
}

/// The formation an entity is in, and those its UnitAI can use.
#[derive(Clone, PartialEq, Eq)]
pub struct Formations {
    /// String record index of the template; the null formation's when in
    /// none.
    pub current: Option<u32>,
    pub list: Vec<Option<u32>>,
}

/// What the command row and the pack, gate and alert panels read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Commands {
    /// The hook's `COMMAND_*` bits, `SpeculaFeed.h` kind 12 group 33554432.
    pub flags: u16,
    /// String record index of AlertRaiser's target classes.
    pub alert_classes: Option<u32>,
    /// The ejectable occupants of a TurretHolder's points.
    pub turrets: u16,
}

#[derive(Clone)]
pub struct Progress {
    /// -1 when not packing or upgrading.
    pub pack: f32,
    pub upgrade: f32,
    /// String record index.
    pub upgrade_target: Option<u32>,
}

#[derive(Clone)]
pub struct Trader {
    /// String record index.
    pub goods: Option<u32>,
    pub gain: f32,
}

/// A trader's `GetFirstMarket` and `GetSecondMarket`, 0 for none, and its
/// `GetTraderGainMultiplier`.
#[derive(Clone)]
pub struct TradeRoute {
    pub first: u32,
    pub second: u32,
    pub multiplier: f32,
}

/// A range circle the selection shows, as `RangeOverlayManager` hands it to
/// the renderer.
#[derive(Clone)]
pub struct RangeOverlay {
    /// 0 attack, 1 aura, 2 heal.
    pub kind: u8,
    pub radius: f32,
    /// String record indices, relative to art/textures/selection/.
    pub texture: Option<u32>,
    pub mask: Option<u32>,
    pub thickness: f32,
}

/// An entity's rally points, none when unset.
#[derive(Clone)]
pub struct Rally {
    /// String record index of the flag's actor.
    pub marker: Option<u32>,
    pub points: Vec<RallyPoint>,
}

#[derive(Clone)]
pub struct RallyPoint {
    /// In metres.
    pub x: f32,
    pub z: f32,
    /// The line to the point, from it back to the previous one or the
    /// footprint's edge, x and z in metres.
    pub path: Vec<[f32; 2]>,
}

/// What a player's panels read that is not an entity's: the formations the
/// civ may use, the barter prices and the trade dialog's state; index 0 is
/// gaia.
#[derive(Clone, PartialEq)]
pub struct PlayerGui {
    /// String record indices of formation templates.
    pub formations: Vec<Option<u32>>,
    /// Resource code string index, buy price, sell price.
    pub prices: Vec<(Option<u32>, f32, f32)>,
    pub can_barter: bool,
    /// Resource code string index and percent, `GetTradingGoods`.
    pub trading_goods: Vec<(Option<u32>, u8)>,
    /// `GetTraderNumber`: land total, trading, garrisoned; ship total,
    /// trading.
    pub traders: [u16; 5],
}

/// A player's top panel numbers; index 0 is gaia.
pub struct Economy {
    pub population: i32,
    pub population_limit: i32,
    pub population_max: i32,
    /// Sorted by code.
    pub resources: Vec<Resource>,
    /// String record index.
    pub phase: Option<u32>,
    /// Heroes and the like, as `panelEntities`.
    pub panel_entities: Vec<u32>,
}

pub struct Resource {
    /// String record index.
    pub code: Option<u32>,
    pub amount: f32,
    pub gatherers: u16,
}

/// The kind 12 to 15 state of one feed. Kind 12 carries only the groups
/// that changed, so the rest must be kept here.
#[derive(Default)]
pub struct Details {
    pub entities: HashMap<u32, EntityDetails>,
    /// Production lists by index, from kind 17 or an older hook's kind 13.
    pub lists: HashMap<u32, Vec<ProductionItem>>,
    /// (old id, new id) of the next turn record.
    pub renames: Vec<(u32, u32)>,
    pub economy: Vec<Economy>,
    /// Kind 21, by player index.
    pub player_gui: Vec<PlayerGui>,
    /// The entities whose groups changed, and those gone, before the next
    /// turn record.
    pub changed: HashSet<u32>,
    pub gone: Vec<u32>,
    /// The hash of the latest JSON of each kind 18 and 19 key.
    pub data: BTreeMap<DataKey, u64>,
    /// The hash of each kind 20 stats JSON, by index.
    pub stats: HashMap<u32, u64>,
}

/// What a kind 18 or 19 record is about.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DataKey {
    /// A template name string and the player its values are for.
    Template { name: u32, player: i32 },
    /// A technology name string and a civ string.
    Technology { name: u32, civ: u32 },
    /// A formation template name string.
    Formation { name: u32 },
}

impl Details {
    pub fn apply(&mut self, changed: Vec<(u32, EntityDetails)>, gone: Vec<u32>) {
        for (id, mut groups) in changed {
            // The hook sends a stats record before the details that use it.
            groups.stats = groups
                .stats
                .and_then(|index| self.stats.get(&(index as u32)).copied());
            self.entities.entry(id).or_default().merge(groups);
            self.changed.insert(id);
        }
        for id in gone {
            self.entities.remove(&id);
            self.changed.remove(&id);
            self.gone.push(id);
        }
    }

    pub fn add_list(&mut self, index: u32, items: Vec<ProductionItem>) {
        self.lists.insert(index, items);
    }

    pub fn set_renames(&mut self, renames: Vec<(u32, u32)>) {
        self.renames = renames;
    }

    pub fn set_economy(&mut self, economy: Vec<Economy>) {
        self.economy = economy;
    }

    pub fn set_player_gui(&mut self, player_gui: Vec<PlayerGui>) {
        self.player_gui = player_gui;
    }

    pub fn set_data(&mut self, key: DataKey, hash: u64) {
        self.data.insert(key, hash);
    }

    pub fn set_stats(&mut self, index: u32, hash: u64) {
        self.stats.insert(index, hash);
    }

    /// The renames and changes belong to the turn just read.
    pub fn end_turn(&mut self) {
        self.renames.clear();
        self.changed.clear();
        self.gone.clear();
    }
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
    Players(Players),
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
    /// Right after the template record of the same index.
    TemplateDetails {
        index: u32,
        details: Box<TemplateDetails>,
    },
    /// Before the turn record: the groups that changed, by entity id, and
    /// the ids that no longer have any.
    Details {
        changed: Vec<(u32, EntityDetails)>,
        gone: Vec<u32>,
    },
    ProductionList {
        index: u32,
        items: Vec<ProductionItem>,
    },
    /// (old id, new id) of the next turn record.
    Renames(Vec<(u32, u32)>),
    Economy(Vec<Economy>),
    /// Kind 21.
    PlayerGui(Vec<PlayerGui>),
    /// Kind 18, 19 or 22: UTF-8 JSON.
    Data {
        key: DataKey,
        json: Vec<u8>,
    },
    /// Kind 20: UTF-8 JSON.
    Stats {
        index: u32,
        json: Vec<u8>,
    },
    /// Kind 23: the answer to an uplink query of `--play`, UTF-8 JSON.
    QueryReply {
        id: u32,
        json: Vec<u8>,
    },
    /// Kind 24: players 1 up, the runs of vertex states that changed.
    Los {
        /// Vertices per side.
        side: u32,
        players: Vec<Vec<LosRun>>,
    },
    /// Kind 25: the statistics summary, whole and per seat.
    Summary(Summary),
    /// The players who have the game paused; empty once resumed.
    Pause(Vec<i8>),
    /// Kind 27: the engine is running though paused, so no turns come.
    KeepAlive,
}

/// The statistics summary of one capture, as JSON.
#[derive(Clone)]
pub struct Summary {
    pub turn: u32,
    pub is_final: bool,
    /// Every player, for `/` and the history.
    pub full: Arc<[u8]>,
    /// Indexed by player id; a seat's own view, for `/play`.
    pub seats: Vec<Option<Arc<[u8]>>>,
}

/// `LOS_EXPLORED` and `LOS_VISIBLE` of a vertex state.
pub const LOS_EXPLORED: u8 = 1;
pub const LOS_VISIBLE: u8 = 2;

/// Consecutive vertices from `start`, row-major, x fastest.
pub struct LosRun {
    pub start: u32,
    pub states: Vec<u8>,
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
#[derive(Clone)]
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
#[derive(Clone)]
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
#[derive(Clone)]
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

/// The feed's version.
pub fn read_header(reader: &mut impl Read) -> io::Result<u32> {
    let mut header = [0u8; 8];
    reader.read_exact(&mut header)?;
    if header[..4] != MAGIC {
        return Err(invalid("not a Specula feed"));
    }
    let version = u32::from_le_bytes(header[4..].try_into().unwrap());
    if !(OLDEST_VERSION..=VERSION).contains(&version) {
        return Err(invalid(format!(
            "feed version {version}, expected {OLDEST_VERSION} to {VERSION}"
        )));
    }
    Ok(version)
}

/// `None` when the observer closed the feed between records. `version` is
/// the header's.
pub fn read_record(reader: &mut impl Read, version: u32) -> io::Result<Option<Record>> {
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
            RECORD_TURN => return decode_turn(&payload, version).map(Some),
            RECORD_MAP => {
                let mut cursor = Cursor(&payload);
                let size = cursor.u32()?;
                let speed = f32::from_bits(cursor.u32()?);
                return Ok(Some(Record::Map { size, speed }));
            }
            RECORD_TERRAIN => return decode_terrain(&payload).map(Some),
            RECORD_PLAYERS => return decode_players(&payload, version).map(Some),
            RECORD_STRING => {
                let mut cursor = Cursor(&payload);
                let index = cursor.u32()?;
                let text = String::from_utf8(cursor.0.to_vec())
                    .map_err(|_| invalid("string not UTF-8"))?;
                return Ok(Some(Record::String { index, text }));
            }
            RECORD_VARIANT_SET => return decode_variant_set(&payload).map(Some),
            RECORD_TERRITORY => return decode_territory(&payload).map(Some),
            RECORD_SOUNDS => return decode_sounds(&payload, version).map(Some),
            RECORD_LOS => return decode_los(&payload).map(Some),
            RECORD_SUMMARY => return decode_summary(&payload).map(Some),
            RECORD_PROJECTILES => return decode_projectiles(&payload).map(Some),
            RECORD_VARIABLE_SET => return decode_variable_set(&payload).map(Some),
            RECORD_TEMPLATE_DETAILS => return decode_template_details(&payload).map(Some),
            RECORD_DETAILS => return decode_details(&payload).map(Some),
            RECORD_STRING_LIST => return decode_string_list(&payload).map(Some),
            RECORD_PRODUCTION_LIST => return decode_production_list(&payload).map(Some),
            RECORD_STATS => {
                let mut cursor = Cursor(&payload);
                let index = cursor.u32()?;
                return Ok(Some(Record::Stats {
                    index,
                    json: cursor.0.to_vec(),
                }));
            }
            RECORD_PAUSE => {
                let mut cursor = Cursor(&payload);
                let count = cursor.u8()?;
                let mut players = Vec::with_capacity(usize::from(count));
                for _ in 0..count {
                    players.push(cursor.u8()? as i8);
                }
                return Ok(Some(Record::Pause(players)));
            }
            RECORD_KEEPALIVE => return Ok(Some(Record::KeepAlive)),
            RECORD_QUERY_REPLY => {
                let mut cursor = Cursor(&payload);
                let id = cursor.u32()?;
                return Ok(Some(Record::QueryReply {
                    id,
                    json: cursor.0.to_vec(),
                }));
            }
            RECORD_PLAYER_GUI => return decode_player_gui(&payload).map(Some),
            RECORD_TEMPLATE_DATA | RECORD_TECHNOLOGY_DATA | RECORD_FORMATION_DATA => {
                return decode_data(head[0], &payload).map(Some);
            }
            RECORD_RENAMES => return decode_renames(&payload).map(Some),
            RECORD_ECONOMY => return decode_economy(&payload).map(Some),
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

fn decode_turn(payload: &[u8], version: u32) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let turn = cursor.u32()?;
    let count = cursor.u32()? as usize;
    let size = if version < 15 {
        ENTITY_SIZE_V14
    } else {
        ENTITY_SIZE
    };
    if cursor.0.len() != count * size {
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
            visibility: 0,
            mirage: None,
        });
        let entity = entities.last_mut().unwrap();
        if version < 15 {
            entity.visibility = widen_visibility(cursor.u16()?);
        } else {
            entity.visibility = cursor.u32()?;
            let player = cursor.i32()?;
            let parent = optional(cursor.u32()?).filter(|parent| *parent != 0);
            entity.mirage = (player >= 0).then_some(Mirage { player, parent });
        }
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

fn decode_players(payload: &[u8], version: u32) -> io::Result<Record> {
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
        let shared_los = version >= 15 && cursor.u8()? != 0;
        players.push(Player {
            name,
            color: u32::from_be_bytes([0, r, g, b]),
            team,
            state,
            stances,
            civ,
            battle,
            shared_los,
        });
    }
    // Older hooks end the record with the players.
    let ceasefire_end = if cursor.0.is_empty() {
        0
    } else {
        cursor.u32()?
    };
    if !cursor.0.is_empty() {
        return Err(invalid("players record longer than its contents"));
    }
    Ok(Record::Players(Players {
        players,
        ceasefire_end,
    }))
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

fn decode_sounds(payload: &[u8], version: u32) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let count = cursor.u32()? as usize;
    let size = if version < 15 {
        SOUND_SIZE_V14
    } else {
        SOUND_SIZE
    };
    if cursor.0.len() != count * size {
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
            visibility: if version < 15 {
                widen_visibility(cursor.u16()?)
            } else {
                cursor.u32()?
            },
        });
    }
    Ok(Record::Sounds(sounds))
}

fn decode_los(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let side = cursor.u32()?;
    let vertices = side as usize * side as usize;
    let count = cursor.u8()?;
    let mut players = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let runs = cursor.u32()? as usize;
        let mut player = Vec::with_capacity(runs.min(vertices));
        for _ in 0..runs {
            let start = cursor.u32()?;
            let length = cursor.u32()? as usize;
            if start as usize + length > vertices {
                return Err(invalid("LOS run past the grid"));
            }
            let packed = cursor.bytes(length.div_ceil(4))?;
            let states = (0..length)
                .map(|k| (packed[k / 4] >> (2 * (k % 4))) & 3)
                .collect();
            player.push(LosRun { start, states });
        }
        players.push(player);
    }
    if !cursor.0.is_empty() {
        return Err(invalid("LOS record longer than its contents"));
    }
    Ok(Record::Los { side, players })
}

fn decode_summary(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let turn = cursor.u32()?;
    let flags = cursor.u8()?;
    let count = cursor.u8()?;
    let mut full = None;
    let mut seats: Vec<Option<Arc<[u8]>>> = Vec::new();
    for _ in 0..count {
        let view = usize::from(cursor.u8()?);
        let length = cursor.u32()? as usize;
        let json = cursor.bytes(length)?;
        std::str::from_utf8(json).map_err(|_| invalid("summary not UTF-8"))?;
        if view == 0 {
            full = Some(Arc::from(json));
        } else {
            if seats.len() <= view {
                seats.resize(view + 1, None);
            }
            seats[view] = Some(Arc::from(json));
        }
    }
    let full = full.ok_or_else(|| invalid("summary without the full view"))?;
    Ok(Record::Summary(Summary {
        turn,
        is_final: flags & 1 != 0,
        full,
        seats,
    }))
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

fn decode_template_details(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let index = cursor.u32()?;
    let mut details = TemplateDetails {
        flags: cursor.u16()?,
        overlay_texture: optional(cursor.u32()?),
        overlay_mask: optional(cursor.u32()?),
        line_thickness: cursor.f32()?,
        overlay_width: cursor.f32()?,
        overlay_depth: cursor.f32()?,
        footprint_width: cursor.f32()?,
        footprint_depth: cursor.f32()?,
        footprint_height: cursor.f32()?,
        bar_width: cursor.f32()?,
        bar_height: cursor.f32()?,
        bar_offset: cursor.f32()?,
        generic_name: optional(cursor.u32()?),
        specific_name: optional(cursor.u32()?),
        icon: optional(cursor.u32()?),
        group_name: optional(cursor.u32()?),
        rank: optional(cursor.u32()?),
        classes: optional(cursor.u32()?),
        visible_classes: optional(cursor.u32()?),
        select_sound: optional(cursor.u32()?),
        lang: None,
        phenotypes: None,
        components: cursor.u32()?,
        rally_texture: None,
        rally_mask: None,
        rally_thickness: 0.0,
        rally_start_cap: 0,
        rally_end_cap: 0,
        supply_type: None,
        gather_rates: None,
        dropsite_types: None,
        attack_types: None,
        order_sounds: None,
        garrison_classes: None,
        turret_classes: None,
    };
    // A hook from before lang and phenotypes ends here; the feed version
    // stayed, so both lengths are read.
    if !cursor.0.is_empty() {
        details.lang = optional(cursor.u32()?);
        details.phenotypes = optional(cursor.u32()?);
    }
    // Likewise for a hook from before rally points.
    if !cursor.0.is_empty() {
        details.rally_texture = optional(cursor.u32()?);
        details.rally_mask = optional(cursor.u32()?);
        details.rally_thickness = cursor.f32()?;
        details.rally_start_cap = cursor.u8()?;
        details.rally_end_cap = cursor.u8()?;
    }
    // And for a hook from before the right click's action strings.
    if !cursor.0.is_empty() {
        details.supply_type = optional(cursor.u32()?);
        details.gather_rates = optional(cursor.u32()?);
        details.dropsite_types = optional(cursor.u32()?);
        details.attack_types = optional(cursor.u32()?);
    }
    // And for one from before the order sounds.
    if !cursor.0.is_empty() {
        details.order_sounds = optional(cursor.u32()?);
    }
    // And for one from before the garrison and turret classes.
    if !cursor.0.is_empty() {
        details.garrison_classes = optional(cursor.u32()?);
        details.turret_classes = optional(cursor.u32()?);
    }
    if !cursor.0.is_empty() {
        return Err(invalid("template details record longer than its contents"));
    }
    Ok(Record::TemplateDetails {
        index,
        details: Box::new(details),
    })
}

fn decode_details(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let count = cursor.u32()? as usize;
    // At least an id and a mask each; a bad count must not reserve gigabytes.
    if cursor.0.len() < count * 8 {
        return Err(invalid("details record shorter than its entities"));
    }
    let mut changed = Vec::with_capacity(count);
    for _ in 0..count {
        let id = cursor.u32()?;
        let mask = cursor.u32()?;
        changed.push((id, decode_groups(&mut cursor, mask)?));
    }
    let count = cursor.u32()? as usize;
    if cursor.0.len() != count * 4 {
        return Err(invalid("details record length does not match its counts"));
    }
    let gone = (0..count)
        .map(|_| cursor.u32())
        .collect::<io::Result<_>>()?;
    Ok(Record::Details { changed, gone })
}

/// The groups follow each other in the order of their bits.
fn decode_groups(cursor: &mut Cursor, mask: u32) -> io::Result<EntityDetails> {
    let mut details = EntityDetails::default();
    if mask & DETAIL_HEALTH != 0 {
        details.health = Some((cursor.f32()?, cursor.f32()?));
    }
    if mask & DETAIL_CAPTURE != 0 {
        let count = cursor.u8()?;
        let points = (0..count)
            .map(|_| cursor.f32())
            .collect::<io::Result<_>>()?;
        details.capture = Some(Capture {
            points,
            max: cursor.f32()?,
        });
    }
    if mask & DETAIL_SUPPLY != 0 {
        details.supply = Some((cursor.f32()?, cursor.f32()?));
    }
    if mask & DETAIL_GATHERERS != 0 {
        details.gatherers = Some((cursor.u8()?, cursor.u8()?));
    }
    if mask & DETAIL_CARRYING != 0 {
        let count = cursor.u8()?;
        let carrying = (0..count)
            .map(|_| {
                Ok(Carried {
                    kind: optional(cursor.u32()?),
                    amount: cursor.f32()?,
                    capacity: cursor.f32()?,
                })
            })
            .collect::<io::Result<_>>()?;
        details.carrying = Some(carrying);
    }
    if mask & DETAIL_EXPERIENCE != 0 {
        details.experience = Some((cursor.f32()?, cursor.f32()?));
    }
    if mask & DETAIL_QUEUE != 0 {
        let autoqueue = cursor.u8()? != 0;
        let count = cursor.u8()?;
        let items = (0..count)
            .map(|_| {
                let id = cursor.u32()?;
                let technology = match cursor.u8()? {
                    QUEUE_UNIT => false,
                    QUEUE_TECHNOLOGY => true,
                    _ => return Err(invalid("unknown production queue item kind")),
                };
                Ok(QueueItem {
                    id,
                    technology,
                    name: optional(cursor.u32()?),
                    count: cursor.u16()?,
                    progress: cursor.f32()?,
                    time_remaining: cursor.f32()?,
                    paused: cursor.u8()? != 0,
                    missing_population: cursor.u16()?,
                })
            })
            .collect::<io::Result<_>>()?;
        details.queue = Some(Queue { autoqueue, items });
    }
    if mask & DETAIL_GARRISON != 0 {
        let capacity = cursor.u16()?;
        let occupied = cursor.u16()?;
        let count = cursor.u16()?;
        let entities = (0..count)
            .map(|_| Ok((cursor.u32()?, cursor.u32()?)))
            .collect::<io::Result<_>>()?;
        details.garrison = Some(Garrison {
            capacity,
            occupied,
            entities,
        });
    }
    if mask & DETAIL_FOUNDATION != 0 {
        details.foundation = Some(decode_building(cursor)?);
    }
    if mask & DETAIL_REPAIR != 0 {
        details.repair = Some(decode_building(cursor)?);
    }
    if mask & DETAIL_UNIT_AI != 0 {
        details.unit_ai = Some(UnitAi {
            state: optional(cursor.u32()?),
            idle: cursor.u8()? != 0,
            stance: optional(cursor.u32()?),
            formation_controller: cursor.u32()?,
        });
    }
    if mask & DETAIL_TRAINABLE != 0 {
        details.trainable = Some(cursor.u32()?);
    }
    if mask & DETAIL_RESEARCHABLE != 0 {
        details.researchable = Some(cursor.u32()?);
    }
    if mask & DETAIL_PROGRESS != 0 {
        details.progress = Some(Progress {
            pack: cursor.f32()?,
            upgrade: cursor.f32()?,
            upgrade_target: optional(cursor.u32()?),
        });
    }
    if mask & DETAIL_HOLDER != 0 {
        details.holder = Some(cursor.u32()?);
    }
    if mask & DETAIL_TRADER != 0 {
        details.trader = Some(Trader {
            goods: optional(cursor.u32()?),
            gain: cursor.f32()?,
        });
    }
    if mask & DETAIL_RANGES != 0 {
        let count = cursor.u8()?;
        let mut ranges = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            ranges.push(RangeOverlay {
                kind: cursor.u8()?,
                radius: cursor.f32()?,
                texture: optional(cursor.u32()?),
                mask: optional(cursor.u32()?),
                thickness: cursor.f32()?,
            });
        }
        details.ranges = Some(ranges);
    }
    if mask & DETAIL_GARRISON_OWNERS != 0 {
        let count = cursor.u16()?;
        details.garrison_owners = Some(
            (0..count)
                .map(|_| Ok(cursor.u8()? as i8))
                .collect::<io::Result<_>>()?,
        );
    }
    // A hook sends these or the older bits above, never both, so they share
    // the fields and the list numbering.
    if mask & DETAIL_TRAINABLE_ITEMS != 0 {
        details.trainable = Some(cursor.u32()?);
    }
    if mask & DETAIL_RESEARCHABLE_ITEMS != 0 {
        details.researchable = Some(cursor.u32()?);
    }
    if mask & DETAIL_RALLY != 0 {
        let count = cursor.u8()?;
        let marker = optional(cursor.u32()?);
        let mut points = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            let x = cursor.f32()?;
            let z = cursor.f32()?;
            let length = cursor.u16()?;
            let path = (0..length)
                .map(|_| Ok([cursor.f32()?, cursor.f32()?]))
                .collect::<io::Result<_>>()?;
            points.push(RallyPoint { x, z, path });
        }
        details.rally = Some(Rally { marker, points });
    }
    if mask & DETAIL_STATS != 0 {
        details.stats = Some(u64::from(cursor.u32()?));
    }
    if mask & DETAIL_BUILDABLE_ITEMS != 0 {
        details.buildable = Some(cursor.u32()?);
    }
    if mask & DETAIL_STANCES != 0 {
        let count = cursor.u8()?;
        details.stances = Some(
            (0..count)
                .map(|_| Ok(optional(cursor.u32()?)))
                .collect::<io::Result<_>>()?,
        );
    }
    if mask & DETAIL_FORMATIONS != 0 {
        let current = optional(cursor.u32()?);
        let count = cursor.u16()?;
        let list = (0..count)
            .map(|_| Ok(optional(cursor.u32()?)))
            .collect::<io::Result<_>>()?;
        details.formations = Some(Formations { current, list });
    }
    if mask & DETAIL_COMMANDS != 0 {
        details.commands = Some(Commands {
            flags: cursor.u16()?,
            alert_classes: optional(cursor.u32()?),
            turrets: cursor.u16()?,
        });
    }
    if mask & DETAIL_UPGRADES != 0 {
        details.upgrades = Some(cursor.u32()?);
    }
    if mask & DETAIL_TRADE_ROUTE != 0 {
        details.trade_route = Some(TradeRoute {
            first: cursor.u32()?,
            second: cursor.u32()?,
            multiplier: cursor.f32()?,
        });
    }
    if mask & DETAIL_MARKET != 0 {
        details.market_bonus = Some(cursor.f32()?);
    }
    if mask & DETAIL_TRADE_GARRISON != 0 {
        details.trade_garrison = Some(cursor.f32()?);
    }
    Ok(details)
}

fn decode_building(cursor: &mut Cursor) -> io::Result<Building> {
    Ok(Building {
        builders: cursor.u8()?,
        seconds_remaining: cursor.f32()?,
    })
}

fn decode_string_list(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let index = cursor.u32()?;
    let count = cursor.u32()? as usize;
    if cursor.0.len() != count * 4 {
        return Err(invalid(
            "string list record length does not match its count",
        ));
    }
    let items = (0..count)
        .map(|_| {
            Ok(ProductionItem {
                name: optional(cursor.u32()?),
                flags: ProductionItem::UNKNOWN,
                can_be_added: None,
                costs: Vec::new(),
            })
        })
        .collect::<io::Result<_>>()?;
    Ok(Record::ProductionList { index, items })
}

fn decode_data(kind: u8, payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let name = cursor.u32()?;
    let key = if kind == RECORD_FORMATION_DATA {
        DataKey::Formation { name }
    } else if kind == RECORD_TEMPLATE_DATA {
        DataKey::Template {
            name,
            player: cursor.u32()? as i32,
        }
    } else {
        DataKey::Technology {
            name,
            civ: cursor.u32()?,
        }
    };
    Ok(Record::Data {
        key,
        json: cursor.0.to_vec(),
    })
}

fn decode_production_list(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let index = cursor.u32()?;
    let count = cursor.u32()?;
    let mut items = Vec::new();
    for _ in 0..count {
        let name = optional(cursor.u32()?);
        let flags = cursor.u8()? & ProductionItem::HOOK_FLAGS;
        let can_be_added = Some(cursor.u16()?).filter(|limit| *limit != NO_LIMIT);
        let costs = (0..cursor.u8()?)
            .map(|_| Ok((optional(cursor.u32()?), cursor.f32()?)))
            .collect::<io::Result<_>>()?;
        items.push(ProductionItem {
            name,
            flags,
            can_be_added,
            costs,
        });
    }
    if !cursor.0.is_empty() {
        return Err(invalid("production list record has trailing bytes"));
    }
    Ok(Record::ProductionList { index, items })
}

fn decode_player_gui(payload: &[u8]) -> io::Result<Record> {
    // An engine built before the trade dialog's fields sends none, and the
    // feed version did not change with them.
    decode_player_gui_fields(payload, true)
        .or_else(|_| decode_player_gui_fields(payload, false))
        .map(Record::PlayerGui)
}

fn decode_player_gui_fields(payload: &[u8], trade: bool) -> io::Result<Vec<PlayerGui>> {
    let mut cursor = Cursor(payload);
    let count = cursor.u32()?;
    let mut players = Vec::new();
    for _ in 0..count {
        let formations = (0..cursor.u16()?)
            .map(|_| Ok(optional(cursor.u32()?)))
            .collect::<io::Result<_>>()?;
        let prices = (0..cursor.u8()?)
            .map(|_| Ok((optional(cursor.u32()?), cursor.f32()?, cursor.f32()?)))
            .collect::<io::Result<_>>()?;
        let mut player = PlayerGui {
            formations,
            prices,
            can_barter: false,
            trading_goods: Vec::new(),
            traders: [0; 5],
        };
        if trade {
            player.can_barter = cursor.u8()? != 0;
            player.trading_goods = (0..cursor.u8()?)
                .map(|_| Ok((optional(cursor.u32()?), cursor.u8()?)))
                .collect::<io::Result<_>>()?;
            for count in &mut player.traders {
                *count = cursor.u16()?;
            }
        }
        players.push(player);
    }
    if !cursor.0.is_empty() {
        return Err(invalid("player GUI record has trailing bytes"));
    }
    Ok(players)
}

fn decode_renames(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let count = cursor.u32()? as usize;
    if cursor.0.len() != count * 8 {
        return Err(invalid("renames record length does not match its count"));
    }
    let renames = (0..count)
        .map(|_| Ok((cursor.u32()?, cursor.u32()?)))
        .collect::<io::Result<_>>()?;
    Ok(Record::Renames(renames))
}

fn decode_economy(payload: &[u8]) -> io::Result<Record> {
    let mut cursor = Cursor(payload);
    let count = cursor.u32()?;
    let mut players = Vec::new();
    for _ in 0..count {
        let population = cursor.i32()?;
        let population_limit = cursor.i32()?;
        let population_max = cursor.i32()?;
        let count = cursor.u8()?;
        let resources = (0..count)
            .map(|_| {
                Ok(Resource {
                    code: optional(cursor.u32()?),
                    amount: cursor.f32()?,
                    gatherers: cursor.u16()?,
                })
            })
            .collect::<io::Result<_>>()?;
        let phase = optional(cursor.u32()?);
        let count = cursor.u16()?;
        let panel_entities = (0..count)
            .map(|_| cursor.u32())
            .collect::<io::Result<_>>()?;
        players.push(Economy {
            population,
            population_limit,
            population_max,
            resources,
            phase,
            panel_entities,
        });
    }
    if !cursor.0.is_empty() {
        return Err(invalid("economy record longer than its contents"));
    }
    Ok(Record::Economy(players))
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

    fn f32(&mut self) -> io::Result<f32> {
        self.u32().map(f32::from_bits)
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
