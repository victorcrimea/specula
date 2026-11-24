//! `/play`'s control groups, as vanilla's `EntityGroupsContainer` and its
//! panel. The server owns the groups (`specula-server/src/groups.rs`): this
//! only reads what it pushes, so a recall sends nothing, and sends a
//! group's contents when a save makes it.

use specula_assets::command::GROUP_COUNT;
use specula_assets::command::GroupContents;
use specula_assets::command::MAX_GROUP;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::Document;
use web_sys::HtmlButtonElement;
use web_sys::HtmlElement;

use crate::chat;
use crate::lang;
use crate::panel::group_name;
use crate::panel::string;
use crate::panel::string_index;
use crate::panel::template_data_of;
use crate::panel::template_details;
use crate::selection;
use crate::stream::Scene;
use crate::tips;
use crate::tips::DataFiles;
use crate::tooltip;

pub const COUNT: usize = GROUP_COUNT;

/// input.js `doublePressTime`.
const DOUBLE_PRESS_MS: f64 = 500.0;

pub const MEMBERSHIP_KEY: &str = "specula.controlgroups.membership";

/// The hotkey families: `selection.group.select`, `.save` and `.add`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Select,
    Save,
    Add,
}

/// `performGroup`'s actions.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Select,
    Add,
    Snap,
    Save,
    BreakUp,
}

/// Whether a save takes the entities out of the other groups, the setting
/// `gui.session.disjointcontrolgroups`.
pub fn disjoint() -> bool {
    chat::choice(MEMBERSHIP_KEY, "single", &["single", "multiple"]) == "single"
}

pub struct ControlGroups {
    /// Seat handles, sorted.
    members: [Vec<u32>; COUNT],
    /// The panel needs a redraw.
    dirty: bool,
    /// input.js `prevHotkey` and `doublePressTimer`.
    last_press: Option<(Key, usize, f64)>,
}

impl ControlGroups {
    pub fn new() -> Self {
        ControlGroups {
            members: std::array::from_fn(|_| Vec::new()),
            dirty: true,
            last_press: None,
        }
    }

    /// Applies the server's push.
    pub fn set(&mut self, contents: GroupContents) {
        let Some(members) = self.members.get_mut(usize::from(contents.group)) else {
            return;
        };
        let mut ids = contents.entities;
        ids.sort_unstable();
        ids.dedup();
        if *members != ids {
            *members = ids;
            self.dirty = true;
        }
    }

    /// A save shown at once; the server's push then overwrites it. The
    /// disjoint rule is the server's, so the two agree.
    pub fn save_local(&mut self, group: usize, ids: &[u32], disjoint: bool) {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        ids.truncate(MAX_GROUP);
        if disjoint {
            for (other, members) in self.members.iter_mut().enumerate() {
                if other != group {
                    members.retain(|id| ids.binary_search(id).is_err());
                }
            }
        }
        if let Some(members) = self.members.get_mut(group) {
            *members = ids;
        }
        self.dirty = true;
    }

    pub fn members(&self, group: usize) -> &[u32] {
        self.members.get(group).map_or(&[], Vec::as_slice)
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// input.js `hotkeypress`: a repeated select within 500 ms of the first
    /// press is a snap, a repeated save or add is ignored, and the second
    /// press does not restart the timer.
    pub fn press(&mut self, key: Key, group: usize, now: f64) -> Option<Action> {
        if let Some((last_key, last_group, at)) = self.last_press
            && last_key == key
            && last_group == group
            && now - at < DOUBLE_PRESS_MS
        {
            return (key == Key::Select).then_some(Action::Snap);
        }
        self.last_press = Some((key, group, now));
        Some(match key {
            Key::Select => Action::Select,
            Key::Save => Action::Save,
            Key::Add => Action::Add,
        })
    }
}

/// `unitGroupPanel`.
pub struct Panel {
    buttons: Vec<Button>,
}

struct Button {
    button: HtmlButtonElement,
    icon: HtmlElement,
    /// The `background-image` last set.
    shown: String,
}

const DEFAULT_ICON: &str = "var(--gui-groups)";

impl Panel {
    pub fn new(document: &Document) -> Result<Panel, String> {
        let root: HtmlElement = crate::element(document, "control-groups")?;
        let tooltip_text = lang::translate(
            "Click to select grouped units, double-click to focus the grouped units and right-click to disband the group.",
        );
        let label = lang::translate("Control group %(number)s");
        let mut buttons = Vec::new();
        let found = root
            .query_selector_all(".control-group")
            .map_err(|_| "cannot list the control group buttons".to_string())?;
        for index in 0..found.length() {
            let button: HtmlButtonElement = found
                .item(index)
                .and_then(|node| node.dyn_into().ok())
                .ok_or("a control group button is not a button")?;
            let child = |class: &str| -> Result<HtmlElement, String> {
                button
                    .query_selector(class)
                    .ok()
                    .flatten()
                    .and_then(|element| element.dyn_into().ok())
                    .ok_or_else(|| format!("a control group button has no {class}"))
            };
            let icon = child(".icon")?;
            let text = child(".text")?;
            let number = (index + 1).to_string();
            text.set_text_content(Some(&number));
            let _ = button.set_attribute(tooltip::ATTRIBUTE, &tooltip_text);
            let _ = button.set_attribute(tooltip::STYLE_ATTRIBUTE, "sessionToolTipBottomBold");
            let _ =
                button.set_attribute("aria-label", &lang::format(&label, &[("number", &number)]));
            buttons.push(Button {
                button,
                icon,
                shown: DEFAULT_ICON.to_string(),
            });
        }
        if buttons.len() != COUNT {
            return Err(format!("{} control group buttons", buttons.len()));
        }
        root.set_hidden(false);
        Ok(Panel { buttons })
    }

    pub fn buttons(&self) -> impl Iterator<Item = &HtmlButtonElement> {
        self.buttons.iter().map(|button| &button.button)
    }

    /// `updateGroups`: the slots stay where they are, an empty group's is
    /// hidden, and the icon is the commonest template's, the dearest on a
    /// tie.
    pub fn refresh(
        &mut self,
        groups: &ControlGroups,
        scene: &Scene,
        data: &DataFiles,
        portrait: &dyn Fn(&str) -> Option<String>,
    ) {
        for (index, button) in self.buttons.iter_mut().enumerate() {
            let members = groups.members(index);
            button.button.set_hidden(members.is_empty());
            if members.is_empty() {
                continue;
            }
            let grouped = selection::groups(members, |id| {
                let dot = scene.dots.get(&id)?;
                Some((dot.owner, group_name(scene, dot.template)?.to_string()))
            });
            let mut best: Option<&Vec<u32>> = None;
            for (_, ids) in &grouped {
                best = Some(match best {
                    None => ids,
                    Some(pre) if pre.len() == ids.len() => {
                        if cost_sum(scene, data, pre[0]) > cost_sum(scene, data, ids[0]) {
                            pre
                        } else {
                            ids
                        }
                    }
                    Some(pre) if pre.len() > ids.len() => pre,
                    Some(_) => ids,
                });
            }
            // Members out of the world have no dot, so the icon stays.
            let Some(dot) = best.and_then(|ids| scene.dots.get(&ids[0])) else {
                continue;
            };
            let url = template_details(scene, dot.template)
                .and_then(|template| string(scene, template.icon))
                .and_then(portrait);
            let image =
                url.map_or_else(|| DEFAULT_ICON.to_string(), |url| format!("url(\"{url}\")"));
            if image != button.shown {
                let _ = button.icon.style().set_property("background-image", &image);
                button.shown = image;
            }
        }
    }
}

/// `getCostSum`: what the template's cost adds up to, 0 while its data is
/// not fetched (a later turn draws it again).
fn cost_sum(scene: &Scene, data: &DataFiles, id: u32) -> f64 {
    let Some(dot) = scene.dots.get(&id) else {
        return 0.0;
    };
    let Some(data) = scene
        .templates
        .get(dot.template as usize)
        .and_then(|name| string_index(scene, name))
        .and_then(|name| template_data_of(scene, data, name, dot.owner))
    else {
        return 0.0;
    };
    let cost = tips::field(&data, "cost");
    if !cost.is_object() {
        return 0.0;
    }
    js_sys::Object::keys(cost.unchecked_ref::<js_sys::Object>())
        .iter()
        .filter_map(|key: JsValue| key.as_string())
        .map(|key| tips::number_field(&cost, &key))
        .sum()
}
