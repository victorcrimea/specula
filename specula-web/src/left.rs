//! The stance, alert, formation and barter panels of the supplemental
//! details panel (`g_SelectionPanels.Stance`, `Alert`, `Formation` and
//! `Barter` in selection_panels.js), drawn into the page's `#garrison`
//! frame next to the garrison buttons, in the engine's layout
//! (`selection_panels_left/`). An observer's buttons are all disabled, so
//! only barter's sell buttons answer a press, as the engine's do; the
//! player of `/play` also orders its own units' stances and formations,
//! raises and ends its own alerts and barters at its own markets.
//! A right click on a formation chooses the default one, for anyone.

use std::cell::Cell;
use std::cell::RefCell;

use specula_assets::command::Command;
use wasm_bindgen::JsValue;
use web_sys::Document;
use web_sys::HtmlElement;

use crate::barter::BarterButtons;
use crate::detail;
use crate::detail::DataKey;
use crate::detail::Group;
use crate::detail::Groups;
use crate::lang;
use crate::panel::Out;
use crate::panel::View;
use crate::panel::create;
use crate::panel::entity_groups;
use crate::panel::grid_position;
use crate::panel::string;
use crate::panel::template_details;
use crate::pick;
use crate::stream::COMPONENT_ALERT_RAISER;
use crate::stream::COMPONENT_FOUNDATION;
use crate::stream::COMPONENT_UNIT_AI;
use crate::stream::Scene;
use crate::tips;
use crate::tooltip;

/// `g_SelectionPanels.Stance.getMaxNumberOfItems`.
const MAX_STANCES: usize = 5;
/// `g_SelectionPanels.Alert.getMaxNumberOfItems`.
const MAX_ALERTS: usize = 2;
/// `g_SelectionPanels.Formation.getMaxNumberOfItems` and its `rowLength`.
const MAX_FORMATIONS: usize = 15;
const FORMATION_ROW: usize = 5;
/// The buttons of `barter_panel.xml`.
const MAX_BARTER: usize = 5;
/// `NULL_FORMATION`.
const NULL_FORMATION: &str = "special/formations/null";
/// `gui.session.defaultformation`'s default (config/default.cfg).
const DEFAULT_FORMATION: &str = "special/formations/box";
/// localStorage key of the default formation, as the engine keeps
/// `gui.session.defaultformation` in the user's config.
const DEFAULT_FORMATION_KEY: &str = "specula.formation";
/// The alert tooltips' `["Civilian"]` and `["Trader"]`.
const ALERT_CIVILIAN: &str = "Civilian";
const ALERT_TRADER: &str = "Trader";

/// A selected entity, as the panels read it.
struct Selected {
    id: u32,
    owner: i32,
    template: u32,
    groups: Groups,
}

impl Selected {
    fn has_component(&self, scene: &Scene, bit: u32) -> bool {
        template_details(scene, self.template).is_some_and(|details| details.components & bit != 0)
    }

    fn has_class(&self, scene: &Scene, class: &str) -> bool {
        template_details(scene, self.template)
            .is_some_and(|details| pick::has_class(details, &scene.strings, class))
    }
}

/// An `iconButton` of the panels, with the overlays some draw over it.
pub(crate) struct Tile {
    pub(crate) tile: Out,
    icon: Out,
    /// `corners.png`.
    selection: Out,
    /// `upgrade.png`, the default formation's mark.
    marker: Out,
    /// The red or black colour modifiers of a sprite.
    pub(crate) tint: Out,
    pub(crate) caption: Out,
}

impl Tile {
    pub(crate) fn new(
        document: &Document,
        parent: &HtmlElement,
        position: &str,
    ) -> Result<Self, String> {
        let tile: HtmlElement = create(document, "div")?;
        let _ = tile.set_attribute("class", "item");
        let _ = tile.set_attribute("style", position);
        let _ = tile.set_attribute(tooltip::STYLE_ATTRIBUTE, "sessionToolTipBottomBold");
        let mut children = Vec::new();
        for class in [
            "icon",
            "tint",
            "selection",
            "marker",
            "text caption sans-bold-stroke-14",
        ] {
            let child: HtmlElement = create(document, "div")?;
            let _ = child.set_attribute("class", class);
            tile.append_child(&child)
                .map_err(|_| "cannot build a panel button")?;
            children.push(Out::new(child));
        }
        parent
            .append_child(&tile)
            .map_err(|_| "cannot add a panel button")?;
        let mut tile = Tile {
            tile: Out::new(tile),
            caption: children.pop().ok_or("a panel button without a caption")?,
            marker: children.pop().ok_or("a panel button without a mark")?,
            selection: children.pop().ok_or("a panel button without a frame")?,
            tint: children.pop().ok_or("a panel button without a tint")?,
            icon: children.pop().ok_or("a panel button without an icon")?,
        };
        tile.tile.hidden(true);
        tile.selection.hidden(true);
        tile.marker.hidden(true);
        Ok(tile)
    }

    /// The icon is a texture of `session.css`, named by its custom
    /// property key.
    pub(crate) fn show(&mut self, key: &str, selected: bool) {
        self.icon
            .style(format!("background-image: var(--gui-{key})"));
        self.selection.hidden(!selected);
        self.tile.hidden(false);
    }
}

pub(crate) struct LeftPanel {
    stance_root: Out,
    stances: Vec<Tile>,
    /// What a press of each stance button orders, for the player of
    /// `/play`: `performStance`.
    stance_orders: Vec<Option<Command>>,
    alert_root: Out,
    alerts: Vec<Tile>,
    /// What a press of each alert button orders, for the player of
    /// `/play`: `raiseAlert` and `endOfAlert`.
    alert_orders: Vec<Option<Command>>,
    formation_root: Out,
    formations: Vec<Tile>,
    /// The formation each button shows, for a right click to choose.
    formation_names: Vec<Option<String>>,
    /// What a press of each formation button orders, for the player of
    /// `/play`: `performFormation`.
    formation_orders: Vec<Option<Command>>,
    barter_root: Out,
    barter: BarterButtons,
    /// Whether the page should fill the panel again.
    redraw: Cell<bool>,
    /// `g_AutoFormation.defaultFormation`.
    default_formation: RefCell<String>,
}

impl LeftPanel {
    /// `None` on a page from before these panels.
    pub(crate) fn find(document: &Document) -> Option<Result<Self, String>> {
        let stance = crate::element::<HtmlElement>(document, "stance-cells").ok()?;
        let alert = crate::element::<HtmlElement>(document, "alert-cells").ok()?;
        let formation = crate::element::<HtmlElement>(document, "formation-cells").ok()?;
        let barter = crate::element::<HtmlElement>(document, "barter-cells").ok()?;
        Some(Self::new(document, &stance, &alert, &formation, &barter))
    }

    fn new(
        document: &Document,
        stance: &HtmlElement,
        alert: &HtmlElement,
        formation: &HtmlElement,
        barter: &HtmlElement,
    ) -> Result<Self, String> {
        let row = |parent: &HtmlElement, count: usize, position: &dyn Fn(usize) -> String| {
            (0..count)
                .map(|i| Tile::new(document, parent, &position(i)))
                .collect::<Result<Vec<_>, _>>()
        };
        let stances = row(stance, MAX_STANCES, &|i| grid_position(i, MAX_STANCES))?;
        // `setPanelObjectPosition(button, getMaxNumberOfItems() - i)`.
        let alerts = row(alert, MAX_ALERTS, &|i| {
            grid_position(MAX_ALERTS - i, usize::MAX)
        })?;
        let formations = row(formation, MAX_FORMATIONS, &|i| {
            grid_position(i, FORMATION_ROW)
        })?;
        let barter = BarterButtons::new(document, barter, MAX_BARTER, "barter", &|i| {
            grid_position(i, usize::MAX)
        })?;
        Ok(LeftPanel {
            stance_root: Out::new(crate::element(document, "stance")?),
            stances,
            stance_orders: vec![None; MAX_STANCES],
            alert_root: Out::new(crate::element(document, "alert")?),
            alerts,
            alert_orders: vec![None; MAX_ALERTS],
            formation_root: Out::new(crate::element(document, "formation")?),
            formations,
            formation_names: vec![None; MAX_FORMATIONS],
            formation_orders: vec![None; MAX_FORMATIONS],
            barter_root: Out::new(crate::element(document, "barter")?),
            barter,
            redraw: Cell::new(false),
            default_formation: RefCell::new(stored_default_formation()),
        })
    }

    /// The barter buttons, for the page to wire.
    pub(crate) fn barter(&self) -> &BarterButtons {
        &self.barter
    }

    /// The stance buttons, for the page to wire to `stance_clicked`.
    pub(crate) fn stance_buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.stances.iter().map(|tile| &tile.tile.element)
    }

    /// Stance button `index` was pressed: its order, if it gives one.
    pub(crate) fn stance_clicked(&self, index: usize) -> Option<Command> {
        self.stance_orders.get(index).cloned().flatten()
    }

    /// The alert buttons, for the page to wire to `alert_clicked`.
    pub(crate) fn alert_buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.alerts.iter().map(|tile| &tile.tile.element)
    }

    /// Alert button `index` was pressed: its order, if it gives one.
    pub(crate) fn alert_clicked(&self, index: usize) -> Option<Command> {
        self.alert_orders.get(index).cloned().flatten()
    }

    /// The formation buttons, for the page to wire to `formation_clicked`
    /// and `formation_right_clicked`.
    pub(crate) fn formation_buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.formations.iter().map(|tile| &tile.tile.element)
    }

    /// Formation button `index` was pressed: its order, if it gives one.
    pub(crate) fn formation_clicked(&self, index: usize) -> Option<Command> {
        self.formation_orders.get(index).cloned().flatten()
    }

    /// `g_AutoFormation.setDefault`, which the button's right press calls
    /// whether or not it is enabled.
    pub(crate) fn formation_right_clicked(&self, index: usize) {
        let Some(Some(name)) = self.formation_names.get(index) else {
            return;
        };
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(DEFAULT_FORMATION_KEY, name);
        }
        self.default_formation.replace(name.clone());
        self.redraw.set(true);
    }

    /// `g_AutoFormation.getDefault`: none for the null formation, which
    /// would otherwise take units out of theirs on every walk.
    pub(crate) fn default_formation(&self) -> Option<String> {
        let name = self.default_formation.borrow();
        (name.as_str() != NULL_FORMATION).then(|| name.clone())
    }

    /// Whether a press changed what the panel shows, once.
    pub(crate) fn take_redraw(&self) -> bool {
        // Both are taken, so neither is left for the next frame.
        let barter = self.barter.take_redraw();
        self.redraw.replace(false) || barter
    }

    /// `g_SelectionPanels.Barter`; true when the panel is used, which keeps
    /// the garrison buttons away.
    pub(crate) fn show_barter(&mut self, view: &View, entities: &[u32]) -> bool {
        let selected = selected(view, entities);
        let scene = view.scene;
        let market = selected.iter().any(|entity| {
            entity.has_class(scene, "Barter") && !entity.has_component(scene, COMPONENT_FOUNDATION)
        });
        self.barter_root.hidden(!market);
        if !market {
            return false;
        }
        // `setViewedPlayer(data.player)`: the first selected entity's
        // owner, and no buttons for gaia.
        let player = selected.first().map_or(-1, |entity| entity.owner);
        self.barter
            .update(scene, player, view.player == Some(player), view.mass_barter);
        true
    }

    /// `Alert`, `Formation` and `Stance`; true when any is used.
    /// `garrison_used` keeps the formations away.
    pub(crate) fn show_others(
        &mut self,
        view: &View,
        entities: &[u32],
        garrison_used: bool,
    ) -> bool {
        let selected = selected(view, entities);
        let alert = self.show_alert(view, &selected);
        let formation = !garrison_used && self.show_formation(view, &selected);
        if garrison_used {
            self.formation_root.hidden(true);
        }
        let stance = self.show_stance(view, &selected);
        alert || formation || stance
    }

    fn show_stance(&mut self, view: &View, selected: &[Selected]) -> bool {
        let scene = view.scene;
        let items = stance_items(scene, selected);
        self.stance_root.hidden(items.is_empty());
        // `controlsPlayer(data.player)`, the first selected entity's owner.
        let controls = selected
            .first()
            .is_some_and(|entity| view.player == Some(entity.owner));
        let ids: Vec<u32> = selected.iter().map(|entity| entity.id).collect();
        self.stance_orders.fill(None);
        for (index, tile) in self.stances.iter_mut().enumerate() {
            let Some(&name) = items.get(index) else {
                tile.tile.hidden(true);
                continue;
            };
            // `IsStanceSelected`: some selected entity is in it, its
            // ordered stance standing in for the streamed one.
            let in_stance = |entity: &Selected| match view.predicted.stance(entity.id) {
                Some(predicted) => Some(predicted == name),
                None => match &entity.groups[detail::UNIT_AI] {
                    Some(Group::UnitAi { stance, .. }) => {
                        Some(string(scene, *stance) == Some(name))
                    }
                    _ => None,
                },
            };
            let current = selected
                .iter()
                .any(|entity| in_stance(entity) == Some(true));
            let predicted = current
                && selected
                    .iter()
                    .any(|entity| view.predicted.stance(entity.id) == Some(name));
            tile.show(&format!("stance-{name}"), current);
            tile.tile.class(String::from(if predicted {
                "item predicted"
            } else {
                "item"
            }));
            if controls {
                self.stance_orders[index] = Some(Command::Stance {
                    entities: ids.clone(),
                    name: name.to_string(),
                });
            }
            tile.tile.tooltip(&format!(
                "{}\n{}",
                stance_name(name),
                tooltip::body_font(&stance_tooltip(name))
            ));
        }
        !items.is_empty()
    }

    fn show_alert(&mut self, view: &View, selected: &[Selected]) -> bool {
        let scene = view.scene;
        let raiser = selected
            .iter()
            .any(|entity| entity.has_component(scene, COMPONENT_ALERT_RAISER));
        self.alert_root.hidden(!raiser);
        // The classes every selected entity's raiser reacts to.
        let classes = |class: &str| {
            selected
                .iter()
                .all(|entity| match &entity.groups[detail::COMMANDS] {
                    Some(Group::Commands(commands)) => string(scene, commands.alert_classes)
                        .is_some_and(|list| matches_class_list(&[class], list)),
                    _ => false,
                })
        };
        let kind = if classes(ALERT_CIVILIAN) {
            Some("Civilians")
        } else if classes(ALERT_TRADER) {
            Some("Traders")
        } else {
            None
        };
        self.alert_orders.fill(None);
        // `controlsPlayer(data.player)`, the first selected entity's owner.
        if raiser
            && selected
                .first()
                .is_some_and(|entity| view.player == Some(entity.owner))
        {
            // The engine sends only the selected entities with an
            // `alertRaiser`.
            let ids: Vec<u32> = selected
                .iter()
                .filter(|entity| entity.has_component(scene, COMPONENT_ALERT_RAISER))
                .map(|entity| entity.id)
                .collect();
            self.alert_orders[0] = Some(Command::AlertRaise {
                entities: ids.clone(),
            });
            self.alert_orders[1] = Some(Command::AlertEnd { entities: ids });
        }
        for (index, tile) in self.alerts.iter_mut().enumerate() {
            if !raiser {
                tile.tile.hidden(true);
                continue;
            }
            let (key, title, body) = if index == 0 {
                (
                    "bell-raise",
                    lang::translate("Raise an alert!"),
                    match kind {
                        Some("Civilians") => {
                            lang::translate("Alert nearby Civilians to seek refuge.")
                        }
                        Some(_) => lang::translate("Alert nearby Traders to seek refuge."),
                        None => lang::translate("Alert nearby vulnerable units to seek refuge."),
                    },
                )
            } else {
                (
                    "bell-end",
                    lang::translate("End the alert."),
                    match kind {
                        Some("Civilians") => lang::translate("Unload nearby Civilians."),
                        Some(_) => lang::translate("Unload nearby Traders."),
                        None => lang::translate("Unload nearby vulnerable units."),
                    },
                )
            };
            tile.show(key, false);
            tile.tile
                .tooltip(&format!("{title}\n{}", tooltip::body_font(&body)));
        }
        raiser
    }

    fn show_formation(&mut self, view: &View, selected: &[Selected]) -> bool {
        let items = formation_items(view, selected);
        self.formation_root.hidden(items.is_empty());
        // `controlsPlayer(data.player)`, the first selected entity's owner.
        let controls = selected
            .first()
            .is_some_and(|entity| view.player == Some(entity.owner));
        let ids: Vec<u32> = selected.iter().map(|entity| entity.id).collect();
        self.formation_names.fill(None);
        self.formation_orders.fill(None);
        let default_formation = self.default_formation.borrow();
        for (index, tile) in self.formations.iter_mut().enumerate() {
            let Some(item) = items.get(index) else {
                tile.tile.hidden(true);
                continue;
            };
            self.formation_names[index] = Some(item.name.clone());
            // `enabled = formationOk && controlsPlayer(data.player)`.
            if item.possible && controls {
                self.formation_orders[index] = Some(Command::Formation {
                    entities: ids.clone(),
                    name: item.name.clone(),
                });
            }
            let icon = item
                .info
                .as_ref()
                .map(|info| tips::text_field(info, "icon"))
                .and_then(|icon| {
                    icon.strip_prefix("formations/")
                        .and_then(|icon| icon.strip_suffix(".png"))
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "null".to_string());
            tile.show(&format!("formation-{icon}"), item.selected);
            let is_default = item.name == *default_formation;
            tile.marker.hidden(!is_default);
            tile.tile
                .class(String::from(match (item.possible, item.predicted) {
                    (true, false) => "item",
                    (true, true) => "item predicted",
                    (false, false) => "item grey",
                    (false, true) => "item grey predicted",
                }));
            tile.tint.style(String::new());
            tile.tile.tooltip(&formation_tooltip(item, is_default));
        }
        !items.is_empty()
    }
}

fn selected(view: &View, entities: &[u32]) -> Vec<Selected> {
    entities
        .iter()
        .filter_map(|&id| {
            let dot = view.scene.dots.get(&id)?;
            Some(Selected {
                id,
                owner: dot.owner,
                template: dot.template,
                groups: entity_groups(view.scene, id, dot.template),
            })
        })
        .collect()
}

/// `g_SelectionPanels.Stance.getItems`.
fn stance_items<'a>(scene: &'a Scene, selected: &[Selected]) -> Vec<&'a str> {
    let Some(first) = selected.first() else {
        return Vec::new();
    };
    if selected.iter().any(|entity| {
        !entity.has_component(scene, COMPONENT_UNIT_AI)
            || !entity.has_class(scene, "Unit")
            || entity.has_class(scene, "Animal")
    }) {
        return Vec::new();
    }
    match &first.groups[detail::STANCES] {
        Some(Group::Stances(stances)) => stances
            .iter()
            .filter_map(|stance| string(scene, *stance))
            .collect(),
        _ => Vec::new(),
    }
}

/// `getStanceDisplayName`.
fn stance_name(name: &str) -> String {
    match name {
        "violent" => lang::translate_context("stance", "Violent"),
        "aggressive" => lang::translate_context("stance", "Aggressive"),
        "defensive" => lang::translate_context("stance", "Defensive"),
        "passive" => lang::translate_context("stance", "Passive"),
        "standground" => lang::translate_context("stance", "Standground"),
        other => other.to_string(),
    }
}

/// `getStanceTooltip`.
fn stance_tooltip(name: &str) -> String {
    match name {
        "violent" => lang::translate_context(
            "stance",
            "Attack nearby opponents, focus on attackers and chase while visible",
        ),
        "aggressive" => lang::translate_context("stance", "Attack nearby opponents"),
        "defensive" => lang::translate_context(
            "stance",
            "Attack nearby opponents, chase a short distance and return to the original location",
        ),
        "passive" => lang::translate_context("stance", "Flee if attacked"),
        "standground" => {
            lang::translate_context("stance", "Attack opponents in range, but don't move")
        }
        _ => String::new(),
    }
}

/// `MatchesClassList(classes, list)` for a `list` of tokens, the whole
/// string an OR of its space separated words, each an AND of its
/// `+`-joined classes; a `!` negates a class.
pub(crate) fn matches_class_list(classes: &[&str], list: &str) -> bool {
    list.split_whitespace().any(|group| {
        group.split('+').all(|class| match class.strip_prefix('!') {
            Some(class) => !classes.contains(&class),
            None => classes.contains(&class),
        })
    })
}

/// A formation of the panel.
struct FormationItem {
    name: String,
    /// The formation's info JSON, once it has arrived.
    info: Option<JsValue>,
    /// `canMoveSelectionIntoFormation`.
    possible: bool,
    /// `IsFormationSelected`.
    selected: bool,
    /// Selected because of an order the stream has not reported yet.
    predicted: bool,
}

/// `g_SelectionPanels.Formation.getItems`, with what `setupButton` asks of
/// GuiInterface decided from the streams.
fn formation_items(view: &View, selected: &[Selected]) -> Vec<FormationItem> {
    let scene = view.scene;
    let Some(first) = selected.first() else {
        return Vec::new();
    };
    if selected
        .iter()
        .any(|entity| !entity.has_class(scene, "Unit"))
    {
        return Vec::new();
    }
    let usable = |entity: &Selected, name: &str| match &entity.groups[detail::FORMATIONS] {
        Some(Group::Formations { list, .. }) => {
            list.iter().any(|item| string(scene, *item) == Some(name))
        }
        _ => false,
    };
    if selected.iter().all(|entity| {
        !entity.has_component(scene, COMPONENT_UNIT_AI)
            || !matches!(&entity.groups[detail::FORMATIONS], Some(Group::Formations { list, .. }) if !list.is_empty())
    }) {
        return Vec::new();
    }
    let available = usize::try_from(first.owner)
        .ok()
        .and_then(|owner| scene.details.players.get(owner))
        .map(|player| player.formations.as_slice())
        .unwrap_or_default();
    let mut items = Vec::new();
    for &formation in available {
        let Some(name) = string(scene, formation) else {
            continue;
        };
        if !selected.iter().any(|entity| usable(entity, name)) {
            continue;
        }
        let info = scene
            .details
            .data
            .get(&DataKey::Formation { name: formation })
            .and_then(|hash| view.data.get(*hash));
        // `CanMoveEntsIntoFormation`, which the null formation always
        // passes; an info still on its way passes too.
        let minimum = info
            .as_ref()
            .map_or(0.0, |info| tips::number_field(info, "minCount"));
        let count = selected
            .iter()
            .filter(|entity| usable(entity, name))
            .count() as f64;
        let possible = name == NULL_FORMATION || count >= minimum;
        // A formation of none is the null formation's; an ordered one
        // stands in for the streamed one.
        let current = |entity: &Selected| match view.predicted.formation(entity.id) {
            Some(predicted) => predicted == name,
            None => match &entity.groups[detail::FORMATIONS] {
                Some(Group::Formations { current, .. }) => {
                    string(scene, *current).unwrap_or(NULL_FORMATION) == name
                }
                _ => false,
            },
        };
        let selected_now = selected.iter().any(current);
        items.push(FormationItem {
            name: name.to_string(),
            info,
            possible,
            selected: selected_now,
            predicted: selected_now
                && selected
                    .iter()
                    .any(|entity| view.predicted.formation(entity.id) == Some(name)),
        });
        if items.len() == MAX_FORMATIONS {
            break;
        }
    }
    items
}

/// `AutoFormation`'s constructor reading `gui.session.defaultformation`.
fn stored_default_formation() -> String {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(DEFAULT_FORMATION_KEY).ok().flatten())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| DEFAULT_FORMATION.to_string())
}

/// The tooltip of `g_SelectionPanels.Formation.setupButton`.
fn formation_tooltip(item: &FormationItem, is_default: bool) -> String {
    let Some(info) = &item.info else {
        return String::new();
    };
    let mut tooltip = lang::translate(&tips::text_field(info, "name"));
    let body = tips::text_field(info, "tooltip");
    if !body.is_empty() {
        tooltip.push('\n');
        tooltip.push_str(&tooltip::body_font(&lang::translate(&body)));
    }
    tooltip.push('\n');
    tooltip.push_str(&if item.name == NULL_FORMATION {
        if is_default {
            lang::translate("Default formation is disabled.")
        } else {
            lang::translate("Right-click to disable the default formation feature.")
        }
    } else if is_default {
        lang::translate("This is the default formation, used for movement orders.")
    } else {
        lang::translate("Right-click to set this as the default formation.")
    });
    let disabled = tips::text_field(info, "disabledTooltip");
    if !item.possible && !disabled.is_empty() {
        tooltip.push('\n');
        tooltip.push_str(&tooltip::objection_font(&lang::translate(&disabled)));
    }
    tooltip
}
