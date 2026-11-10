//! What the player of `/play` ordered and the stream has not reported yet.
//! An order makes a round trip through the server and an engine turn
//! before its effect is streamed back, so the panels draw a toggle's new
//! state at once, marked as predicted, until the stream agrees or the
//! engine is taken to have refused it. The scene itself stays what the
//! server sent.

use specula_assets::command::Command;

use crate::detail;
use crate::detail::Commands;
use crate::detail::Group;
use crate::panel::entity_groups;
use crate::panel::string;
use crate::stream::Scene;

/// `NULL_FORMATION`, which an entity in no formation is in.
const NULL_FORMATION: &str = "special/formations/null";

/// Long enough for a 200 ms turn and the round trip, short enough that a
/// refused order does not leave a wrong mark for long.
const TIMEOUT_MS: f64 = 3000.0;

/// What an order is expected to set on one entity.
#[derive(Clone, PartialEq)]
enum Value {
    Stance(String),
    Autoqueue(bool),
    Formation(String),
    DropsiteShared(bool),
}

impl Value {
    fn same_kind(&self, other: &Value) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

struct Prediction {
    entity: u32,
    value: Value,
    until: f64,
}

#[derive(Default)]
pub(crate) struct Predictions {
    list: Vec<Prediction>,
}

impl Predictions {
    /// Records what `order` will set; any other command is ignored.
    pub(crate) fn record(&mut self, order: &Command, scene: &Scene, now: f64) {
        let (entities, value) = match order {
            Command::Stance { entities, name } => (entities.clone(), Value::Stance(name.clone())),
            Command::Autoqueue { on, entities } => (entities.clone(), Value::Autoqueue(*on)),
            Command::DropsiteSharing { shared, entities } => {
                (entities.clone(), Value::DropsiteShared(*shared))
            }
            // `GetFormationUnitAIs` leaves out those that cannot use it.
            Command::Formation { entities, name } => (
                entities
                    .iter()
                    .copied()
                    .filter(|&entity| can_use_formation(scene, entity, name))
                    .collect(),
                Value::Formation(name.clone()),
            ),
            _ => return,
        };
        // A later order of the same kind replaces the earlier one.
        self.list.retain(|prediction| {
            !(entities.contains(&prediction.entity) && prediction.value.same_kind(&value))
        });
        self.list.extend(entities.iter().map(|&entity| Prediction {
            entity,
            value: value.clone(),
            until: now + TIMEOUT_MS,
        }));
    }

    pub(crate) fn stance(&self, entity: u32) -> Option<&str> {
        self.list
            .iter()
            .find_map(|prediction| match &prediction.value {
                Value::Stance(name) if prediction.entity == entity => Some(name.as_str()),
                _ => None,
            })
    }

    pub(crate) fn formation(&self, entity: u32) -> Option<&str> {
        self.list
            .iter()
            .find_map(|prediction| match &prediction.value {
                Value::Formation(name) if prediction.entity == entity => Some(name.as_str()),
                _ => None,
            })
    }

    pub(crate) fn autoqueue(&self, entity: u32) -> Option<bool> {
        self.list
            .iter()
            .find_map(|prediction| match prediction.value {
                Value::Autoqueue(on) if prediction.entity == entity => Some(on),
                _ => None,
            })
    }

    pub(crate) fn dropsite_shared(&self, entity: u32) -> Option<bool> {
        self.list
            .iter()
            .find_map(|prediction| match prediction.value {
                Value::DropsiteShared(shared) if prediction.entity == entity => Some(shared),
                _ => None,
            })
    }

    /// Drops the predictions the stream confirmed and those timed out;
    /// true when any was dropped, so the panels redraw. An entity without
    /// the group (gone, or out of the detail stream) waits for the
    /// timeout.
    pub(crate) fn reconcile(&mut self, scene: &Scene, now: f64) -> bool {
        if self.list.is_empty() {
            return false;
        }
        let before = self.list.len();
        self.list.retain(|prediction| {
            if now >= prediction.until {
                return false;
            }
            let Some(dot) = scene.dots.get(&prediction.entity) else {
                return true;
            };
            let groups = entity_groups(scene, prediction.entity, dot.template);
            let confirmed = match &prediction.value {
                Value::Stance(name) => matches!(
                    &groups[detail::UNIT_AI],
                    Some(Group::UnitAi { stance, .. }) if string(scene, *stance) == Some(name.as_str())
                ),
                Value::Autoqueue(on) => matches!(
                    &groups[detail::QUEUE],
                    Some(Group::Queue(queue)) if queue.autoqueue == *on
                ),
                Value::DropsiteShared(shared) => matches!(
                    &groups[detail::COMMANDS],
                    Some(Group::Commands(commands))
                        if (commands.flags & Commands::SHARED != 0) == *shared
                ),
                Value::Formation(name) => matches!(
                    &groups[detail::FORMATIONS],
                    Some(Group::Formations { current, .. })
                        if string(scene, *current).unwrap_or(NULL_FORMATION) == name
                ),
            };
            !confirmed
        });
        self.list.len() != before
    }
}

/// Whether `entity`'s UnitAI lists formation `name`.
fn can_use_formation(scene: &Scene, entity: u32, name: &str) -> bool {
    let Some(dot) = scene.dots.get(&entity) else {
        return false;
    };
    matches!(
        &entity_groups(scene, entity, dot.template)[detail::FORMATIONS],
        Some(Group::Formations { list, .. })
            if list.iter().any(|item| string(scene, *item) == Some(name))
    )
}
