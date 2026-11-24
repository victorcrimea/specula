//! The orders a web player gives, as the page sends them to the server on
//! `/play/command`. The server turns each into the engine's command JSON
//! (`simulation/helpers/Commands.js`) for the player's seat.
//!
//! A message is one command: u8 kind, then its fields. Entity lists are a
//! varint count and the ids sorted, each as a varint delta from the one
//! before; positions are f32 metres; strings are a varint length and
//! UTF-8; `flags` is a u8 of the `QUEUED`, `PUSH_FRONT` and
//! `ALLOW_CAPTURE` bits. A freehand line (`WalkCustom`) is a varint count
//! and its points in sixteenths of a metre, each x and z a zigzag varint
//! delta from the point before (the first from 0, 0).
//!
//! A message starting with `QUERY_TAG` is a `Query` instead: a GuiInterface
//! call the session GUI makes while placing a structure, answered on the
//! same socket by a text message `{"id": id, "result": JSON}`.
//!
//! A message starting with `GROUP_TAG` is a control group, in both
//! directions (the direction tells them apart). From the page it is a
//! `GroupSave`: the group, a flags byte (bit 0 is "disjoint") and the
//! selection to keep. From the server it is a `GroupContents`: the group
//! and its members' handles. Neither is an engine order, so they are not
//! in `Command`.
//!
//! A message starting with `SELECTION_DEFINE_TAG` or `SELECTION_USE_TAG`
//! wraps a command, because orders are spammed with the same selection and
//! resending the whole list each time dominates their size. DEFINE is a
//! varint token and a full command, which the server keeps the list of for
//! the socket. USE is the token, the handles that left the kept list since
//! (as an entity list) and a command with an empty selection; the server
//! shrinks its list by them and fills it back in.

use crate::bytes::ByteReader;

/// The version of this format, which the server sends as the first byte of
/// its seat message; a page on another version must reload.
pub const COMMAND_VERSION: u8 = 13;

pub const QUEUED: u8 = 1;
pub const PUSH_FRONT: u8 = 2;
pub const ALLOW_CAPTURE: u8 = 4;

const WALK: u8 = 1;
const ATTACK_WALK: u8 = 2;
const ATTACK: u8 = 3;
const GATHER: u8 = 4;
const REPAIR: u8 = 5;
const GARRISON: u8 = 6;
const UNLOAD_ALL: u8 = 7;
const CONSTRUCT: u8 = 8;
const TRAIN: u8 = 9;
const RESEARCH: u8 = 10;
const STOP: u8 = 11;
const DELETE: u8 = 12;
const RALLY: u8 = 13;
const STANCE: u8 = 14;
const RETURN_RESOURCE: u8 = 15;
const CONSTRUCT_WALL: u8 = 16;
const STOP_PRODUCTION: u8 = 17;
const AUTOQUEUE: u8 = 18;
const FORMATION: u8 = 19;
const UNLOAD_ALL_BY_OWNER: u8 = 20;
const UNLOAD_TURRETS: u8 = 21;
const LEAVE_TURRET: u8 = 22;
const BACK_TO_WORK: u8 = 23;
const REMOVE_GUARD: u8 = 24;
const DROPSITE_SHARING: u8 = 25;
const GUARD: u8 = 26;
const OCCUPY_TURRET: u8 = 27;
const PATROL: u8 = 28;
const CALL_TO_ARMS: u8 = 29;
const ALERT_RAISE: u8 = 30;
const ALERT_END: u8 = 31;
const UPGRADE: u8 = 32;
const CANCEL_UPGRADE: u8 = 33;
const BARTER: u8 = 34;
const SET_TRADING_GOODS: u8 = 35;
const SETUP_TRADE_ROUTE: u8 = 36;
const CANCEL_TRADE_ROUTE: u8 = 37;
const LOCK_GATE: u8 = 38;
const PACK: u8 = 39;
const CANCEL_PACK: u8 = 40;
const UNLOAD_TEMPLATE: u8 = 41;
const TRIBUTE: u8 = 42;
const DIPLOMACY: u8 = 43;
const ATTACK_REQUEST: u8 = 44;
const SPY_REQUEST: u8 = 45;
const WALK_CUSTOM: u8 = 46;
const ATTACK_WALK_CUSTOM: u8 = 47;

/// The stance codes the wire player table uses.
pub const DIPLOMACY_ENEMY: u8 = 0;
pub const DIPLOMACY_ALLY: u8 = 1;
pub const DIPLOMACY_NEUTRAL: u8 = 2;

/// Above every command kind.
pub const QUERY_TAG: u8 = 0x80;
const QUERY_PREVIEW: u8 = 1;
const QUERY_SNAP: u8 = 2;
const QUERY_WALL: u8 = 3;

/// Above every command kind, and not `QUERY_TAG`.
pub const GROUP_TAG: u8 = 0x81;
/// Above every command kind, and not `QUERY_TAG` or `GROUP_TAG`.
pub const SELECTION_DEFINE_TAG: u8 = 0x82;
pub const SELECTION_USE_TAG: u8 = 0x83;
/// Vanilla's control groups, the keys 1-9 and 0.
pub const GROUP_COUNT: usize = 10;
/// The same as the page's selection limit (`MAX_SELECTION` in
/// specula-web), so a whole selection fits in a group; not imported, as
/// the crates do not share it.
pub const MAX_GROUP: usize = 1500;
const GROUP_DISJOINT: u8 = 1;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// `formation` is the default formation's template, empty for none
    /// (`g_AutoFormation.getDefault()` returning undefined).
    Walk {
        flags: u8,
        entities: Vec<u32>,
        x: f32,
        z: f32,
        formation: String,
    },
    AttackWalk {
        flags: u8,
        entities: Vec<u32>,
        x: f32,
        z: f32,
    },
    /// `walk-custom`: the entities spread along `line` (x and z in
    /// metres, as `quantize_line` leaves them) by `freehand_points`, the
    /// lowest id at the line's start, or at its end when `reversed`. The
    /// points are sent rather than worked out by the page because they are
    /// a function of the line and the count, and a line is far shorter
    /// than a point per unit.
    WalkCustom {
        flags: u8,
        entities: Vec<u32>,
        line: Vec<[f32; 2]>,
        reversed: bool,
    },
    /// `attack-walk-custom`, as `WalkCustom`.
    AttackWalkCustom {
        flags: u8,
        entities: Vec<u32>,
        line: Vec<[f32; 2]>,
        reversed: bool,
    },
    Attack {
        flags: u8,
        entities: Vec<u32>,
        target: u32,
    },
    Gather {
        flags: u8,
        entities: Vec<u32>,
        target: u32,
    },
    Repair {
        flags: u8,
        entities: Vec<u32>,
        target: u32,
    },
    Garrison {
        flags: u8,
        entities: Vec<u32>,
        target: u32,
    },
    ReturnResource {
        flags: u8,
        entities: Vec<u32>,
        target: u32,
    },
    UnloadAll {
        holders: Vec<u32>,
    },
    Construct {
        flags: u8,
        entities: Vec<u32>,
        template: String,
        x: f32,
        z: f32,
        angle: f32,
        seed: u32,
    },
    Train {
        entities: Vec<u32>,
        template: String,
        count: u32,
    },
    Research {
        flags: u8,
        entity: u32,
        template: String,
    },
    Stop {
        flags: u8,
        entities: Vec<u32>,
    },
    Delete {
        entities: Vec<u32>,
    },
    /// `order` is the rally point's command ("walk", "gather", ...), with
    /// its target entity, 0 for none. On a resource, `resource_type`
    /// ("food.fruit") and `resource_template` are the target's, empty
    /// otherwise: once the target is gone, `GetRallyPointCommands` gathers
    /// near the point by them.
    Rally {
        flags: u8,
        entities: Vec<u32>,
        x: f32,
        z: f32,
        order: String,
        target: u32,
        resource_type: String,
        resource_template: String,
        /// The "trade" order's origin market, 0 otherwise.
        source: u32,
    },
    Stance {
        entities: Vec<u32>,
        name: String,
    },
    /// `pieces` as `SetWallPlacementPreview` laid them out; `tower` is the
    /// wall set's tower template, all `TryConstructWall` reads of the set.
    /// Snapped entities are 0 for none.
    ConstructWall {
        flags: u8,
        entities: Vec<u32>,
        tower: String,
        pieces: Vec<WallPiece>,
        start_snapped: u32,
        end_snapped: u32,
    },
    /// Removes item `id` from `entity`'s production queue.
    StopProduction {
        entity: u32,
        id: u32,
    },
    /// Turns the producers' auto-queue on or off.
    Autoqueue {
        on: bool,
        entities: Vec<u32>,
    },
    /// `performFormation`: moves the entities into formation template
    /// `name`, the null formation's to leave theirs.
    Formation {
        entities: Vec<u32>,
        name: String,
    },
    /// "unload-all-by-owner": the player's own units out of others'
    /// holders.
    UnloadAllByOwner {
        holders: Vec<u32>,
    },
    UnloadTurrets {
        entities: Vec<u32>,
    },
    LeaveTurret {
        entities: Vec<u32>,
    },
    BackToWork {
        entities: Vec<u32>,
    },
    RemoveGuard {
        entities: Vec<u32>,
    },
    /// "set-dropsite-sharing".
    DropsiteSharing {
        shared: bool,
        entities: Vec<u32>,
    },
    Guard {
        flags: u8,
        entities: Vec<u32>,
        target: u32,
    },
    OccupyTurret {
        flags: u8,
        entities: Vec<u32>,
        target: u32,
    },
    /// `formation` as in `Walk`; `target` is the entity clicked, 0 for
    /// none.
    Patrol {
        flags: u8,
        entities: Vec<u32>,
        x: f32,
        z: f32,
        target: u32,
        formation: String,
    },
    CallToArms {
        flags: u8,
        entities: Vec<u32>,
        x: f32,
        z: f32,
    },
    AlertRaise {
        entities: Vec<u32>,
    },
    /// "alert-end".
    AlertEnd {
        entities: Vec<u32>,
    },
    Upgrade {
        entities: Vec<u32>,
        template: String,
    },
    CancelUpgrade {
        entities: Vec<u32>,
    },
    /// Sells `amount` of resource `sell` for `buy` at a market's prices.
    Barter {
        sell: String,
        buy: String,
        amount: u32,
    },
    /// The share in percent of each resource code the traders carry.
    SetTradingGoods {
        goods: Vec<(String, u8)>,
    },
    /// "tribute": `amount` of resource `resource` to player `player`.
    Tribute {
        player: u8,
        resource: String,
        amount: u32,
    },
    /// "diplomacy": our stance towards `player`, a `DIPLOMACY_*` code.
    Diplomacy {
        player: u8,
        stance: u8,
    },
    /// "attack-request" against `player`; the server fills `source` with
    /// the seat's own player.
    AttackRequest {
        player: u8,
    },
    /// "spy-request" against `player`.
    SpyRequest {
        player: u8,
    },
    /// "setup-trade-route": the market becomes the traders' origin, or
    /// their destination once they have one.
    SetupTradeRoute {
        flags: u8,
        entities: Vec<u32>,
        target: u32,
    },
    /// "cancel-setup-trade-route": the origin is dropped.
    CancelTradeRoute {
        flags: u8,
        entities: Vec<u32>,
        target: u32,
    },
    /// "lock-gate": locks the gates among them, or unlocks them.
    LockGate {
        lock: bool,
        entities: Vec<u32>,
    },
    /// "pack": packs them, or unpacks them.
    Pack {
        pack: bool,
        entities: Vec<u32>,
    },
    /// "cancel-pack": cancels a pack, or an unpack.
    CancelPack {
        pack: bool,
        entities: Vec<u32>,
    },
    /// "unload-template": one entity of `template` (a selection group
    /// name or template name) owned by `owner` leaves the holders, or
    /// every such entity with `all`.
    UnloadTemplate {
        all: bool,
        template: String,
        owner: u32,
        holders: Vec<u32>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct WallPiece {
    pub template: String,
    pub x: f32,
    pub z: f32,
    pub angle: f32,
}

/// Far more pieces than a wall across the largest map has.
const MAX_PIECES: usize = 1024;
/// Far more resources than any mod trades.
const MAX_GOODS: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    /// Echoed in the reply.
    pub id: u32,
    pub kind: QueryKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum QueryKind {
    /// `SetBuildingPlacementPreview`; an empty template ends the preview.
    Preview {
        template: String,
        x: f32,
        z: f32,
        angle: f32,
        seed: u32,
    },
    /// `GetFoundationSnapData`, with `edges` for the snap to the edges of
    /// the static obstructions near the point.
    Snap {
        template: String,
        x: f32,
        z: f32,
        angle: f32,
        edges: bool,
    },
    /// `SetWallPlacementPreview` of the wall set template's set; no start
    /// ends the preview. `snap` are the towers the ends may snap to, which
    /// the session GUI picks with `PickSimilarPlayerEntities`.
    Wall {
        wallset: String,
        start: Option<(f32, f32)>,
        end: Option<(f32, f32)>,
        snap: Vec<u32>,
    },
}

impl Query {
    /// As `Command::entity_ids_mut`.
    pub fn entity_ids_mut(&mut self) -> Vec<&mut u32> {
        match &mut self.kind {
            QueryKind::Wall { snap, .. } => snap.iter_mut().filter(|id| **id != 0).collect(),
            _ => Vec::new(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Out(vec![QUERY_TAG]);
        out.varint(u64::from(self.id));
        match &self.kind {
            QueryKind::Preview {
                template,
                x,
                z,
                angle,
                seed,
            } => {
                out.0.push(QUERY_PREVIEW);
                out.text(template);
                out.point(*x, *z);
                out.0.extend_from_slice(&angle.to_le_bytes());
                out.varint(u64::from(*seed));
            }
            QueryKind::Snap {
                template,
                x,
                z,
                angle,
                edges,
            } => {
                out.0.push(QUERY_SNAP);
                out.text(template);
                out.point(*x, *z);
                out.0.extend_from_slice(&angle.to_le_bytes());
                out.0.push(u8::from(*edges));
            }
            QueryKind::Wall {
                wallset,
                start,
                end,
                snap,
            } => {
                out.0.push(QUERY_WALL);
                out.text(wallset);
                out.0
                    .push(u8::from(start.is_some()) | u8::from(end.is_some()) << 1);
                for (x, z) in start.iter().chain(end) {
                    out.point(*x, *z);
                }
                out.entities(snap);
            }
        }
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Query, &'static str> {
        let mut input = ByteReader(bytes);
        if input.u8()? != QUERY_TAG {
            return Err("not a query");
        }
        let id = input.varint_u32()?;
        let kind = match input.u8()? {
            QUERY_PREVIEW => {
                let template = input.text()?;
                let (x, z) = point(&mut input)?;
                QueryKind::Preview {
                    template,
                    x,
                    z,
                    angle: finite(input.f32()?)?,
                    seed: input.varint_u32()?,
                }
            }
            QUERY_SNAP => {
                let template = input.text()?;
                let (x, z) = point(&mut input)?;
                QueryKind::Snap {
                    template,
                    x,
                    z,
                    angle: finite(input.f32()?)?,
                    edges: input.u8()? != 0,
                }
            }
            QUERY_WALL => {
                let wallset = input.text()?;
                let present = input.u8()?;
                let start = if present & 1 != 0 {
                    Some(point(&mut input)?)
                } else {
                    None
                };
                let end = if present & 2 != 0 {
                    Some(point(&mut input)?)
                } else {
                    None
                };
                QueryKind::Wall {
                    wallset,
                    start,
                    end,
                    snap: entities(&mut input)?,
                }
            }
            _ => return Err("unknown query"),
        };
        if !input.0.is_empty() {
            return Err("trailing bytes after the query");
        }
        Ok(Query { id, kind })
    }
}

/// A page's request to keep a selection as a control group; an empty
/// selection clears the group.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupSave {
    pub group: u8,
    /// Take the entities out of the other groups first.
    pub disjoint: bool,
    pub entities: Vec<u32>,
}

impl GroupSave {
    /// As `Command::entity_ids_mut`.
    pub fn entity_ids_mut(&mut self) -> Vec<&mut u32> {
        self.entities.iter_mut().collect()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Out(vec![GROUP_TAG]);
        out.0.push(self.group);
        out.0.push(if self.disjoint { GROUP_DISJOINT } else { 0 });
        out.entities(&self.entities);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<GroupSave, &'static str> {
        let mut input = ByteReader(bytes);
        if input.u8()? != GROUP_TAG {
            return Err("not a control group");
        }
        let group = input.u8()?;
        if usize::from(group) >= GROUP_COUNT {
            return Err("control group out of range");
        }
        let flags = input.u8()?;
        let entities = entities(&mut input)?;
        if !input.0.is_empty() {
            return Err("trailing bytes after the control group");
        }
        Ok(GroupSave {
            group,
            disjoint: flags & GROUP_DISJOINT != 0,
            entities,
        })
    }
}

/// An order that names its selection by the socket's token.
#[derive(Clone, Debug, PartialEq)]
pub enum Tokened {
    /// `command.selection()` is the full list, kept under `token`.
    Define { token: u32, command: Command },
    /// `command.selection()` is empty; the kept list minus `removed`
    /// is the selection, and is kept from now on.
    Use {
        token: u32,
        removed: Vec<u32>,
        command: Command,
    },
}

impl Tokened {
    pub fn encode(&self) -> Vec<u8> {
        let (tag, token, command) = match self {
            Tokened::Define { token, command } => (SELECTION_DEFINE_TAG, token, command),
            Tokened::Use { token, command, .. } => (SELECTION_USE_TAG, token, command),
        };
        let mut out = Out(vec![tag]);
        out.varint(u64::from(*token));
        if let Tokened::Use { removed, .. } = self {
            out.entities(removed);
        }
        out.0.extend_from_slice(&command.encode());
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Tokened, &'static str> {
        let mut input = ByteReader(bytes);
        let tag = input.u8()?;
        if tag != SELECTION_DEFINE_TAG && tag != SELECTION_USE_TAG {
            return Err("not a tokened order");
        }
        let token = input.varint_u32()?;
        let removed = if tag == SELECTION_USE_TAG {
            entities(&mut input)?
        } else {
            Vec::new()
        };
        let command = Command::decode(input.0)?;
        let selection = command.selection().ok_or("order has no selection")?;
        if tag == SELECTION_USE_TAG {
            if !selection.is_empty() {
                return Err("token order with a selection");
            }
            Ok(Tokened::Use {
                token,
                removed,
                command,
            })
        } else {
            if selection.is_empty() {
                return Err("token definition without a selection");
            }
            Ok(Tokened::Define { token, command })
        }
    }
}

/// A control group's members, as the server pushes it to the page.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupContents {
    pub group: u8,
    pub entities: Vec<u32>,
}

impl GroupContents {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Out(vec![GROUP_TAG]);
        out.0.push(self.group);
        out.entities(&self.entities);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<GroupContents, &'static str> {
        let mut input = ByteReader(bytes);
        if input.u8()? != GROUP_TAG {
            return Err("not a control group");
        }
        let group = input.u8()?;
        if usize::from(group) >= GROUP_COUNT {
            return Err("control group out of range");
        }
        let entities = entities(&mut input)?;
        if !input.0.is_empty() {
            return Err("trailing bytes after the control group");
        }
        Ok(GroupContents { group, entities })
    }
}

impl Command {
    /// The entities the order is given to; `holders` and a lone `entity`
    /// are not a selection.
    pub fn selection(&self) -> Option<&Vec<u32>> {
        match self {
            Command::Walk { entities, .. }
            | Command::AttackWalk { entities, .. }
            | Command::WalkCustom { entities, .. }
            | Command::AttackWalkCustom { entities, .. }
            | Command::Attack { entities, .. }
            | Command::Gather { entities, .. }
            | Command::Repair { entities, .. }
            | Command::Garrison { entities, .. }
            | Command::ReturnResource { entities, .. }
            | Command::Construct { entities, .. }
            | Command::Train { entities, .. }
            | Command::Stop { entities, .. }
            | Command::Delete { entities, .. }
            | Command::Rally { entities, .. }
            | Command::Stance { entities, .. }
            | Command::ConstructWall { entities, .. }
            | Command::Autoqueue { entities, .. }
            | Command::Formation { entities, .. }
            | Command::UnloadTurrets { entities, .. }
            | Command::LeaveTurret { entities, .. }
            | Command::BackToWork { entities, .. }
            | Command::RemoveGuard { entities, .. }
            | Command::DropsiteSharing { entities, .. }
            | Command::Guard { entities, .. }
            | Command::OccupyTurret { entities, .. }
            | Command::Patrol { entities, .. }
            | Command::CallToArms { entities, .. }
            | Command::AlertRaise { entities, .. }
            | Command::AlertEnd { entities, .. }
            | Command::Upgrade { entities, .. }
            | Command::CancelUpgrade { entities, .. }
            | Command::SetupTradeRoute { entities, .. }
            | Command::CancelTradeRoute { entities, .. }
            | Command::LockGate { entities, .. }
            | Command::Pack { entities, .. }
            | Command::CancelPack { entities, .. } => Some(entities),
            Command::Research { .. }
            | Command::StopProduction { .. }
            | Command::UnloadAll { .. }
            | Command::UnloadAllByOwner { .. }
            | Command::UnloadTemplate { .. }
            | Command::Barter { .. }
            | Command::SetTradingGoods { .. }
            | Command::Tribute { .. }
            | Command::Diplomacy { .. }
            | Command::AttackRequest { .. }
            | Command::SpyRequest { .. } => None,
        }
    }

    pub fn selection_mut(&mut self) -> Option<&mut Vec<u32>> {
        match self {
            Command::Walk { entities, .. }
            | Command::AttackWalk { entities, .. }
            | Command::WalkCustom { entities, .. }
            | Command::AttackWalkCustom { entities, .. }
            | Command::Attack { entities, .. }
            | Command::Gather { entities, .. }
            | Command::Repair { entities, .. }
            | Command::Garrison { entities, .. }
            | Command::ReturnResource { entities, .. }
            | Command::Construct { entities, .. }
            | Command::Train { entities, .. }
            | Command::Stop { entities, .. }
            | Command::Delete { entities, .. }
            | Command::Rally { entities, .. }
            | Command::Stance { entities, .. }
            | Command::ConstructWall { entities, .. }
            | Command::Autoqueue { entities, .. }
            | Command::Formation { entities, .. }
            | Command::UnloadTurrets { entities, .. }
            | Command::LeaveTurret { entities, .. }
            | Command::BackToWork { entities, .. }
            | Command::RemoveGuard { entities, .. }
            | Command::DropsiteSharing { entities, .. }
            | Command::Guard { entities, .. }
            | Command::OccupyTurret { entities, .. }
            | Command::Patrol { entities, .. }
            | Command::CallToArms { entities, .. }
            | Command::AlertRaise { entities, .. }
            | Command::AlertEnd { entities, .. }
            | Command::Upgrade { entities, .. }
            | Command::CancelUpgrade { entities, .. }
            | Command::SetupTradeRoute { entities, .. }
            | Command::CancelTradeRoute { entities, .. }
            | Command::LockGate { entities, .. }
            | Command::Pack { entities, .. }
            | Command::CancelPack { entities, .. } => Some(entities),
            Command::Research { .. }
            | Command::StopProduction { .. }
            | Command::UnloadAll { .. }
            | Command::UnloadAllByOwner { .. }
            | Command::UnloadTemplate { .. }
            | Command::Barter { .. }
            | Command::SetTradingGoods { .. }
            | Command::Tribute { .. }
            | Command::Diplomacy { .. }
            | Command::AttackRequest { .. }
            | Command::SpyRequest { .. } => None,
        }
    }

    /// Every entity id the command names, for a server that numbers the
    /// entities its player is shown on its own (a seat's handles). An id
    /// of 0, "none", is left out.
    pub fn entity_ids_mut(&mut self) -> Vec<&mut u32> {
        let mut ids: Vec<&mut u32> = Vec::new();
        match self {
            Command::Walk { entities, .. }
            | Command::AttackWalk { entities, .. }
            | Command::WalkCustom { entities, .. }
            | Command::AttackWalkCustom { entities, .. }
            | Command::Construct { entities, .. }
            | Command::Train { entities, .. }
            | Command::Stop { entities, .. }
            | Command::Delete { entities }
            | Command::Stance { entities, .. }
            | Command::Autoqueue { entities, .. }
            | Command::Formation { entities, .. }
            | Command::UnloadTurrets { entities }
            | Command::LeaveTurret { entities }
            | Command::BackToWork { entities }
            | Command::RemoveGuard { entities }
            | Command::DropsiteSharing { entities, .. }
            | Command::CallToArms { entities, .. }
            | Command::AlertRaise { entities }
            | Command::AlertEnd { entities }
            | Command::Upgrade { entities, .. }
            | Command::CancelUpgrade { entities }
            | Command::LockGate { entities, .. }
            | Command::Pack { entities, .. }
            | Command::CancelPack { entities, .. } => ids.extend(entities.iter_mut()),
            Command::Attack {
                entities, target, ..
            }
            | Command::Gather {
                entities, target, ..
            }
            | Command::Repair {
                entities, target, ..
            }
            | Command::Garrison {
                entities, target, ..
            }
            | Command::ReturnResource {
                entities, target, ..
            }
            | Command::Guard {
                entities, target, ..
            }
            | Command::OccupyTurret {
                entities, target, ..
            }
            | Command::SetupTradeRoute {
                entities, target, ..
            }
            | Command::CancelTradeRoute {
                entities, target, ..
            }
            | Command::Patrol {
                entities, target, ..
            } => {
                ids.extend(entities.iter_mut());
                ids.push(target);
            }
            Command::UnloadAll { holders }
            | Command::UnloadAllByOwner { holders }
            | Command::UnloadTemplate { holders, .. } => ids.extend(holders.iter_mut()),
            Command::Rally {
                entities,
                target,
                source,
                ..
            } => {
                ids.extend(entities.iter_mut());
                ids.push(target);
                ids.push(source);
            }
            Command::Research { entity, .. } | Command::StopProduction { entity, .. } => {
                ids.push(entity)
            }
            Command::ConstructWall {
                entities,
                start_snapped,
                end_snapped,
                ..
            } => {
                ids.extend(entities.iter_mut());
                ids.push(start_snapped);
                ids.push(end_snapped);
            }
            Command::Barter { .. }
            | Command::SetTradingGoods { .. }
            | Command::Tribute { .. }
            | Command::Diplomacy { .. }
            | Command::AttackRequest { .. }
            | Command::SpyRequest { .. } => {}
        }
        ids.retain(|id| **id != 0);
        ids
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Out(Vec::new());
        match self {
            Command::Walk {
                flags,
                entities,
                x,
                z,
                formation,
            } => {
                out.head(WALK, *flags, entities);
                out.point(*x, *z);
                out.text(formation);
            }
            Command::AttackWalk {
                flags,
                entities,
                x,
                z,
            } => {
                out.head(ATTACK_WALK, *flags, entities);
                out.point(*x, *z);
            }
            Command::WalkCustom {
                flags,
                entities,
                line,
                reversed,
            } => out.freehand(WALK_CUSTOM, *flags, entities, line, *reversed),
            Command::AttackWalkCustom {
                flags,
                entities,
                line,
                reversed,
            } => out.freehand(ATTACK_WALK_CUSTOM, *flags, entities, line, *reversed),
            Command::Attack {
                flags,
                entities,
                target,
            } => out.targeted(ATTACK, *flags, entities, *target),
            Command::Gather {
                flags,
                entities,
                target,
            } => out.targeted(GATHER, *flags, entities, *target),
            Command::Repair {
                flags,
                entities,
                target,
            } => out.targeted(REPAIR, *flags, entities, *target),
            Command::Garrison {
                flags,
                entities,
                target,
            } => out.targeted(GARRISON, *flags, entities, *target),
            Command::ReturnResource {
                flags,
                entities,
                target,
            } => out.targeted(RETURN_RESOURCE, *flags, entities, *target),
            Command::UnloadAll { holders } => {
                out.0.push(UNLOAD_ALL);
                out.entities(holders);
            }
            Command::Construct {
                flags,
                entities,
                template,
                x,
                z,
                angle,
                seed,
            } => {
                out.head(CONSTRUCT, *flags, entities);
                out.text(template);
                out.point(*x, *z);
                out.0.extend_from_slice(&angle.to_le_bytes());
                out.varint(u64::from(*seed));
            }
            Command::Train {
                entities,
                template,
                count,
            } => {
                out.0.push(TRAIN);
                out.entities(entities);
                out.text(template);
                out.varint(u64::from(*count));
            }
            Command::Research {
                flags,
                entity,
                template,
            } => {
                out.0.extend_from_slice(&[RESEARCH, *flags]);
                out.varint(u64::from(*entity));
                out.text(template);
            }
            Command::Stop { flags, entities } => out.head(STOP, *flags, entities),
            Command::Delete { entities } => {
                out.0.push(DELETE);
                out.entities(entities);
            }
            Command::Rally {
                flags,
                entities,
                x,
                z,
                order,
                target,
                resource_type,
                resource_template,
                source,
            } => {
                out.head(RALLY, *flags, entities);
                out.point(*x, *z);
                out.text(order);
                out.varint(u64::from(*target));
                out.text(resource_type);
                out.text(resource_template);
                out.varint(u64::from(*source));
            }
            Command::Stance { entities, name } => {
                out.0.push(STANCE);
                out.entities(entities);
                out.text(name);
            }
            Command::ConstructWall {
                flags,
                entities,
                tower,
                pieces,
                start_snapped,
                end_snapped,
            } => {
                out.head(CONSTRUCT_WALL, *flags, entities);
                out.text(tower);
                out.varint(pieces.len() as u64);
                for piece in pieces {
                    out.text(&piece.template);
                    out.point(piece.x, piece.z);
                    out.0.extend_from_slice(&piece.angle.to_le_bytes());
                }
                out.varint(u64::from(*start_snapped));
                out.varint(u64::from(*end_snapped));
            }
            Command::StopProduction { entity, id } => {
                out.0.push(STOP_PRODUCTION);
                out.varint(u64::from(*entity));
                out.varint(u64::from(*id));
            }
            Command::Autoqueue { on, entities } => {
                out.0.extend_from_slice(&[AUTOQUEUE, u8::from(*on)]);
                out.entities(entities);
            }
            Command::Formation { entities, name } => {
                out.0.push(FORMATION);
                out.entities(entities);
                out.text(name);
            }
            Command::UnloadAllByOwner { holders } => {
                out.0.push(UNLOAD_ALL_BY_OWNER);
                out.entities(holders);
            }
            Command::UnloadTurrets { entities } => {
                out.0.push(UNLOAD_TURRETS);
                out.entities(entities);
            }
            Command::LeaveTurret { entities } => {
                out.0.push(LEAVE_TURRET);
                out.entities(entities);
            }
            Command::BackToWork { entities } => {
                out.0.push(BACK_TO_WORK);
                out.entities(entities);
            }
            Command::RemoveGuard { entities } => {
                out.0.push(REMOVE_GUARD);
                out.entities(entities);
            }
            Command::AlertRaise { entities } => {
                out.0.push(ALERT_RAISE);
                out.entities(entities);
            }
            Command::AlertEnd { entities } => {
                out.0.push(ALERT_END);
                out.entities(entities);
            }
            Command::Upgrade { entities, template } => {
                out.0.push(UPGRADE);
                out.entities(entities);
                out.text(template);
            }
            Command::CancelUpgrade { entities } => {
                out.0.push(CANCEL_UPGRADE);
                out.entities(entities);
            }
            Command::Barter { sell, buy, amount } => {
                out.0.push(BARTER);
                out.text(sell);
                out.text(buy);
                out.varint(u64::from(*amount));
            }
            Command::SetTradingGoods { goods } => {
                out.0.push(SET_TRADING_GOODS);
                out.varint(goods.len() as u64);
                for (code, percent) in goods {
                    out.text(code);
                    out.0.push(*percent);
                }
            }
            Command::Tribute {
                player,
                resource,
                amount,
            } => {
                out.0.push(TRIBUTE);
                out.0.push(*player);
                out.text(resource);
                out.varint(u64::from(*amount));
            }
            Command::Diplomacy { player, stance } => {
                out.0.extend_from_slice(&[DIPLOMACY, *player, *stance]);
            }
            Command::AttackRequest { player } => {
                out.0.extend_from_slice(&[ATTACK_REQUEST, *player]);
            }
            Command::SpyRequest { player } => {
                out.0.extend_from_slice(&[SPY_REQUEST, *player]);
            }
            Command::DropsiteSharing { shared, entities } => {
                out.0
                    .extend_from_slice(&[DROPSITE_SHARING, u8::from(*shared)]);
                out.entities(entities);
            }
            Command::Guard {
                flags,
                entities,
                target,
            } => out.targeted(GUARD, *flags, entities, *target),
            Command::OccupyTurret {
                flags,
                entities,
                target,
            } => out.targeted(OCCUPY_TURRET, *flags, entities, *target),
            Command::SetupTradeRoute {
                flags,
                entities,
                target,
            } => out.targeted(SETUP_TRADE_ROUTE, *flags, entities, *target),
            Command::CancelTradeRoute {
                flags,
                entities,
                target,
            } => out.targeted(CANCEL_TRADE_ROUTE, *flags, entities, *target),
            Command::Patrol {
                flags,
                entities,
                x,
                z,
                target,
                formation,
            } => {
                out.head(PATROL, *flags, entities);
                out.point(*x, *z);
                out.varint(u64::from(*target));
                out.text(formation);
            }
            Command::CallToArms {
                flags,
                entities,
                x,
                z,
            } => {
                out.head(CALL_TO_ARMS, *flags, entities);
                out.point(*x, *z);
            }
            Command::LockGate { lock, entities } => {
                out.0.extend_from_slice(&[LOCK_GATE, u8::from(*lock)]);
                out.entities(entities);
            }
            Command::Pack { pack, entities } => {
                out.0.extend_from_slice(&[PACK, u8::from(*pack)]);
                out.entities(entities);
            }
            Command::CancelPack { pack, entities } => {
                out.0.extend_from_slice(&[CANCEL_PACK, u8::from(*pack)]);
                out.entities(entities);
            }
            Command::UnloadTemplate {
                all,
                template,
                owner,
                holders,
            } => {
                out.0.extend_from_slice(&[UNLOAD_TEMPLATE, u8::from(*all)]);
                out.text(template);
                out.varint(u64::from(*owner));
                out.entities(holders);
            }
        }
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Command, &'static str> {
        let mut input = ByteReader(bytes);
        let kind = input.u8()?;
        let command = match kind {
            WALK | ATTACK_WALK => {
                let flags = input.u8()?;
                let entities = entities(&mut input)?;
                let (x, z) = point(&mut input)?;
                if kind == WALK {
                    Command::Walk {
                        flags,
                        entities,
                        x,
                        z,
                        formation: input.text()?,
                    }
                } else {
                    Command::AttackWalk {
                        flags,
                        entities,
                        x,
                        z,
                    }
                }
            }
            WALK_CUSTOM | ATTACK_WALK_CUSTOM => {
                let flags = input.u8()?;
                let entities = entities(&mut input)?;
                let reversed = input.u8()? != 0;
                let line = line(&mut input)?;
                if kind == WALK_CUSTOM {
                    Command::WalkCustom {
                        flags,
                        entities,
                        line,
                        reversed,
                    }
                } else {
                    Command::AttackWalkCustom {
                        flags,
                        entities,
                        line,
                        reversed,
                    }
                }
            }
            ATTACK | GATHER | REPAIR | GARRISON | RETURN_RESOURCE | GUARD | OCCUPY_TURRET
            | SETUP_TRADE_ROUTE | CANCEL_TRADE_ROUTE => {
                let flags = input.u8()?;
                let entities = entities(&mut input)?;
                let target = input.varint_u32()?;
                match kind {
                    ATTACK => Command::Attack {
                        flags,
                        entities,
                        target,
                    },
                    GATHER => Command::Gather {
                        flags,
                        entities,
                        target,
                    },
                    REPAIR => Command::Repair {
                        flags,
                        entities,
                        target,
                    },
                    GARRISON => Command::Garrison {
                        flags,
                        entities,
                        target,
                    },
                    GUARD => Command::Guard {
                        flags,
                        entities,
                        target,
                    },
                    OCCUPY_TURRET => Command::OccupyTurret {
                        flags,
                        entities,
                        target,
                    },
                    SETUP_TRADE_ROUTE => Command::SetupTradeRoute {
                        flags,
                        entities,
                        target,
                    },
                    CANCEL_TRADE_ROUTE => Command::CancelTradeRoute {
                        flags,
                        entities,
                        target,
                    },
                    _ => Command::ReturnResource {
                        flags,
                        entities,
                        target,
                    },
                }
            }
            UNLOAD_ALL => Command::UnloadAll {
                holders: entities(&mut input)?,
            },
            CONSTRUCT => {
                let flags = input.u8()?;
                let entities = entities(&mut input)?;
                let template = input.text()?;
                let (x, z) = point(&mut input)?;
                Command::Construct {
                    flags,
                    entities,
                    template,
                    x,
                    z,
                    angle: finite(input.f32()?)?,
                    seed: input.varint_u32()?,
                }
            }
            TRAIN => Command::Train {
                entities: entities(&mut input)?,
                template: input.text()?,
                count: input.varint_u32()?,
            },
            RESEARCH => Command::Research {
                flags: input.u8()?,
                entity: input.varint_u32()?,
                template: input.text()?,
            },
            STOP => Command::Stop {
                flags: input.u8()?,
                entities: entities(&mut input)?,
            },
            DELETE => Command::Delete {
                entities: entities(&mut input)?,
            },
            RALLY => {
                let flags = input.u8()?;
                let entities = entities(&mut input)?;
                let (x, z) = point(&mut input)?;
                Command::Rally {
                    flags,
                    entities,
                    x,
                    z,
                    order: input.text()?,
                    target: input.varint_u32()?,
                    resource_type: input.text()?,
                    resource_template: input.text()?,
                    source: input.varint_u32()?,
                }
            }
            STANCE => Command::Stance {
                entities: entities(&mut input)?,
                name: input.text()?,
            },
            FORMATION => Command::Formation {
                entities: entities(&mut input)?,
                name: input.text()?,
            },
            CONSTRUCT_WALL => {
                let flags = input.u8()?;
                let entities = entities(&mut input)?;
                let tower = input.text()?;
                let count = input.varint()? as usize;
                if count > MAX_PIECES {
                    return Err("too many wall pieces");
                }
                let mut pieces = Vec::with_capacity(count);
                for _ in 0..count {
                    let template = input.text()?;
                    let (x, z) = point(&mut input)?;
                    pieces.push(WallPiece {
                        template,
                        x,
                        z,
                        angle: finite(input.f32()?)?,
                    });
                }
                Command::ConstructWall {
                    flags,
                    entities,
                    tower,
                    pieces,
                    start_snapped: input.varint_u32()?,
                    end_snapped: input.varint_u32()?,
                }
            }
            STOP_PRODUCTION => Command::StopProduction {
                entity: input.varint_u32()?,
                id: input.varint_u32()?,
            },
            AUTOQUEUE => Command::Autoqueue {
                on: input.u8()? != 0,
                entities: entities(&mut input)?,
            },
            UNLOAD_ALL_BY_OWNER => Command::UnloadAllByOwner {
                holders: entities(&mut input)?,
            },
            UNLOAD_TURRETS => Command::UnloadTurrets {
                entities: entities(&mut input)?,
            },
            LEAVE_TURRET => Command::LeaveTurret {
                entities: entities(&mut input)?,
            },
            BACK_TO_WORK => Command::BackToWork {
                entities: entities(&mut input)?,
            },
            REMOVE_GUARD => Command::RemoveGuard {
                entities: entities(&mut input)?,
            },
            ALERT_RAISE => Command::AlertRaise {
                entities: entities(&mut input)?,
            },
            ALERT_END => Command::AlertEnd {
                entities: entities(&mut input)?,
            },
            UPGRADE => Command::Upgrade {
                entities: entities(&mut input)?,
                template: input.text()?,
            },
            CANCEL_UPGRADE => Command::CancelUpgrade {
                entities: entities(&mut input)?,
            },
            BARTER => Command::Barter {
                sell: input.text()?,
                buy: input.text()?,
                amount: input.varint_u32()?,
            },
            SET_TRADING_GOODS => {
                let count = input.varint_u32()? as usize;
                if count > MAX_GOODS {
                    return Err("too many trading goods");
                }
                let goods = (0..count)
                    .map(|_| Ok((input.text()?, input.u8()?)))
                    .collect::<Result<_, &'static str>>()?;
                Command::SetTradingGoods { goods }
            }
            TRIBUTE => Command::Tribute {
                player: input.u8()?,
                resource: input.text()?,
                amount: input.varint_u32()?,
            },
            DIPLOMACY => {
                let player = input.u8()?;
                let stance = input.u8()?;
                if stance > DIPLOMACY_NEUTRAL {
                    return Err("unknown stance");
                }
                Command::Diplomacy { player, stance }
            }
            ATTACK_REQUEST => Command::AttackRequest {
                player: input.u8()?,
            },
            SPY_REQUEST => Command::SpyRequest {
                player: input.u8()?,
            },
            DROPSITE_SHARING => Command::DropsiteSharing {
                shared: input.u8()? != 0,
                entities: entities(&mut input)?,
            },
            PATROL => {
                let flags = input.u8()?;
                let entities = entities(&mut input)?;
                let (x, z) = point(&mut input)?;
                Command::Patrol {
                    flags,
                    entities,
                    x,
                    z,
                    target: input.varint_u32()?,
                    formation: input.text()?,
                }
            }
            CALL_TO_ARMS => {
                let flags = input.u8()?;
                let entities = entities(&mut input)?;
                let (x, z) = point(&mut input)?;
                Command::CallToArms {
                    flags,
                    entities,
                    x,
                    z,
                }
            }
            LOCK_GATE => Command::LockGate {
                lock: input.u8()? != 0,
                entities: entities(&mut input)?,
            },
            PACK => Command::Pack {
                pack: input.u8()? != 0,
                entities: entities(&mut input)?,
            },
            CANCEL_PACK => Command::CancelPack {
                pack: input.u8()? != 0,
                entities: entities(&mut input)?,
            },
            UNLOAD_TEMPLATE => Command::UnloadTemplate {
                all: input.u8()? != 0,
                template: input.text()?,
                owner: input.varint_u32()?,
                holders: entities(&mut input)?,
            },
            _ => return Err("unknown command"),
        };
        if !input.0.is_empty() {
            return Err("trailing bytes after the command");
        }
        Ok(command)
    }
}

struct Out(Vec<u8>);

impl Out {
    fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push(value as u8 | 0x80);
            value >>= 7;
        }
        self.0.push(value as u8);
    }

    fn text(&mut self, text: &str) {
        self.varint(text.len() as u64);
        self.0.extend_from_slice(text.as_bytes());
    }

    fn entities(&mut self, entities: &[u32]) {
        let mut sorted = entities.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        self.varint(sorted.len() as u64);
        let mut previous = 0;
        for id in sorted {
            self.varint(u64::from(id - previous));
            previous = id;
        }
    }

    fn point(&mut self, x: f32, z: f32) {
        self.0.extend_from_slice(&x.to_le_bytes());
        self.0.extend_from_slice(&z.to_le_bytes());
    }

    fn freehand(
        &mut self,
        kind: u8,
        flags: u8,
        entities: &[u32],
        line: &[[f32; 2]],
        reversed: bool,
    ) {
        self.head(kind, flags, entities);
        self.0.push(u8::from(reversed));
        self.varint(line.len() as u64);
        let mut previous = [0i64; 2];
        for point in line {
            for (axis, value) in point.iter().enumerate() {
                let sixteenths = sixteenths(*value);
                let delta = sixteenths - previous[axis];
                self.varint(((delta << 1) ^ (delta >> 63)) as u64);
                previous[axis] = sixteenths;
            }
        }
    }

    fn head(&mut self, kind: u8, flags: u8, entities: &[u32]) {
        self.0.extend_from_slice(&[kind, flags]);
        self.entities(entities);
    }

    fn targeted(&mut self, kind: u8, flags: u8, entities: &[u32], target: u32) {
        self.head(kind, flags, entities);
        self.varint(u64::from(target));
    }
}

/// Far more than a selection holds (300), so a bad count fails early.
const MAX_ENTITIES: usize = 4096;

fn entities(input: &mut ByteReader) -> Result<Vec<u32>, &'static str> {
    let count = input.varint()? as usize;
    if count > MAX_ENTITIES {
        return Err("too many entities");
    }
    let mut ids = Vec::with_capacity(count);
    let mut previous = 0u32;
    for _ in 0..count {
        previous = previous
            .checked_add(input.varint_u32()?)
            .ok_or("entity id out of range")?;
        ids.push(previous);
    }
    Ok(ids)
}

/// The most points a freehand line may have: at the page's 1 m spacing,
/// several drags across the largest map.
pub const MAX_LINE: usize = 16384;

/// Past any map, so the deltas cannot overflow and the sums in
/// `freehand_points` stay exact enough.
const MAX_LINE_SIXTEENTHS: i64 = 1 << 24;

fn sixteenths(value: f32) -> i64 {
    (f64::from(value) * 16.0)
        .round()
        .clamp(-MAX_LINE_SIXTEENTHS as f64, MAX_LINE_SIXTEENTHS as f64) as i64
}

/// `line` as the wire carries it, which the page works from too, so its
/// markers and reversal check see the server's points.
pub fn quantize_line(line: &[[f32; 2]]) -> Vec<[f32; 2]> {
    line.iter()
        .map(|point| point.map(|value| sixteenths(value) as f32 / 16.0))
        .collect()
}

fn line(input: &mut ByteReader) -> Result<Vec<[f32; 2]>, &'static str> {
    let count = input.varint()? as usize;
    if count > MAX_LINE {
        return Err("line too long");
    }
    let mut points = Vec::with_capacity(count);
    let mut previous = [0i64; 2];
    for _ in 0..count {
        let mut point = [0.0; 2];
        for axis in 0..2 {
            let zigzag = input.varint()?;
            let delta = (zigzag >> 1) as i64 ^ -((zigzag & 1) as i64);
            let value = previous[axis]
                .checked_add(delta)
                .filter(|value| value.abs() <= MAX_LINE_SIXTEENTHS)
                .ok_or("line point out of range")?;
            previous[axis] = value;
            point[axis] = value as f32 / 16.0;
        }
        points.push(point);
    }
    Ok(points)
}

/// `count` points spread evenly along `line`, the even distribution of
/// `positionUnitsFreehandSelectionMouseUp`, before its reversal check.
/// Empty for an empty line.
pub fn freehand_points(line: &[[f32; 2]], count: usize) -> Vec<[f32; 2]> {
    let Some(&last) = line.last() else {
        return Vec::new();
    };
    let dist = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let length: f32 = line.windows(2).map(|pair| dist(pair[0], pair[1])).sum();
    let step = length / count.saturating_sub(1).max(1) as f32;
    let mut p0 = line[0];
    let mut out = vec![p0];
    let mut free = -step;
    for pair in line.windows(2) {
        let p1 = pair[1];
        free += dist(pair[0], p1);
        // Bounded by the count as well: a step of 0, or one lost against a
        // long segment's float, would never bring `free` below 0, and the
        // server runs this on what a page sent.
        while free >= 0.0 && out.len() < count {
            let away = [p0[0] - p1[0], p0[1] - p1[1]];
            let len = away[0].hypot(away[1]);
            // Vector2D.normalize leaves a zero vector alone.
            let unit = if len > 0.0 {
                [away[0] / len, away[1] / len]
            } else {
                away
            };
            p0 = [unit[0] * free + p1[0], unit[1] * free + p1[1]];
            out.push(p0);
            free -= step;
        }
    }
    out.truncate(count);
    out.resize(count, last);
    out
}

// The JSON the server builds has no way to say NaN or infinity.
fn finite(value: f32) -> Result<f32, &'static str> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err("position not finite")
    }
}

fn point(input: &mut ByteReader) -> Result<(f32, f32), &'static str> {
    Ok((finite(input.f32()?)?, finite(input.f32()?)?))
}
