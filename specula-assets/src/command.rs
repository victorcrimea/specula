//! The orders a web player gives, as the page sends them to the server on
//! `/play/command`. The server turns each into the engine's command JSON
//! (`simulation/helpers/Commands.js`) for the player's seat.
//!
//! A message is one command: u8 kind, then its fields. Entity lists are a
//! varint count and the ids sorted, each as a varint delta from the one
//! before; positions are f32 metres; strings are a varint length and
//! UTF-8; `flags` is a u8 of the `QUEUED`, `PUSH_FRONT` and
//! `ALLOW_CAPTURE` bits.
//!
//! A message starting with `QUERY_TAG` is a `Query` instead: a GuiInterface
//! call the session GUI makes while placing a structure, answered on the
//! same socket by a text message `{"id": id, "result": JSON}`.

use crate::bytes::ByteReader;

/// The version of this format, which the server sends as the first byte of
/// its seat message; a page on another version must reload.
pub const COMMAND_VERSION: u8 = 2;

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

/// Above every command kind.
pub const QUERY_TAG: u8 = 0x80;
const QUERY_PREVIEW: u8 = 1;
const QUERY_SNAP: u8 = 2;
const QUERY_WALL: u8 = 3;

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
    /// its target entity, 0 for none.
    Rally {
        flags: u8,
        entities: Vec<u32>,
        x: f32,
        z: f32,
        order: String,
        target: u32,
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

impl Command {
    /// Every entity id the command names, for a server that numbers the
    /// entities its player is shown on its own (a seat's handles). An id
    /// of 0, "none", is left out.
    pub fn entity_ids_mut(&mut self) -> Vec<&mut u32> {
        let mut ids: Vec<&mut u32> = Vec::new();
        match self {
            Command::Walk { entities, .. }
            | Command::AttackWalk { entities, .. }
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
            | Command::AlertEnd { entities } => ids.extend(entities.iter_mut()),
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
            | Command::Rally {
                entities, target, ..
            }
            | Command::Patrol {
                entities, target, ..
            } => {
                ids.extend(entities.iter_mut());
                ids.push(target);
            }
            Command::UnloadAll { holders } | Command::UnloadAllByOwner { holders } => {
                ids.extend(holders.iter_mut())
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
            } => {
                out.head(RALLY, *flags, entities);
                out.point(*x, *z);
                out.text(order);
                out.varint(u64::from(*target));
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
            ATTACK | GATHER | REPAIR | GARRISON | RETURN_RESOURCE | GUARD | OCCUPY_TURRET => {
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
