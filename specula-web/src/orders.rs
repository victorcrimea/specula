//! What a right click orders the player's selection to do: a part of
//! `gui/session/unit_actions.js`, the actions a plain right click can pick
//! (`determineAction` without hotkeys), from the most specific down. The
//! engine checks every order again; what this gets wrong only sends an
//! order the units cannot carry out.

use specula_assets::command::ALLOW_CAPTURE;
use specula_assets::command::Command;
use specula_assets::command::QUEUED;

use crate::detail;
use crate::detail::Group;
use crate::panel::entity_groups;
use crate::panel::template_details;
use crate::stream::COMPONENT_ATTACK;
use crate::stream::COMPONENT_BUILDER;
use crate::stream::COMPONENT_CAPTURABLE;
use crate::stream::COMPONENT_FOUNDATION;
use crate::stream::COMPONENT_HEALTH;
use crate::stream::COMPONENT_RALLY_POINT;
use crate::stream::COMPONENT_REPAIRABLE;
use crate::stream::COMPONENT_RESOURCE_DROPSITE;
use crate::stream::COMPONENT_RESOURCE_GATHERER;
use crate::stream::COMPONENT_RESOURCE_SUPPLY;
use crate::stream::COMPONENT_UNIT_AI;
use crate::stream::Scene;
use crate::stream::Stance;

/// Where a right click landed: the entity under the cursor, and the
/// ground point, x and z in metres.
pub struct Target {
    pub entity: Option<u32>,
    pub point: Option<[f32; 2]>,
    pub queued: bool,
}

/// The order for `player`'s entities among `selected`, or none.
pub fn order(scene: &Scene, selected: &[u32], player: i32, target: &Target) -> Option<Command> {
    let own: Vec<u32> = selected
        .iter()
        .copied()
        .filter(|id| scene.dots.get(id).is_some_and(|dot| dot.owner == player))
        .collect();
    if own.is_empty() {
        return None;
    }
    let flags = if target.queued { QUEUED } else { 0 };
    let components = |id: u32| {
        scene
            .dots
            .get(&id)
            .and_then(|dot| template_details(scene, dot.template))
            .map_or(0, |details| details.components)
    };
    let any = |bit: u32| own.iter().any(|&id| components(id) & bit != 0);
    let point = target.point.or_else(|| {
        let dot = scene.dots.get(&target.entity?)?;
        Some([dot.x as f32 / 16.0, dot.z as f32 / 16.0])
    });

    // Structures without UnitAI only set their rally point.
    if !any(COMPONENT_UNIT_AI) {
        if !any(COMPONENT_RALLY_POINT) {
            return None;
        }
        let [x, z] = point?;
        let (order, rally_target) = match target.entity {
            Some(id) => rally_order(scene, player, id, components(id)),
            None => ("walk", 0),
        };
        return Some(Command::Rally {
            flags,
            entities: own,
            x,
            z,
            order: order.to_string(),
            target: rally_target,
        });
    }

    if let Some(id) = target.entity
        && let Some(dot) = scene.dots.get(&id)
    {
        let bits = components(id);
        let relation = relation(scene, player, dot.owner);
        let friendly = matches!(relation, Relation::Own | Relation::Ally);
        let entities = own.clone();
        // "repair": builders on a friendly foundation or damaged building.
        if friendly
            && any(COMPONENT_BUILDER)
            && (bits & COMPONENT_FOUNDATION != 0
                || (bits & COMPONENT_REPAIRABLE != 0 && dot.hitpoints < 255))
        {
            return Some(Command::Repair {
                flags,
                entities,
                target: id,
            });
        }
        // "returnresource": carriers to a dropsite of their own.
        if relation == Relation::Own
            && bits & COMPONENT_RESOURCE_DROPSITE != 0
            && own.iter().any(|&unit| carries(scene, unit))
        {
            return Some(Command::ReturnResource {
                flags,
                entities,
                target: id,
            });
        }
        // "gather": a supply that is gaia's or friendly; hunting too.
        if bits & COMPONENT_RESOURCE_SUPPLY != 0
            && (friendly || relation == Relation::Gaia)
            && any(COMPONENT_RESOURCE_GATHERER)
        {
            return Some(Command::Gather {
                flags,
                entities,
                target: id,
            });
        }
        // "attack", capturing where the target can be captured.
        if relation == Relation::Enemy
            && any(COMPONENT_ATTACK)
            && bits & (COMPONENT_HEALTH | COMPONENT_CAPTURABLE) != 0
        {
            return Some(Command::Attack {
                flags: flags | ALLOW_CAPTURE,
                entities,
                target: id,
            });
        }
    }

    let [x, z] = point?;
    Some(Command::Walk {
        flags,
        entities: own,
        x,
        z,
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Relation {
    Own,
    Ally,
    Neutral,
    Enemy,
    Gaia,
}

/// `player`'s stance towards `owner`.
fn relation(scene: &Scene, player: i32, owner: i32) -> Relation {
    if owner == player {
        return Relation::Own;
    }
    if owner <= 0 {
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

fn carries(scene: &Scene, id: u32) -> bool {
    let Some(dot) = scene.dots.get(&id) else {
        return false;
    };
    match &entity_groups(scene, id, dot.template)[detail::CARRYING] {
        Some(Group::Carrying(carried)) => carried.iter().any(|carried| carried.amount > 0),
        _ => false,
    }
}

/// `set-rallypoint`'s data command for a rally point put on `id`.
fn rally_order(scene: &Scene, player: i32, id: u32, bits: u32) -> (&'static str, u32) {
    let owner = scene.dots.get(&id).map_or(0, |dot| dot.owner);
    let relation = relation(scene, player, owner);
    let friendly = matches!(relation, Relation::Own | Relation::Ally);
    if friendly && bits & COMPONENT_FOUNDATION != 0 {
        ("repair", id)
    } else if bits & COMPONENT_RESOURCE_SUPPLY != 0 && (friendly || relation == Relation::Gaia) {
        ("gather", id)
    } else if relation == Relation::Enemy && bits & (COMPONENT_HEALTH | COMPONENT_CAPTURABLE) != 0 {
        ("attack", id)
    } else {
        ("walk", 0)
    }
}
