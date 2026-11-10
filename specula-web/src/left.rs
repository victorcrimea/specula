//! The stance, alert, formation and barter panels of the supplemental
//! details panel (`g_SelectionPanels.Stance`, `Alert`, `Formation` and
//! `Barter` in selection_panels.js), drawn into the page's `#garrison`
//! frame next to the garrison buttons, in the engine's layout
//! (`selection_panels_left/`). An observer's buttons are all disabled, so
//! only barter's sell buttons answer a press, as the engine's do; the
//! player of `/play` also orders its own units' stances.

use std::cell::Cell;

use specula_assets::command::Command;
use wasm_bindgen::JsValue;
use web_sys::Document;
use web_sys::HtmlElement;

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
use crate::panel::string_index;
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
/// `BarterButton.prototype.BarterResourceSellQuantity`.
const BARTER_QUANTITY: f64 = 100.0;
/// A buy button under its sell button, `BarterButton.xml`.
const BARTER_BUY_TOP: usize = 42;
/// `g_ResourceData.GetBarterableCodes()`, in the order of the resources'
/// data files.
const BARTER_CODES: [&str; 4] = ["food", "wood", "stone", "metal"];
/// `NULL_FORMATION`.
const NULL_FORMATION: &str = "special/formations/null";
/// `gui.session.defaultformation`'s default (config/default.cfg).
const DEFAULT_FORMATION: &str = "special/formations/box";
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
struct Tile {
    tile: Out,
    icon: Out,
    /// `corners.png`.
    selection: Out,
    /// `upgrade.png`, the default formation's mark.
    marker: Out,
    /// The red or black colour modifiers of a sprite.
    tint: Out,
    caption: Out,
}

impl Tile {
    fn new(document: &Document, parent: &HtmlElement, position: &str) -> Result<Self, String> {
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
    fn show(&mut self, key: &str, selected: bool) {
        self.icon
            .style(format!("background-image: var(--gui-{key})"));
        self.selection.hidden(!selected);
        self.tile.hidden(false);
    }
}

/// A sell button, with the buy button under it.
struct BarterItem {
    sell: Tile,
    buy: Tile,
}

pub(crate) struct LeftPanel {
    stance_root: Out,
    stances: Vec<Tile>,
    /// What a press of each stance button orders, for the player of
    /// `/play`: `performStance`.
    stance_orders: Vec<Option<Command>>,
    alert_root: Out,
    alerts: Vec<Tile>,
    formation_root: Out,
    formations: Vec<Tile>,
    barter_root: Out,
    barter: Vec<BarterItem>,
    /// `BarterButtonManager.selectedResource`, an index of `BARTER_CODES`,
    /// and whether the page should fill the panel again.
    selected_resource: Cell<usize>,
    redraw: Cell<bool>,
    default_formation: String,
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
        let mut items = Vec::new();
        for i in 0..MAX_BARTER {
            let item: HtmlElement = create(document, "div")?;
            let _ = item.set_attribute("class", "barter");
            let _ = item.set_attribute("style", &grid_position(i, usize::MAX));
            barter
                .append_child(&item)
                .map_err(|_| "cannot add a barter item")?;
            items.push(BarterItem {
                sell: Tile::new(document, &item, "left: 0; top: 0")?,
                buy: Tile::new(
                    document,
                    &item,
                    &format!("left: 0; top: {BARTER_BUY_TOP}px"),
                )?,
            });
        }
        Ok(LeftPanel {
            stance_root: Out::new(crate::element(document, "stance")?),
            stances,
            stance_orders: vec![None; MAX_STANCES],
            alert_root: Out::new(crate::element(document, "alert")?),
            alerts,
            formation_root: Out::new(crate::element(document, "formation")?),
            formations,
            barter_root: Out::new(crate::element(document, "barter")?),
            barter: items,
            selected_resource: Cell::new(0),
            redraw: Cell::new(false),
            default_formation: DEFAULT_FORMATION.to_string(),
        })
    }

    /// The sell buttons, for the page to wire to `sell_clicked`.
    pub(crate) fn sell_buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.barter.iter().map(|item| &item.sell.tile.element)
    }

    /// The stance buttons, for the page to wire to `stance_clicked`.
    pub(crate) fn stance_buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.stances.iter().map(|tile| &tile.tile.element)
    }

    /// Stance button `index` was pressed: its order, if it gives one.
    pub(crate) fn stance_clicked(&self, index: usize) -> Option<Command> {
        self.stance_orders.get(index).cloned().flatten()
    }

    /// `BarterButtonManager.setSelectedResource`.
    pub(crate) fn sell_clicked(&self, index: usize) {
        if index < BARTER_CODES.len() {
            self.selected_resource.set(index);
            self.redraw.set(true);
        }
    }

    /// Whether a press changed what the panel shows, once.
    pub(crate) fn take_redraw(&self) -> bool {
        self.redraw.replace(false)
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
        let selected_code = BARTER_CODES[self.selected_resource.get().min(BARTER_CODES.len() - 1)];
        let stock = |code: &str| {
            let index = string_index(scene, code)?;
            let economy = usize::try_from(player)
                .ok()
                .and_then(|player| scene.economy.get(player))?;
            economy
                .resources
                .iter()
                .find(|stock| stock.code == index)
                .map(|stock| f64::from(stock.amount))
        };
        let prices = usize::try_from(player)
            .ok()
            .and_then(|player| scene.details.players.get(player))
            .map(|player| player.prices.as_slice())
            .unwrap_or_default();
        let price = |code: &str| {
            let index = string_index(scene, code)?;
            prices
                .iter()
                .find(|(resource, _, _)| *resource == index)
                .map(|(_, buy, sell)| (f64::from(*buy), f64::from(*sell)))
        };
        // `neededResourcesSell` and `neededResourcesBuy`.
        let short = |code: &str| stock(code).is_some_and(|stock| stock < BARTER_QUANTITY);
        let buy_short = short(selected_code);
        for (index, item) in self.barter.iter_mut().enumerate() {
            let Some(&code) = BARTER_CODES.get(index) else {
                item.sell.tile.hidden(true);
                item.buy.tile.hidden(true);
                continue;
            };
            if player < 1 {
                item.sell.tile.hidden(true);
                item.buy.tile.hidden(true);
                continue;
            }
            let is_selected = code == selected_code;
            let name = lang::translate_context(
                "withinSentence",
                tooltip::resource_data(code).map_or(code, |(name, _)| name),
            );
            item.sell.show(&format!("resource-{code}"), is_selected);
            item.sell.tint.style(barter_tint(is_selected, short(code)));
            item.sell.tile.class(String::from(if is_selected {
                "item selected"
            } else {
                "item"
            }));
            item.sell.caption.text(&lang::format(
                &lang::translate_context("sell action", "-%(amount)s"),
                &[("amount", &BARTER_QUANTITY.to_string())],
            ));
            item.sell.tile.tooltip(&lang::format(
                &lang::translate("Sell %(resource)s"),
                &[("resource", &name)],
            ));

            item.buy.show(&format!("resource-{code}"), false);
            item.buy.tint.style(barter_tint(false, buy_short));
            // Math.round of the amount bought.
            let bought = match (price(selected_code), price(code)) {
                (Some((_, sell)), Some((buy, _))) if buy > 0.0 => {
                    (sell / buy * BARTER_QUANTITY + 0.5).floor()
                }
                _ => 0.0,
            };
            item.buy.caption.text(&lang::format(
                &lang::translate_context("buy action", "+%(amount)s"),
                &[("amount", &bought.to_string())],
            ));
            item.buy.tile.tooltip(&lang::format(
                &lang::translate("Buy %(resource)s"),
                &[("resource", &name)],
            ));
            item.buy.tile.hidden(is_selected);
        }
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
        let scene = view.scene;
        let items = formation_items(view, selected);
        self.formation_root.hidden(items.is_empty());
        for (index, tile) in self.formations.iter_mut().enumerate() {
            let Some(item) = items.get(index) else {
                tile.tile.hidden(true);
                continue;
            };
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
            let is_default = item.name == self.default_formation;
            tile.marker.hidden(!is_default);
            tile.tile.class(String::from(if item.possible {
                "item"
            } else {
                "item grey"
            }));
            tile.tint.style(String::new());
            tile.tile.tooltip(&formation_tooltip(item, is_default));
        }
        let _ = scene;
        !items.is_empty()
    }
}

/// `barterButton`'s sprite modifiers: `SelectedModifier`, then
/// `DisabledModifier` over it.
fn barter_tint(selected: bool, short: bool) -> String {
    let mut layers = Vec::new();
    if short {
        layers.push("linear-gradient(rgba(255, 0, 0, 0.314), rgba(255, 0, 0, 0.314))");
    }
    if selected {
        layers.push("linear-gradient(rgba(0, 0, 0, 0.392), rgba(0, 0, 0, 0.392))");
    }
    if layers.is_empty() {
        String::new()
    } else {
        format!("background: {}", layers.join(", "))
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
        // A formation of none is the null formation's.
        let current = |entity: &Selected| match &entity.groups[detail::FORMATIONS] {
            Some(Group::Formations { current, .. }) => {
                string(scene, *current).unwrap_or(NULL_FORMATION) == name
            }
            _ => false,
        };
        items.push(FormationItem {
            name: name.to_string(),
            info,
            possible,
            selected: selected.iter().any(current),
        });
        if items.len() == MAX_FORMATIONS {
            break;
        }
    }
    items
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
