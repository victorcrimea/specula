//! Counts how often each entity field changes from one turn to the next, to
//! decide what the view model has to send per turn (init.md 4 and 10.2).

use std::collections::HashMap;

use crate::feed::Entity;

/// 25 turns of 200 ms: a summary about every 5 s of game time.
const WINDOW_TURNS: u32 = 25;

#[derive(Default)]
pub struct Classifier {
    previous: HashMap<u32, Entity>,
    has_previous: bool,
    window: Window,
}

#[derive(Default)]
struct Window {
    first_turn: u32,
    turns: u32,
    entities: u64,
    persisting: u64,
    spawned: u64,
    destroyed: u64,
    moved: u64,
    turned: u64,
    owner: u64,
    template: u64,
}

impl Classifier {
    /// Returns a summary line when a window is complete.
    pub fn observe(&mut self, turn: u32, entities: &[Entity]) -> Option<String> {
        let current: HashMap<u32, Entity> =
            entities.iter().map(|entity| (entity.id, *entity)).collect();
        let window = &mut self.window;
        if window.turns == 0 {
            window.first_turn = turn;
        }
        window.turns += 1;
        window.entities += current.len() as u64;

        // The first turn after joining has nothing to diff against, so every
        // entity would look spawned.
        if self.has_previous {
            for (id, now) in &current {
                let Some(before) = self.previous.get(id) else {
                    window.spawned += 1;
                    continue;
                };
                window.persisting += 1;
                window.moved += u64::from(before.x != now.x || before.z != now.z);
                window.turned += u64::from(before.angle != now.angle);
                window.owner += u64::from(before.owner != now.owner);
                window.template += u64::from(before.template != now.template);
            }
            window.destroyed += self
                .previous
                .keys()
                .filter(|id| !current.contains_key(id))
                .count() as u64;
        }
        self.previous = current;
        self.has_previous = true;

        if window.turns < WINDOW_TURNS {
            return None;
        }
        let window = std::mem::take(&mut self.window);
        Some(window.summary(turn))
    }
}

impl Window {
    fn summary(&self, last_turn: u32) -> String {
        let percent = |count: u64| {
            if self.persisting == 0 {
                0.0
            } else {
                100.0 * count as f64 / self.persisting as f64
            }
        };
        format!(
            "turns {}-{}: {} entities avg, +{} -{}, per entity-turn: moved {:.2}%, turned {:.2}%, owner {:.3}%, template {:.3}%",
            self.first_turn,
            last_turn,
            self.entities / u64::from(self.turns),
            self.spawned,
            self.destroyed,
            percent(self.moved),
            percent(self.turned),
            percent(self.owner),
            percent(self.template),
        )
    }
}
