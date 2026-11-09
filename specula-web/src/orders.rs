//! What a right click orders the player's selection to do, and the cursor
//! that shows it beforehand: a part of `gui/session/unit_actions.js`, the
//! actions a plain right click can pick (`determineAction` without
//! hotkeys), in the engine's order of `specificness`. The engine checks
//! every order again; what this gets wrong only sends an order the units
//! cannot carry out.

use specula_assets::command::ALLOW_CAPTURE;
use specula_assets::command::Command;
use specula_assets::command::QUEUED;

use crate::detail;
use crate::detail::Group;
use crate::left::matches_class_list;
use crate::panel::entity_groups;
use crate::panel::string;
use crate::panel::template_details;
use crate::stream::COMPONENT_ATTACK;
use crate::stream::COMPONENT_BUILDER;
use crate::stream::COMPONENT_CAPTURABLE;
use crate::stream::COMPONENT_FOUNDATION;
use crate::stream::COMPONENT_RALLY_POINT;
use crate::stream::COMPONENT_REPAIRABLE;
use crate::stream::COMPONENT_RESOURCE_DROPSITE;
use crate::stream::COMPONENT_RESOURCE_GATHERER;
use crate::stream::COMPONENT_RESOURCE_SUPPLY;
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
}

/// What a right click would do: the cursor the engine shows for it, the
/// name of a file in `art/textures/cursors/` or empty for the default
/// arrow, and the order, none when the click does nothing.
pub struct Action {
    pub cursor: String,
    pub command: Option<Command>,
}

/// The order for `player`'s entities among `selected`, or none.
pub fn order(scene: &Scene, selected: &[u32], player: i32, target: &Target) -> Option<Command> {
    action(scene, selected, player, target)?.command
}

/// The action for `player`'s entities among `selected`, none when the
/// engine shows the default cursor and orders nothing.
pub fn action(scene: &Scene, selected: &[u32], player: i32, target: &Target) -> Option<Action> {
    let own: Vec<u32> = selected
        .iter()
        .copied()
        .filter(|id| scene.dots.get(id).is_some_and(|dot| dot.owner == player))
        .collect();
    if own.is_empty() {
        return None;
    }
    let details = |id: u32| {
        scene
            .dots
            .get(&id)
            .and_then(|dot| template_details(scene, dot.template))
    };
    let components = |id: u32| details(id).map_or(0, |details| details.components);
    // getActionInfo skips what the player cannot control.
    let able: Vec<u32> = own
        .iter()
        .copied()
        .filter(|&id| {
            details(id).is_none_or(|details| details.flags & TEMPLATE_UNCONTROLLABLE == 0)
        })
        .collect();
    if able.is_empty() {
        // The "uncontrollable" action.
        return Some(Action {
            cursor: "cursor-no".to_string(),
            command: None,
        });
    }
    let flags = if target.queued { QUEUED } else { 0 };
    let entities = || own.clone();
    let has = |id: u32, bit: u32| components(id) & bit != 0;
    let target_dot = target
        .entity
        .and_then(|id| Some((id, scene.dots.get(&id)?)));
    let unit_ai = able.iter().any(|&id| has(id, COMPONENT_UNIT_AI));

    if let Some((id, dot)) = target_dot {
        let target_details = template_details(scene, dot.template);
        let bits = target_details.map_or(0, |details| details.components);
        let relation = relation(scene, player, dot.owner);

        // "gather"
        if bits & COMPONENT_RESOURCE_SUPPLY != 0
            && let Some(resource) = able.iter().find_map(|&unit| {
                gathered(
                    scene,
                    details(unit)?,
                    target_details?,
                    has(unit, COMPONENT_RESOURCE_GATHERER),
                )
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
            });
        }
        // "returnresource"; shared dropsites are not considered.
        if relation == Relation::Own
            && bits & COMPONENT_RESOURCE_DROPSITE != 0
            && let Some(carried) = able.iter().find_map(|&unit| {
                let carried = carried(scene, unit)?;
                let types = target_details?.dropsite_types;
                (types == 0
                    || string(scene, types)
                        .is_some_and(|types| types.split_whitespace().any(|kind| kind == carried)))
                .then_some(carried)
            })
        {
            return Some(Action {
                cursor: format!("action-return-{carried}"),
                command: Some(Command::ReturnResource {
                    flags,
                    entities: entities(),
                    target: id,
                }),
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
                .filter_map(|&unit| details(unit))
        };

        // "attack", without capturing.
        if dot.hitpoints > 0
            && attackers().any(|attacker| can_attack(scene, attacker, relation, &classes))
        {
            return Some(Action {
                cursor: "action-attack".to_string(),
                command: Some(Command::Attack {
                    flags,
                    entities: entities(),
                    target: id,
                }),
            });
        }
        // "capture"
        if bits & COMPONENT_CAPTURABLE != 0
            && attackers().any(|attacker| can_capture(scene, attacker, relation, &classes))
        {
            return Some(Action {
                cursor: "action-capture".to_string(),
                command: Some(Command::Attack {
                    flags: flags | ALLOW_CAPTURE,
                    entities: entities(),
                    target: id,
                }),
            });
        }
        // "repair": builders on a friendly foundation or damaged building.
        if matches!(relation, Relation::Own | Relation::Ally)
            && able.iter().any(|&unit| has(unit, COMPONENT_BUILDER))
            && (bits & COMPONENT_FOUNDATION != 0
                || (bits & COMPONENT_REPAIRABLE != 0 && dot.hitpoints < 255))
        {
            return Some(Action {
                cursor: "action-repair".to_string(),
                command: Some(Command::Repair {
                    flags,
                    entities: entities(),
                    target: id,
                }),
            });
        }
    }

    // "move", which has no cursor of its own.
    if !unit_ai {
        return None;
    }
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
        }),
    })
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
    })
}
