//! What is selected and hovered, a port of `EntitySelection` of
//! gui/session/selection.js, and the fade of each entity's selection
//! overlay, as `CCmpSelectable` keeps it.
//!
//! `EntityGroups` is left to the details panel, the only thing that reads
//! it.

use std::collections::HashMap;
use std::collections::HashSet;

/// `g_MaxSelectionSize`.
const MAX_SELECTION: usize = 300;
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

/// Looks up an entity, `None` when it does not exist.
pub type Lookup<'a> = &'a dyn Fn(u32) -> Option<Selectee>;

#[derive(Default)]
pub struct Selection {
    /// In the order added, as the engine's `Set`.
    selected: Vec<u32>,
    highlighted: HashSet<u32>,
    /// The overlays that differ from `Overlay::idle`.
    overlays: HashMap<u32, Overlay>,
}

impl Selection {
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

    /// `addList`. A broadcast viewer observes, so the engine's refusal to
    /// add to a lone entity of another player is left out: every entity
    /// belongs to another player. Gaia still stays out of a selection of
    /// several, as the engine's `isUnowned` for an observer. Returns the
    /// first entity added, whose select sound the engine plays.
    pub fn add_list(&mut self, ids: &[u32], look: Lookup) -> Option<u32> {
        let mut added = Vec::new();
        for &id in ids {
            if self.selected.len() >= MAX_SELECTION {
                break;
            }
            if self.selected.contains(&id) {
                continue;
            }
            let Some(entity) = look(id) else {
                continue;
            };
            if entity.owner == 0 && (ids.len() > 1 || !self.selected.is_empty()) {
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
    /// leaves the selection. After a seek this drops what does not exist
    /// at the new time.
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
        self.selected.retain(|id| look(*id).is_some());
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
