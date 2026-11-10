//! What a right click orders the player's selection to do, and the cursor
//! that shows it beforehand: a part of `gui/session/unit_actions.js`, the
//! actions a right click can pick (`determineAction`, with the hotkeys of
//! the actions ported here), in the engine's order of `specificness`, and
//! the left click of a command button's preselected action. The engine
//! checks every order again; what this gets wrong only sends an order the
//! units cannot carry out.

use specula_assets::command::ALLOW_CAPTURE;
use specula_assets::command::Command;
use specula_assets::command::QUEUED;

use crate::detail;
use crate::detail::Commands;
use crate::detail::Group;
use crate::left::matches_class_list;
use crate::panel::entity_groups;
use crate::panel::mutual_allies;
use crate::panel::string;
use crate::panel::template_details;
use crate::stream::COMPONENT_ATTACK;
use crate::stream::COMPONENT_BUILDER;
use crate::stream::COMPONENT_CAPTURABLE;
use crate::stream::COMPONENT_FOUNDATION;
use crate::stream::COMPONENT_GARRISON_HOLDER;
use crate::stream::COMPONENT_GARRISONABLE;
use crate::stream::COMPONENT_GUARD;
use crate::stream::COMPONENT_RALLY_POINT;
use crate::stream::COMPONENT_REPAIRABLE;
use crate::stream::COMPONENT_RESOURCE_DROPSITE;
use crate::stream::COMPONENT_RESOURCE_GATHERER;
use crate::stream::COMPONENT_RESOURCE_SUPPLY;
use crate::stream::COMPONENT_TURRET_HOLDER;
use crate::stream::COMPONENT_TURRETABLE;
use crate::stream::COMPONENT_UNIT_AI;
use crate::stream::Dot;
use crate::stream::Scene;
use crate::stream::Stance;
use crate::stream::TEMPLATE_UNCONTROLLABLE;
use crate::stream::TemplateDetails;

/// Where a right click landed, or where the mouse is: the entity under the
/// cursor, and the ground point, x and z in metres.
pub struct Target {
    pub entity: Option<u32>,
    pub point: Option<[f32; 2]>,
    pub queued: bool,
    /// The `HOTKEY_*` modifiers held.
    pub hotkeys: u8,
    /// A left click, which only a preselected action takes.
    pub left: bool,
}

/// The engine's session.garrison and session.occupyturret (Ctrl),
/// session.guard (G), session.patrol (P) and session.repair (J).
pub const HOTKEY_GARRISON: u8 = 1;
pub const HOTKEY_GUARD: u8 = 2;
pub const HOTKEY_PATROL: u8 = 4;
pub const HOTKEY_REPAIR: u8 = 8;

/// A command button's `preSelectedAction`: the action the next left
/// click on the view takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Preselect {
    CallToArms,
    Garrison,
    OccupyTurret,
    Repair,
    Guard,
    Patrol,
}

/// What a right click would do: the cursor the engine shows for it, the
/// name of a file in `art/textures/cursors/` or empty for the default
/// arrow, the order, none when the click does nothing, and the sound group
/// the order plays with the entity it plays from, the action's
/// `firstAbleEntity`.
pub struct Action {
    pub cursor: String,
    pub command: Option<Command>,
    pub sound: Option<(&'static str, u32)>,
}

/// The action for `player`'s entities among `selected`, none when the
/// engine shows the default cursor and orders nothing. A walk puts them in
/// `formation`, empty for none.
pub fn action(
    scene: &Scene,
    selected: &[u32],
    player: i32,
    target: &Target,
    formation: &str,
) -> Option<Action> {
    let (own, able) = own_and_able(scene, selected, player)?;
    let details = |id: u32| {
        scene
            .dots
            .get(&id)
            .and_then(|dot| template_details(scene, dot.template))
    };
    let components = |id: u32| details(id).map_or(0, |details| details.components);
    if able.is_empty() {
        // The "uncontrollable" action.
        return Some(Action {
            cursor: "cursor-no".to_string(),
            command: None,
            sound: None,
        });
    }
    let flags = if target.queued { QUEUED } else { 0 };
    let entities = || own.clone();
    let has = |id: u32, bit: u32| components(id) & bit != 0;
    let target_dot = target
        .entity
        .and_then(|id| Some((id, scene.dots.get(&id)?)));
    let unit_ai = able.iter().any(|&id| has(id, COMPONENT_UNIT_AI));
    let actions = Actions {
        scene,
        own: &own,
        able: &able,
        player,
        target,
        target_dot,
        flags,
        formation,
    };

    // `hotkeyActionCheck`, before the plain actions.
    let held = |hotkey: u8| target.hotkeys & hotkey != 0;
    let by_hotkey = None
        .or_else(|| held(HOTKEY_REPAIR).then(|| actions.repair()).flatten())
        .or_else(|| held(HOTKEY_GARRISON).then(|| actions.garrison()).flatten())
        .or_else(|| {
            held(HOTKEY_GARRISON)
                .then(|| actions.occupy_turret())
                .flatten()
        })
        .or_else(|| held(HOTKEY_PATROL).then(|| actions.patrol()).flatten())
        .or_else(|| held(HOTKEY_GUARD).then(|| actions.guard()).flatten())
        .or_else(|| held(HOTKEY_GUARD).then(|| actions.remove_guard()).flatten());
    if by_hotkey.is_some() {
        return by_hotkey;
    }

    if let Some((id, dot)) = target_dot {
        let target_details = template_details(scene, dot.template);
        let bits = target_details.map_or(0, |details| details.components);
        let relation = relation(scene, player, dot.owner);

        // "gather"
        if bits & COMPONENT_RESOURCE_SUPPLY != 0
            && let Some((gatherer, resource)) = able.iter().find_map(|&unit| {
                let resource = gathered(
                    scene,
                    details(unit)?,
                    target_details?,
                    has(unit, COMPONENT_RESOURCE_GATHERER),
                )?;
                Some((unit, resource))
            })
        {
            return Some(Action {
                cursor: if resource.is_empty() {
                    String::new()
                } else {
                    format!("action-gather-{resource}")
                },
                command: Some(Command::Gather {
                    flags,
                    entities: entities(),
                    target: id,
                }),
                sound: Some(("order_gather", gatherer)),
            });
        }
        // "returnresource"; shared dropsites are not considered.
        if relation == Relation::Own
            && bits & COMPONENT_RESOURCE_DROPSITE != 0
            && let Some((carrier, carried)) = able.iter().find_map(|&unit| {
                let carried = carried(scene, unit)?;
                let types = target_details?.dropsite_types;
                (types == 0
                    || string(scene, types)
                        .is_some_and(|types| types.split_whitespace().any(|kind| kind == carried)))
                .then_some((unit, carried))
            })
        {
            return Some(Action {
                cursor: format!("action-return-{carried}"),
                command: Some(Command::ReturnResource {
                    flags,
                    entities: entities(),
                    target: id,
                }),
                sound: Some(("order_gather", carrier)),
            });
        }
    }

    // "set-rallypoint": structures only, as commands to units take
    // precedence.
    if !unit_ai
        && able.iter().any(|&id| has(id, COMPONENT_RALLY_POINT))
        && let Some(action) = rally(scene, &own, player, target, target_dot, flags)
    {
        return Some(action);
    }

    if let Some((id, dot)) = target_dot {
        let target_details = template_details(scene, dot.template);
        let bits = target_details.map_or(0, |details| details.components);
        let relation = relation(scene, player, dot.owner);
        let classes = target_classes(scene, target_details);
        let attackers = || {
            able.iter()
                .filter(|&&unit| has(unit, COMPONENT_ATTACK))
                .filter_map(|&unit| Some((unit, details(unit)?)))
        };

        // "attack", without capturing.
        if dot.hitpoints > 0
            && let Some((attacker, _)) =
                attackers().find(|&(_, attacker)| can_attack(scene, attacker, relation, &classes))
        {
            return Some(Action {
                cursor: "action-attack".to_string(),
                command: Some(Command::Attack {
                    flags,
                    entities: entities(),
                    target: id,
                }),
                sound: Some(("order_attack", attacker)),
            });
        }
        // "capture"
        if bits & COMPONENT_CAPTURABLE != 0
            && let Some((attacker, _)) =
                attackers().find(|&(_, attacker)| can_capture(scene, attacker, relation, &classes))
        {
            return Some(Action {
                cursor: "action-capture".to_string(),
                command: Some(Command::Attack {
                    flags: flags | ALLOW_CAPTURE,
                    entities: entities(),
                    target: id,
                }),
                sound: Some(("order_attack", attacker)),
            });
        }
        if let Some(action) = actions.repair() {
            return Some(action);
        }
    }

    // "move", which has no cursor of its own.
    let &walker = able.iter().find(|&&id| has(id, COMPONENT_UNIT_AI))?;
    let point = target
        .point
        .or_else(|| target_dot.map(|(_, dot)| position(dot)));
    Some(Action {
        cursor: String::new(),
        command: point.map(|[x, z]| Command::Walk {
            flags,
            entities: entities(),
            x,
            z,
            formation: formation.to_string(),
        }),
        sound: Some(("order_walk", walker)),
    })
}

/// The action a left click takes in `preselect`'s mode, as its
/// `preSelectedActionCheck`: a disabled cursor where it cannot, which a
/// click still ends the mode on. None without any of the player's own
/// selected, where the engine leaves the mode as it is.
pub fn preselected(
    scene: &Scene,
    selected: &[u32],
    player: i32,
    target: &Target,
    formation: &str,
    preselect: Preselect,
) -> Option<Action> {
    let (own, able) = own_and_able(scene, selected, player)?;
    let actions = Actions {
        scene,
        own: &own,
        able: &able,
        player,
        target,
        target_dot: target
            .entity
            .and_then(|id| Some((id, scene.dots.get(&id)?))),
        flags: if target.queued { QUEUED } else { 0 },
        formation,
    };
    let (action, disabled) = match preselect {
        Preselect::CallToArms => (actions.call_to_arms(), ""),
        Preselect::Garrison => (actions.garrison(), "action-garrison-disabled"),
        Preselect::OccupyTurret => (actions.occupy_turret(), "action-occupy-turret-disabled"),
        Preselect::Repair => (actions.repair(), "action-repair-disabled"),
        Preselect::Guard => (actions.guard(), "action-guard-disabled"),
        Preselect::Patrol => (actions.patrol(), ""),
    };
    Some(action.unwrap_or_else(|| Action {
        cursor: disabled.to_string(),
        command: None,
        sound: None,
    }))
}

/// The player's own among `selected`, and of those the ones getActionInfo
/// does not skip as uncontrollable; none without any own.
fn own_and_able(scene: &Scene, selected: &[u32], player: i32) -> Option<(Vec<u32>, Vec<u32>)> {
    let own: Vec<u32> = selected
        .iter()
        .copied()
        .filter(|id| scene.dots.get(id).is_some_and(|dot| dot.owner == player))
        .collect();
    if own.is_empty() {
        return None;
    }
    let able = own
        .iter()
        .copied()
        .filter(|id| {
            scene
                .dots
                .get(id)
                .and_then(|dot| template_details(scene, dot.template))
                .is_none_or(|details| details.flags & TEMPLATE_UNCONTROLLABLE == 0)
        })
        .collect();
    Some((own, able))
}

/// What the actions that both a hotkey and a command button pick read:
/// each one's `actionCheck`, through `getActionInfo`'s first able entity.
struct Actions<'a> {
    scene: &'a Scene,
    own: &'a [u32],
    able: &'a [u32],
    player: i32,
    target: &'a Target,
    target_dot: Option<(u32, &'a Dot)>,
    flags: u8,
    formation: &'a str,
}

impl Actions<'_> {
    fn details(&self, id: u32) -> Option<&TemplateDetails> {
        let dot = self.scene.dots.get(&id)?;
        template_details(self.scene, dot.template)
    }

    fn has(&self, id: u32, bit: u32) -> bool {
        self.details(id)
            .is_some_and(|details| details.components & bit != 0)
    }

    /// UnitAI's and the like, as the hook reports them.
    fn command_flags(&self, id: u32) -> u32 {
        let Some(dot) = self.scene.dots.get(&id) else {
            return 0;
        };
        match &entity_groups(self.scene, id, dot.template)[detail::COMMANDS] {
            Some(Group::Commands(commands)) => commands.flags,
            _ => 0,
        }
    }

    /// The target, its details' component bits and the player's relation
    /// to its owner.
    fn target(&self) -> Option<(u32, &Dot, u32, Relation)> {
        let (id, dot) = self.target_dot?;
        let bits =
            template_details(self.scene, dot.template).map_or(0, |details| details.components);
        Some((id, dot, bits, relation(self.scene, self.player, dot.owner)))
    }

    /// `playerCheck` with "Player" and "MutualAlly".
    fn own_or_mutual_ally(&self, owner: i32) -> bool {
        owner == self.player
            || matches!(
                (usize::try_from(self.player), usize::try_from(owner)),
                (Ok(player), Ok(owner)) if owner > 0 && mutual_allies(self.scene, player, owner)
            )
    }

    /// The ground clicked, else the target's position, as
    /// `GetTerrainAtScreenPoint` always has one.
    fn point(&self) -> Option<[f32; 2]> {
        self.target
            .point
            .or_else(|| self.target_dot.map(|(_, dot)| position(dot)))
    }

    /// "repair": builders on a friendly foundation or damaged building.
    fn repair(&self) -> Option<Action> {
        let (id, dot, bits, relation) = self.target()?;
        if !matches!(relation, Relation::Own | Relation::Ally)
            || (bits & COMPONENT_FOUNDATION == 0
                && (bits & COMPONENT_REPAIRABLE == 0 || dot.hitpoints == u8::MAX))
        {
            return None;
        }
        let &builder = self
            .able
            .iter()
            .find(|&&unit| self.has(unit, COMPONENT_BUILDER))?;
        Some(Action {
            cursor: "action-repair".to_string(),
            command: Some(Command::Repair {
                flags: self.flags,
                entities: self.own.to_vec(),
                target: id,
            }),
            sound: Some((
                if bits & COMPONENT_FOUNDATION != 0 {
                    "order_build"
                } else {
                    "order_repair"
                },
                builder,
            )),
        })
    }

    /// "garrison": into an own or mutually allied holder whose List the
    /// unit matches; any, from a hook that does not send the List.
    fn garrison(&self) -> Option<Action> {
        let (id, dot, bits, _) = self.target()?;
        if bits & COMPONENT_GARRISON_HOLDER == 0 || !self.own_or_mutual_ally(dot.owner) {
            return None;
        }
        let list = self
            .details(id)
            .map_or(0, |details| details.garrison_classes);
        let &unit = self.able.iter().find(|&&unit| {
            self.has(unit, COMPONENT_GARRISONABLE)
                && (list == 0
                    || string(self.scene, list).is_some_and(|list| {
                        matches_class_list(&target_classes(self.scene, self.details(unit)), list)
                    }))
        })?;
        Some(Action {
            cursor: "action-garrison".to_string(),
            command: Some(Command::Garrison {
                flags: self.flags,
                entities: self.own.to_vec(),
                target: id,
            }),
            sound: Some(("order_garrison", unit)),
        })
    }

    /// "occupy-turret": an own or mutually allied holder with a point the
    /// unit may take, full or not.
    fn occupy_turret(&self) -> Option<Action> {
        let (id, dot, bits, _) = self.target()?;
        if bits & COMPONENT_TURRET_HOLDER == 0 || !self.own_or_mutual_ally(dot.owner) {
            return None;
        }
        let points = self.details(id).map_or(0, |details| details.turret_classes);
        let &unit = self.able.iter().find(|&&unit| {
            self.has(unit, COMPONENT_TURRETABLE)
                && (points == 0
                    || string(self.scene, points).is_some_and(|points| {
                        let classes = target_classes(self.scene, self.details(unit));
                        points.split(';').any(|allowed| {
                            allowed.trim().is_empty() || matches_class_list(&classes, allowed)
                        })
                    }))
        })?;
        Some(Action {
            cursor: "action-occupy-turret".to_string(),
            command: Some(Command::OccupyTurret {
                flags: self.flags,
                entities: self.own.to_vec(),
                target: id,
            }),
            sound: Some(("order_garrison", unit)),
        })
    }

    /// "guard": an own or allied entity with Guard, other than the unit.
    fn guard(&self) -> Option<Action> {
        let (id, _, bits, relation) = self.target()?;
        if bits & COMPONENT_GUARD == 0 || !matches!(relation, Relation::Own | Relation::Ally) {
            return None;
        }
        let &unit = self.able.iter().find(|&&unit| {
            unit != id
                && self.has(unit, COMPONENT_UNIT_AI)
                && self.command_flags(unit) & Commands::CAN_GUARD != 0
        })?;
        Some(Action {
            cursor: "action-guard".to_string(),
            command: Some(Command::Guard {
                flags: self.flags,
                entities: self.own.to_vec(),
                target: id,
            }),
            sound: Some(("order_guard", unit)),
        })
    }

    /// "remove-guard", wherever the mouse is.
    fn remove_guard(&self) -> Option<Action> {
        let &unit = self.able.iter().find(|&&unit| {
            self.has(unit, COMPONENT_UNIT_AI) && self.command_flags(unit) & Commands::GUARDING != 0
        })?;
        Some(Action {
            cursor: "action-remove-guard".to_string(),
            command: Some(Command::RemoveGuard {
                entities: self.own.to_vec(),
            }),
            sound: Some(("order_guard", unit)),
        })
    }

    /// "patrol" to the point, in the default formation.
    fn patrol(&self) -> Option<Action> {
        let &unit = self.able.iter().find(|&&unit| {
            self.has(unit, COMPONENT_UNIT_AI)
                && self.command_flags(unit) & Commands::CAN_PATROL != 0
        })?;
        Some(Action {
            cursor: "action-patrol".to_string(),
            command: self.point().map(|[x, z]| Command::Patrol {
                flags: self.flags,
                entities: self.own.to_vec(),
                x,
                z,
                target: self.target.entity.unwrap_or(0),
                formation: self.formation.to_string(),
            }),
            sound: Some(("order_patrol", unit)),
        })
    }

    /// "call-to-arms", which plays no sound.
    fn call_to_arms(&self) -> Option<Action> {
        self.able
            .iter()
            .find(|&&unit| self.has(unit, COMPONENT_UNIT_AI))?;
        Some(Action {
            cursor: "action-attack".to_string(),
            command: self.point().map(|[x, z]| Command::CallToArms {
                flags: self.flags,
                entities: self.own.to_vec(),
                x,
                z,
            }),
            sound: None,
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Relation {
    Own,
    Ally,
    Neutral,
    Enemy,
    Gaia,
    /// No owner, which no player check passes.
    None,
}

impl Relation {
    /// `isEnemy`, which gaia is to every player.
    fn enemy(self) -> bool {
        matches!(self, Relation::Enemy | Relation::Gaia)
    }
}

/// `player`'s stance towards `owner`.
fn relation(scene: &Scene, player: i32, owner: i32) -> Relation {
    if owner == player {
        return Relation::Own;
    }
    if owner < 0 {
        return Relation::None;
    }
    if owner == 0 {
        return Relation::Gaia;
    }
    let stance = usize::try_from(player)
        .ok()
        .and_then(|player| scene.players.get(player))
        .and_then(|player| player.stances.get(owner as usize));
    match stance {
        Some(Stance::Ally) => Relation::Ally,
        Some(Stance::Neutral) => Relation::Neutral,
        _ => Relation::Enemy,
    }
}

fn position(dot: &Dot) -> [f32; 2] {
    [dot.x as f32 / 16.0, dot.z as f32 / 16.0]
}

/// The type the first resource `id` carries, as `resourceCarrying[0]`.
fn carried(scene: &Scene, id: u32) -> Option<&str> {
    let dot = scene.dots.get(&id)?;
    match &entity_groups(scene, id, dot.template)[detail::CARRYING] {
        Some(Group::Carrying(carried)) => carried
            .iter()
            .find(|carried| carried.amount > 0)
            .and_then(|carried| string(scene, carried.kind)),
        _ => None,
    }
}

/// The gather action's `resource`, the specific type the gatherer has a
/// rate for, else the generic one; none when it cannot gather the supply.
/// Empty when the stream does not say (an older hook), which gathers
/// anything with the default cursor.
fn gathered<'a>(
    scene: &'a Scene,
    gatherer: &TemplateDetails,
    supply: &TemplateDetails,
    can_gather: bool,
) -> Option<&'a str> {
    if !can_gather {
        return None;
    }
    if gatherer.gather_rates == 0 || supply.supply_type == 0 {
        return Some("");
    }
    let rates = string(scene, gatherer.gather_rates)?;
    let kind = string(scene, supply.supply_type)?;
    let (generic, specific) = kind.split_once('.').unwrap_or((kind, ""));
    let rate = |key: &str| rates.split_whitespace().any(|rate| rate == key);
    if rate(kind) {
        Some(specific)
    } else if rate(generic) {
        Some(generic)
    } else {
        None
    }
}

/// `GetIdentityClasses`: Classes and VisibleClasses together.
fn target_classes<'a>(scene: &'a Scene, details: Option<&TemplateDetails>) -> Vec<&'a str> {
    let Some(details) = details else {
        return Vec::new();
    };
    [details.classes, details.visible_classes]
        .into_iter()
        .filter_map(|index| string(scene, index))
        .flat_map(str::split_whitespace)
        .collect()
}

/// The attacker's attack types and each one's RestrictedClasses.
fn attack_types<'a>(scene: &'a Scene, attacker: &TemplateDetails) -> Vec<(&'a str, String)> {
    string(scene, attacker.attack_types)
        .unwrap_or_default()
        .split_whitespace()
        .map(|token| {
            let (kind, restricted) = token.split_once(':').unwrap_or((token, ""));
            (kind, restricted.replace(',', " "))
        })
        .collect()
}

/// `Attack.CanAttack(target, ["!Capture"])`, without the height check.
/// An older hook does not name the types: any enemy then.
fn can_attack(
    scene: &Scene,
    attacker: &TemplateDetails,
    relation: Relation,
    classes: &[&str],
) -> bool {
    if attacker.attack_types == 0 {
        return relation.enemy();
    }
    let types = attack_types(scene, attacker);
    if classes.contains(&"Domestic") && types.iter().any(|(kind, _)| *kind == "Slaughter") {
        return true;
    }
    relation.enemy()
        && types.iter().any(|(kind, restricted)| {
            matches!(*kind, "Melee" | "Ranged") && !matches_class_list(classes, restricted)
        })
}

/// `Attack.CanAttack(target, ["Capture"])`, taking an enemy owner for
/// enemy capture points.
fn can_capture(
    scene: &Scene,
    attacker: &TemplateDetails,
    relation: Relation,
    classes: &[&str],
) -> bool {
    if attacker.attack_types == 0 {
        return relation.enemy();
    }
    relation.enemy()
        && attack_types(scene, attacker)
            .iter()
            .any(|(kind, restricted)| {
                *kind == "Capture" && !matches_class_list(classes, restricted)
            })
}

/// "set-rallypoint" without hotkeys: on a resource gather, on a friendly
/// foundation or damaged building repair, on an enemy attack, else walk.
/// Garrisoning, turrets, treasures and trade routes are not ordered, so
/// those show the walk's arrow. None on a selected entity itself, which
/// the engine unsets the rally point on.
fn rally(
    scene: &Scene,
    own: &[u32],
    player: i32,
    target: &Target,
    target_dot: Option<(u32, &Dot)>,
    flags: u8,
) -> Option<Action> {
    let (cursor, order, rally_target, point) = match target_dot {
        Some((id, _)) if own.contains(&id) => return None,
        Some((id, dot)) => {
            let details = template_details(scene, dot.template);
            let bits = details.map_or(0, |details| details.components);
            let relation = relation(scene, player, dot.owner);
            let friendly = matches!(relation, Relation::Own | Relation::Ally);
            let (cursor, order, rally_target) = if bits & COMPONENT_RESOURCE_SUPPLY != 0 {
                let specific = details
                    .and_then(|details| string(scene, details.supply_type))
                    .and_then(|kind| kind.split_once('.'))
                    .map_or("", |(_, specific)| specific);
                let cursor = if specific.is_empty() {
                    String::new()
                } else {
                    format!("action-gather-{specific}")
                };
                (cursor, "gather", id)
            } else if friendly
                && (bits & COMPONENT_FOUNDATION != 0
                    || (bits & COMPONENT_REPAIRABLE != 0 && dot.hitpoints < 255))
            {
                ("action-repair".to_string(), "repair", id)
            } else if relation.enemy() {
                if dot.hitpoints > 0 {
                    ("action-attack".to_string(), "attack", id)
                } else if bits & COMPONENT_CAPTURABLE != 0 {
                    ("action-capture".to_string(), "attack", id)
                } else {
                    return None;
                }
            } else {
                (String::new(), "walk", 0)
            };
            (cursor, order, rally_target, Some(position(dot)))
        }
        None => (String::new(), "walk", 0, target.point),
    };
    Some(Action {
        cursor,
        command: point.map(|[x, z]| Command::Rally {
            flags,
            entities: own.to_vec(),
            x,
            z,
            order: order.to_string(),
            target: rally_target,
        }),
        // unit_actions.js plays nothing on setting a rally point.
        sound: None,
    })
}
