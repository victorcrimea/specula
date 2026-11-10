//! What the player of `/play` ordered and the stream has not reported yet.
//! An order makes a round trip through the server and an engine turn
//! before its effect is streamed back, so the panels draw a toggle's new
//! state at once, marked as predicted, until the stream agrees or the
//! engine is taken to have refused it. The scene itself stays what the
//! server sent.

use specula_assets::command::Command;

use crate::detail;
use crate::detail::Group;
use crate::panel::entity_groups;
use crate::panel::string;
use crate::stream::Scene;

/// Long enough for a 200 ms turn and the round trip, short enough that a
/// refused order does not leave a wrong mark for long.
const TIMEOUT_MS: f64 = 3000.0;

/// What an order is expected to set on one entity.
#[derive(Clone, PartialEq)]
enum Value {
    Stance(String),
    Autoqueue(bool),
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
    pub(crate) fn record(&mut self, order: &Command, now: f64) {
        let (entities, value) = match order {
            Command::Stance { entities, name } => (entities, Value::Stance(name.clone())),
            Command::Autoqueue { on, entities } => (entities, Value::Autoqueue(*on)),
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

    pub(crate) fn autoqueue(&self, entity: u32) -> Option<bool> {
        self.list
            .iter()
            .find_map(|prediction| match prediction.value {
                Value::Autoqueue(on) if prediction.entity == entity => Some(on),
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
            };
            !confirmed
        });
        self.list.len() != before
    }
}
