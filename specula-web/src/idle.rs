//! The idle unit hotkeys and the minimap's idle worker button, worked out
//! from the detail stream's UnitAI idle flags as the GuiInterface's
//! `FindIdleUnits` does in the simulation, so a press asks the server
//! nothing.

use crate::POSITION_UNITS_PER_METRE;
use crate::detail;
use crate::detail::Group;
use crate::left::matches_class_list;
use crate::panel::string;
use crate::panel::template_details;
use crate::stream::Dot;
use crate::stream::Scene;

/// `g_WorkerTypes`, the idle worker button's and `selection.idleworker`'s.
pub const WORKERS: &[&str] = &["Civilian", "Trader", "FishingBoat", "Citizen"];
/// `g_MilitaryTypes`, `selection.idlewarrior`'s.
pub const WARRIORS: &[&str] = &["Melee", "Ranged"];
/// `selection.idlebuilder`'s.
pub const BUILDERS: &[&str] = &["Builder"];
/// `selection.idleunit`'s.
pub const UNITS: &[&str] = &["!Domestic+!Formation"];
/// `PlaySoundForPlayer({ "name": "no_idle_unit" })`: the player template's
/// Sound entry of that name.
pub const NO_IDLE_UNIT_SOUND: &str = "interface/alarm/alarm_no_idle_unit.xml";

/// `GetIdentityClasses`: the classes, the visible classes and the rank.
fn classes(scene: &Scene, template: u32) -> Vec<&str> {
    let Some(details) = template_details(scene, template) else {
        return Vec::new();
    };
    [details.classes, details.visible_classes, details.rank]
        .into_iter()
        .filter_map(|index| string(scene, index))
        .flat_map(str::split_whitespace)
        .collect()
}

/// `UnitAI.IsIdle` as the detail stream last had it.
pub fn unit_ai_idle(scene: &Scene, id: u32, dot: &Dot) -> bool {
    scene.details.known
        && scene.details.match_id == scene.match_id
        && matches!(
            scene.details.group(id, Some(dot.template), detail::UNIT_AI),
            Some(Group::UnitAi { idle: true, .. })
        )
}

/// `IdleUnitFilter`: the index of the first of `classes` the idle unit
/// matches. A garrisoned unit has no dot, so only turrets are checked.
fn bucket(scene: &Scene, id: u32, dot: &Dot, classes_list: &[&str]) -> Option<usize> {
    if !unit_ai_idle(scene, id, dot) {
        return None;
    }
    let turreted = matches!(
        scene.details.group(id, Some(dot.template), detail::HOLDER),
        Some(Group::Holder(holder)) if *holder != 0
    );
    if turreted {
        return None;
    }
    let classes = classes(scene, dot.template);
    classes_list
        .iter()
        .position(|list| matches_class_list(&classes, list))
}

/// `FindIdleUnits` for `player`, without its `limit` when `limit` is 0.
/// Ids stand in for the engine's entity ids in the `prev_unit` order.
fn find(
    scene: &Scene,
    player: i32,
    classes: &[&str],
    exclude: &[u32],
    prev_unit: u32,
    limit: usize,
) -> Vec<u32> {
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); classes.len() + 1];
    for (id, dot) in &scene.dots {
        if dot.owner != player || exclude.contains(id) {
            continue;
        }
        let Some(mut bucket) = bucket(scene, *id, dot, classes) else {
            continue;
        };
        // On a resumed search the first bucket's units up to the previous
        // one go last, so the cycle carries on without a pause.
        if bucket == 0 && prev_unit != 0 && *id <= prev_unit {
            bucket = classes.len();
        }
        buckets[bucket].push(*id);
        if limit != 0 && bucket == 0 && buckets[0].len() == limit {
            return std::mem::take(&mut buckets[0]);
        }
    }
    let mut found: Vec<u32> = buckets.into_iter().flatten().collect();
    if limit != 0 {
        found.truncate(limit);
    }
    found
}

/// How many of `player`'s units the idle worker button counts.
pub fn count(scene: &Scene, player: i32, classes: &[&str]) -> usize {
    scene
        .dots
        .iter()
        .filter(|(id, dot)| dot.owner == player && bucket(scene, **id, dot, classes).is_some())
        .count()
}

/// What a press selects.
pub struct Found {
    pub ids: Vec<u32>,
    /// Where the camera moves to, x and z in metres; `None` for an
    /// offscreen press, which selects them all.
    pub focus: Option<[f32; 2]>,
}

/// `findIdleUnit`'s state between presses: `lastIdleUnit`,
/// `currIdleClassIndex` and `lastIdleClasses`.
#[derive(Default)]
pub struct Finder {
    last_unit: u32,
    class_index: usize,
    last_classes: Vec<String>,
}

impl Finder {
    fn reset(&mut self) {
        self.last_unit = 0;
        self.class_index = 0;
        self.last_classes.clear();
    }

    /// `findIdleUnit(classes)`: with `append` (selection.add) the selected
    /// stay and are skipped, with `offscreen` every idle unit is found.
    /// `None` when there is none, for the caller to play the alarm.
    pub fn find(
        &mut self,
        scene: &Scene,
        player: i32,
        classes: &[&str],
        selected: &[u32],
        append: bool,
        offscreen: bool,
    ) -> Option<Found> {
        if offscreen || self.last_classes.iter().ne(classes.iter()) {
            self.reset();
        }
        self.last_classes = classes.iter().map(|class| class.to_string()).collect();
        // The class the last unit was found for first.
        let rotated: Vec<&str> = classes[self.class_index..]
            .iter()
            .chain(&classes[..self.class_index])
            .copied()
            .collect();
        let exclude = if append { selected } else { &[] };
        let ids = if offscreen {
            find(scene, player, &rotated, exclude, 0, 0)
        } else {
            find(scene, player, &rotated, exclude, self.last_unit, 1)
        };
        let Some(&first) = ids.first() else {
            self.reset();
            return None;
        };
        if offscreen {
            return Some(Found { ids, focus: None });
        }
        self.last_unit = first;
        let dot = &scene.dots[&first];
        let classes_of = self::classes(scene, dot.template);
        if let Some(change) = rotated
            .iter()
            .position(|list| matches_class_list(&classes_of, list))
        {
            self.class_index = (self.class_index + change) % classes.len();
        }
        Some(Found {
            ids,
            focus: Some([dot.x, dot.z].map(|value| value as f32 / POSITION_UNITS_PER_METRE)),
        })
    }
}
