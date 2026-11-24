//! What is selected and hovered, a port of `EntitySelection` of
//! gui/session/selection.js, its `EntityGroups` as the details panel reads
//! it, and the fade of each entity's selection overlay, as
//! `CCmpSelectable` keeps it.

use std::collections::HashMap;
use std::collections::HashSet;

/// `g_MaxSelectionSize` raised from the engine's 300, which is a GUI limit
/// only: the simulation takes any entity list. A player's selection stays
/// bounded so one order cannot cost the engine unbounded path work in a
/// turn; an observer gives no orders and is not limited.
const MAX_SELECTION: usize = 1500;
/// `g_HighlightedAlpha`.
const HIGHLIGHTED_ALPHA: f32 = 0.75;
/// `MIN_ALPHA_ALWAYS_VISIBLE`.
const ALWAYS_VISIBLE_ALPHA: f32 = 0.65;
/// `CCmpSelectable::FADE_DURATION`, in seconds.
const FADE_DURATION: f32 = 0.3;

/// What the selection needs to know of an entity that exists.
#[derive(Clone, Copy)]
pub struct Selectee {
    /// Player index, -1 for none.
    pub owner: i32,
    /// The template's `Selectable/Overlay/AlwaysVisible`.
    pub always_visible: bool,
    /// Visible to the shown perspective.
    pub seen: bool,
}

/// An entity's overlay, faded as `CCmpSelectable` fades it.
#[derive(Clone, Copy)]
pub struct Overlay {
    pub alpha: f32,
    /// An always visible overlay is desaturated while this is false.
    pub selected: bool,
    min: f32,
    baseline: f32,
    delta: f32,
    progress: f32,
}

impl Overlay {
    /// One nothing has selected or hovered: hidden, or faint when always
    /// visible.
    pub fn idle(always_visible: bool) -> Self {
        let min = if always_visible {
            ALWAYS_VISIBLE_ALPHA
        } else {
            0.0
        };
        Overlay {
            alpha: min,
            selected: false,
            min,
            baseline: min,
            delta: 0.0,
            progress: 0.0,
        }
    }

    /// `SetSelectionHighlight`; the colour is the drawer's.
    fn set(&mut self, alpha: f32, selected: bool) {
        self.selected = selected;
        self.baseline = self.alpha;
        self.delta = alpha.max(self.min) - self.baseline;
        self.progress = 0.0;
    }

    /// `MT_Interpolate`, `seconds` of real time later.
    fn tick(&mut self, seconds: f32) {
        if self.delta == 0.0 {
            return;
        }
        self.progress += seconds;
        if self.progress >= FADE_DURATION {
            self.alpha = self.baseline + self.delta;
            self.baseline = self.alpha;
            self.delta = 0.0;
            self.progress = FADE_DURATION;
        } else {
            // Ease::QuartOut.
            let t = self.progress / FADE_DURATION - 1.0;
            self.alpha = -self.delta * (t * t * t * t - 1.0) + self.baseline;
        }
    }

    fn is_idle(&self) -> bool {
        self.delta == 0.0 && self.alpha == self.min && !self.selected
    }
}

/// `EntityGroups.getEntsGrouped` of the selection: (key, ids) by key, ids
/// rising, as the engine's string keys and integer object keys iterate.
/// `key_of` gives an entity's owner and its template's selection group name
/// or else template name, `None` for one that does not exist. The engine
/// keeps the groups as the selection changes; built afresh they are the
/// same, renames and removals included.
pub fn groups(
    selected: &[u32],
    key_of: impl Fn(u32) -> Option<(i32, String)>,
) -> Vec<(String, Vec<u32>)> {
    let mut groups: Vec<(String, Vec<u32>)> = Vec::new();
    for &id in selected {
        let Some((owner, name)) = key_of(id) else {
            continue;
        };
        let key = format!("p{owner}&{name}");
        match groups.iter_mut().find(|(other, _)| *other == key) {
            Some((_, ids)) => ids.push(id),
            None => groups.push((key, vec![id])),
        }
    }
    // `getKeys` sorts the keys as strings, all ASCII here but the names.
    groups.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    for (_, ids) in &mut groups {
        ids.sort_unstable();
    }
    groups
}

/// Looks up an entity, `None` when it does not exist.
pub type Lookup<'a> = &'a dyn Fn(u32) -> Option<Selectee>;

#[derive(Default)]
pub struct Selection {
    /// In the order added, as the engine's `Set`.
    selected: Vec<u32>,
    highlighted: HashSet<u32>,
    /// The overlays that differ from `Overlay::idle`.
    overlays: HashMap<u32, Overlay>,
    /// `g_ViewedPlayer`, `None` for an observer.
    viewed: Option<i32>,
    /// Not `g_IsObserver`: the `/play` player.
    controls: bool,
}

impl Selection {
    pub fn set_viewer(&mut self, viewed: Option<i32>, controls: bool) {
        self.viewed = viewed;
        self.controls = controls;
    }

    /// In the order added.
    pub fn selected(&self) -> &[u32] {
        &self.selected
    }

    pub fn overlay(&self, id: u32, always_visible: bool) -> Overlay {
        self.overlays
            .get(&id)
            .copied()
            .unwrap_or_else(|| Overlay::idle(always_visible))
    }

    /// Whether `id` is selected or highlighted, as the engine's
    /// `RangeOverlayManager` is told to show its range circles, and
    /// `recalculateStatusBarDisplay` its status bars.
    pub fn shows_ranges(&self, id: u32) -> bool {
        self.highlighted.contains(&id) || self.selected.contains(&id)
    }

    fn set_highlight(&mut self, ids: &[u32], alpha: f32, selected: bool, look: Lookup) {
        for &id in ids {
            let Some(entity) = look(id) else {
                continue;
            };
            self.overlays
                .entry(id)
                .or_insert_with(|| Overlay::idle(entity.always_visible))
                .set(alpha, selected);
        }
    }

    /// `addList`. Under a player perspective nothing is added to a lone
    /// entity of another player, and only that player's entities enter a
    /// selection of several. Under Observer the refusal is left out, as
    /// every entity belongs to another player there; gaia still stays out
    /// of a selection of several, as the engine's `isUnowned` for an
    /// observer. Returns the first entity added, whose select sound the
    /// engine plays.
    pub fn add_list(&mut self, ids: &[u32], look: Lookup) -> Option<u32> {
        if let Some(viewed) = self.viewed
            && let [first] = self.selected[..]
            && look(first).is_some_and(|entity| entity.owner != viewed)
        {
            return None;
        }
        let mut added = Vec::new();
        for &id in ids {
            if self.viewed.is_some() && self.selected.len() >= MAX_SELECTION {
                break;
            }
            if self.selected.contains(&id) {
                continue;
            }
            let Some(entity) = look(id) else {
                continue;
            };
            let unowned = match self.viewed {
                Some(viewed) => entity.owner != viewed,
                None => entity.owner == 0,
            };
            if unowned && (ids.len() > 1 || !self.selected.is_empty()) {
                continue;
            }
            added.push(id);
            self.selected.push(id);
        }
        self.set_highlight(&added, 1.0, true, look);
        added.first().copied()
    }

    /// `removeList`.
    pub fn remove_list(&mut self, ids: &[u32], look: Lookup) {
        let mut removed = Vec::new();
        self.selected.retain(|id| {
            let keep = !ids.contains(id);
            if !keep {
                removed.push(*id);
            }
            keep
        });
        self.set_highlight(&removed, 0.0, false, look);
    }

    /// `reset`.
    pub fn reset(&mut self, look: Lookup) {
        let removed = std::mem::take(&mut self.selected);
        self.set_highlight(&removed, 0.0, false, look);
    }

    /// `setHighlightList`: the hovered entity, or what a bandbox would
    /// select.
    pub fn set_highlight_list(&mut self, ids: &[u32], look: Lookup) {
        let highlighted: HashSet<u32> = ids.iter().copied().collect();
        if highlighted == self.highlighted {
            return;
        }
        let removed: Vec<u32> = self
            .highlighted
            .iter()
            .copied()
            .filter(|id| !highlighted.contains(id) && !self.selected.contains(id))
            .collect();
        let added: Vec<u32> = ids
            .iter()
            .copied()
            .filter(|id| !self.highlighted.contains(id) && !self.selected.contains(id))
            .collect();
        self.set_highlight(&removed, 0.0, false, look);
        self.set_highlight(&added, HIGHLIGHTED_ALPHA, true, look);
        self.highlighted = highlighted;
    }

    /// `checkRenamedEntities`, then `update`: a promoted unit or a finished
    /// building stays selected under its new id, and what no longer exists
    /// leaves the selection, as does what the shown perspective no longer
    /// sees (there are no mirages to keep) and, in a selection of several
    /// on `/play`, what changed owner. After a seek this drops what does
    /// not exist at the new time.
    pub fn update(&mut self, renames: &[(u32, u32)], look: Lookup) {
        if renames.iter().any(|(old, _)| self.selected.contains(old)) {
            // `rebuildSelection`, a rename at a time so a chain of them
            // between two frames is followed to its end.
            let mut ids = self.selected.clone();
            for (old, new) in renames {
                for id in &mut ids {
                    if id == old {
                        *id = *new;
                    }
                }
            }
            self.reset(look);
            // `rebuildSelection` adds quietly.
            self.add_list(&ids, look);
        }
        let viewed = self
            .viewed
            .filter(|_| self.controls && self.selected.len() > 1);
        let mut hidden = Vec::new();
        self.selected.retain(|id| match look(*id) {
            None => false,
            Some(entity) if !entity.seen || viewed.is_some_and(|viewed| entity.owner != viewed) => {
                hidden.push(*id);
                false
            }
            Some(_) => true,
        });
        self.set_highlight(&hidden, 0.0, false, look);
        self.highlighted.retain(|id| look(*id).is_some());
        self.overlays.retain(|id, _| look(*id).is_some());
    }

    /// Fades the overlays `seconds` of real time on.
    pub fn tick(&mut self, seconds: f32) {
        self.overlays.retain(|_, overlay| {
            overlay.tick(seconds);
            !overlay.is_idle()
        });
    }
}
