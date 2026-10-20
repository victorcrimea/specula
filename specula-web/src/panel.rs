//! The selection details panel, a port of gui/session/selection_details.js
//! (`displaySingle`, `displayMultiple`) and of the selection panel's group
//! buttons (`g_SelectionPanels.Selection` in selection_panels.js) and
//! garrison buttons (`g_SelectionPanels.Garrison`), drawn into the page's
//! `#details`. The engine's tooltips are left out.
//!
//! Each element keeps what was last written to it, so an update that
//! changes nothing touches no DOM.

use std::cell::RefCell;

use wasm_bindgen::JsCast;
use web_sys::Document;
use web_sys::HtmlElement;

use crate::detail;
use crate::detail::Group;
use crate::detail::Groups;
use crate::diplomacy::Perspective;
use crate::pick;
use crate::stream::Scene;
use crate::stream::Stance;
use crate::stream::TemplateDetails;

/// `g_SelectionPanels.Selection.getMaxNumberOfItems`.
const MAX_GROUPS: usize = 16;
/// Its `rowLength`.
const GROUP_ROW: usize = 4;
/// `setPanelObjectPosition`: a 38 px button and a 1 px margin.
const GROUP_STEP: usize = 39;
/// `g_SelectionPanels.Garrison.getMaxNumberOfItems`, in rows of
/// `GROUP_ROW`.
const MAX_GARRISONED: usize = 12;
/// `captureBar[n]` and `captureBarMultiple[n]`, `g_MaxPlayers + 1`.
const CAPTURE_SEGMENTS: usize = 9;
/// `getPlayerColor(id, 128)` and `(id, 160)`, as CSS alphas.
const BAND_ALPHA: f64 = 128.0 / 255.0;
const GROUP_ALPHA: f64 = 160.0 / 255.0;
/// `ResourceSupply`'s `isInfinite`, the hook's template flag.
const INFINITE_SUPPLY: u32 = 1024;
/// `KillBeforeGather` is in the fauna templates only, which all have it.
const KILL_BEFORE_GATHER_CLASS: &str = "Animal";

/// An element and what was last written to it.
struct Out {
    element: HtmlElement,
    text: Option<String>,
    style: Option<String>,
    class: Option<String>,
    src: Option<String>,
    hidden: Option<bool>,
}

impl Out {
    fn new(element: HtmlElement) -> Self {
        Out {
            element,
            text: None,
            style: None,
            class: None,
            src: None,
            hidden: None,
        }
    }

    fn find(document: &Document, id: &str) -> Result<Self, String> {
        Ok(Out::new(crate::element(document, id)?))
    }

    fn text(&mut self, text: &str) {
        if self.text.as_deref() != Some(text) {
            self.element.set_text_content(Some(text));
            self.text = Some(text.to_string());
        }
    }

    fn style(&mut self, style: String) {
        if self.style.as_ref() != Some(&style) {
            let _ = self.element.set_attribute("style", &style);
            self.style = Some(style);
        }
    }

    /// The element's classes beyond those of the page.
    fn class(&mut self, class: String) {
        if self.class.as_ref() != Some(&class) {
            let _ = self.element.set_attribute("class", &class);
            self.class = Some(class);
        }
    }

    fn src(&mut self, src: Option<String>) {
        let src = src.unwrap_or_default();
        if self.src.as_ref() != Some(&src) {
            if src.is_empty() {
                let _ = self.element.remove_attribute("src");
            } else {
                let _ = self.element.set_attribute("src", &src);
            }
            self.src = Some(src);
        }
    }

    fn hidden(&mut self, hidden: bool) {
        if self.hidden != Some(hidden) {
            self.element.set_hidden(hidden);
            self.hidden = Some(hidden);
        }
    }
}

/// A bar's foreground and the capture bar's segments, by player.
struct CaptureBar {
    bar: Out,
    segments: Vec<Out>,
}

impl CaptureBar {
    fn find(document: &Document, bar: &str) -> Result<Self, String> {
        let bar = Out::find(document, bar)?;
        let parent = bar
            .element
            .query_selector(".segments")
            .ok()
            .flatten()
            .ok_or("a capture bar without .segments")?;
        let mut segments = Vec::new();
        for _ in 0..CAPTURE_SEGMENTS {
            let segment: HtmlElement = create(document, "div")?;
            parent
                .append_child(&segment)
                .map_err(|_| "cannot add a capture segment")?;
            segments.push(Out::new(segment));
        }
        Ok(CaptureBar { bar, segments })
    }

    /// `setCaptureBarPart` for each (player, points) in drawing order, as
    /// fractions of `max` laid end to end, across or from the top.
    fn show(&mut self, parts: &[(usize, f64)], max: f64, palette: &[u32], vertical: bool) {
        for (player, segment) in self.segments.iter_mut().enumerate() {
            segment.hidden(!parts.iter().any(|(id, _)| *id == player));
        }
        let mut start = 0.0;
        for &(player, points) in parts {
            let Some(segment) = self.segments.get_mut(player) else {
                continue;
            };
            let length = 100.0 * fraction(points, max);
            let color = css_color(palette.get(player).copied().unwrap_or(0), BAND_ALPHA);
            segment.style(if vertical {
                format!("left: 0; right: 0; top: {start}%; height: {length}%; background: {color}")
            } else {
                format!("top: 0; bottom: 0; left: {start}%; width: {length}%; background: {color}")
            });
            start += length;
        }
    }
}

/// One of the selection panel's group buttons, or a garrison button.
struct GroupButton {
    button: Out,
    owner: Out,
    icon: Out,
    count: Out,
}

impl GroupButton {
    /// The `i`-th of a grid, a `tag` of `class` at `setPanelObjectPosition`.
    fn new(
        document: &Document,
        grid: &HtmlElement,
        tag: &str,
        class: &str,
        i: usize,
    ) -> Result<Self, String> {
        let button: HtmlElement = create(document, tag)?;
        if tag == "button" {
            let _ = button.set_attribute("type", "button");
        }
        let _ = button.set_attribute("class", class);
        let _ = button.set_attribute(
            "style",
            &format!(
                "left: {}px; top: {}px",
                i % GROUP_ROW * GROUP_STEP,
                i / GROUP_ROW * GROUP_STEP
            ),
        );
        let owner: HtmlElement = create(document, "div")?;
        let _ = owner.set_attribute("class", "owner");
        let icon: HtmlElement = create(document, "img")?;
        let _ = icon.set_attribute("alt", "");
        let count: HtmlElement = create(document, "div")?;
        let _ = count.set_attribute("class", "text center sans-bold-stroke-14");
        for child in [&owner, &icon, &count] {
            button
                .append_child(child)
                .map_err(|_| "cannot build a group button")?;
        }
        grid.append_child(&button)
            .map_err(|_| "cannot add a group button")?;
        Ok(GroupButton {
            button: Out::new(button),
            owner: Out::new(owner),
            icon: Out::new(icon),
            count: Out::new(count),
        })
    }

    /// `setupButton`'s owner colour, icon and count, for observers.
    fn show(&mut self, icon: Option<String>, color: u32, count: usize) {
        self.icon.src(icon);
        self.owner
            .style(format!("background: {}", css_color(color, GROUP_ALPHA)));
        self.count.text(&if count > 1 {
            count.to_string()
        } else {
            String::new()
        });
        self.button.hidden(false);
    }
}

/// The `#details` panel. Its buttons ask for camera and selection changes,
/// which the page takes with `take_requests`.
pub struct DetailsPanel {
    root: Out,
    single: Out,
    multiple: Out,
    emblem: Out,
    player_color: Out,
    phase: Out,
    player: Out,
    primary: Out,
    secondary: Out,
    capture_section: Out,
    capture_stats: Out,
    capture: CaptureBar,
    health_section: Out,
    health_stats: Out,
    health_bar: Out,
    resource_section: Out,
    resource_stats: Out,
    resource_bar: Out,
    carrying_text: Out,
    carrying_icon: Out,
    portrait: Out,
    icon: Out,
    experience: Out,
    experience_bar: Out,
    rank: Out,
    groups: Vec<GroupButton>,
    count: Out,
    health_multiple: Out,
    health_multiple_bar: Out,
    capture_multiple: CaptureBar,
    strip_icon: Out,
    strip_name: Out,
    strip_health: Out,
    minimap_background: Out,
    /// `None` on a page from before the garrison panel.
    garrison: Option<GarrisonPanel>,
    /// The entity the portrait follows, and the ids of each group button.
    portrait_id: Option<u32>,
    group_ids: Vec<Vec<u32>>,
    requests: RefCell<Vec<Request>>,
}

/// `unitGarrisonPanel` and `healthGarrison` in the supplemental panel.
struct GarrisonPanel {
    root: Out,
    cells: Vec<GroupButton>,
    health: Out,
    health_bar: Out,
}

/// What a click on the panel asks of the page.
pub enum Request {
    Follow(u32),
    Group { ids: Vec<u32>, remove: bool },
}

/// What one update reads.
pub struct View<'a> {
    pub scene: &'a Scene,
    pub selected: &'a [u32],
    pub palette: &'a [u32],
    pub perspective: Perspective,
    /// Identity's Icon -> where an `<img>` finds its portrait.
    pub portrait: &'a dyn Fn(&str) -> Option<String>,
}

impl DetailsPanel {
    pub fn new(document: &Document) -> Result<Self, String> {
        let find = |id: &str| Out::find(document, id);
        let grid: HtmlElement = crate::element(document, "details-groups")?;
        let groups = (0..MAX_GROUPS)
            .map(|i| GroupButton::new(document, &grid, "button", "group", i))
            .collect::<Result<_, _>>()?;
        let garrison = match crate::element::<HtmlElement>(document, "garrison-cells") {
            Ok(grid) => Some(GarrisonPanel {
                root: find("garrison")?,
                cells: (0..MAX_GARRISONED)
                    .map(|i| GroupButton::new(document, &grid, "div", "cell", i))
                    .collect::<Result<_, _>>()?,
                health: find("garrison-health")?,
                health_bar: find("garrison-health-bar")?,
            }),
            Err(_) => None,
        };
        Ok(DetailsPanel {
            root: find("details")?,
            single: find("details-single")?,
            multiple: find("details-multiple")?,
            emblem: find("details-emblem")?,
            player_color: find("details-player-color")?,
            phase: find("details-phase")?,
            player: find("details-player")?,
            primary: find("details-primary")?,
            secondary: find("details-secondary")?,
            capture_section: find("details-capture-section")?,
            capture_stats: find("details-capture-stats")?,
            capture: CaptureBar::find(document, "details-capture")?,
            health_section: find("details-health-section")?,
            health_stats: find("details-health-stats")?,
            health_bar: find("details-health-bar")?,
            resource_section: find("details-resource-section")?,
            resource_stats: find("details-resource-stats")?,
            resource_bar: find("details-resource-bar")?,
            carrying_text: find("details-carrying-text")?,
            carrying_icon: find("details-carrying-icon")?,
            portrait: find("details-portrait")?,
            icon: find("details-icon")?,
            experience: find("details-experience")?,
            experience_bar: find("details-experience-bar")?,
            rank: find("details-rank")?,
            groups,
            count: find("details-count")?,
            health_multiple: find("details-health-multiple")?,
            health_multiple_bar: find("details-health-multiple-bar")?,
            capture_multiple: CaptureBar::find(document, "details-capture-multiple")?,
            strip_icon: find("details-strip-icon")?,
            strip_name: find("details-strip-name")?,
            strip_health: find("details-strip-health")?,
            minimap_background: find("minimap-background")?,
            garrison,
            portrait_id: None,
            group_ids: Vec::new(),
            requests: RefCell::new(Vec::new()),
        })
    }

    /// The portrait button and the group buttons, for the page to wire.
    pub fn portrait_button(&self) -> &HtmlElement {
        &self.portrait.element
    }

    pub fn group_buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.groups.iter().map(|group| &group.button.element)
    }

    /// The portrait was clicked: `setCameraFollow`, when it shows an icon.
    pub fn portrait_clicked(&self) {
        if let Some(id) = self.portrait_id {
            self.requests.borrow_mut().push(Request::Follow(id));
        }
    }

    /// Group button `index` was pressed; `remove` for the right button or
    /// the session.deselectgroup modifier (Ctrl).
    pub fn group_clicked(&self, index: usize, remove: bool) {
        if let Some(ids) = self.group_ids.get(index) {
            self.requests.borrow_mut().push(Request::Group {
                ids: ids.clone(),
                remove,
            });
        }
    }

    pub fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }

    /// `updateSelectionDetails`, and the minimap's
    /// `setCivBackgroundTexture`.
    pub fn update(&mut self, view: &View) {
        let civ = match view.perspective {
            Perspective::Player(id) => view
                .scene
                .players
                .get(id)
                .map_or("gaia", |player| player.civ.as_str()),
            Perspective::Observer => "gaia",
        };
        self.minimap_background.style(format!(
            "background-image: var(--gui-minimap-background-{civ})"
        ));
        // `GetEntityState` is null for what is gone.
        let entities: Vec<u32> = view
            .selected
            .iter()
            .copied()
            .filter(|id| view.scene.dots.contains_key(id))
            .collect();
        self.root.hidden(entities.is_empty());
        self.portrait_id = None;
        match entities.as_slice() {
            [] => {}
            [id] => {
                self.single.hidden(false);
                self.multiple.hidden(true);
                self.show_single(view, *id);
            }
            _ => {
                self.single.hidden(true);
                self.multiple.hidden(false);
                self.show_multiple(view, &entities);
            }
        }
        if let Some(garrison) = &mut self.garrison {
            garrison.show(view, &entities);
        }
    }

    fn show_single(&mut self, view: &View, id: u32) {
        let scene = view.scene;
        let Some(dot) = scene.dots.get(&id) else {
            return;
        };
        let template = template_details(scene, dot.template);
        let groups = entity_groups(scene, id, dot.template);
        let owner = usize::try_from(dot.owner).ok();
        let player = owner.and_then(|owner| scene.players.get(owner));

        // g_SpecificNamesPrimary and g_ShowSecondaryNames, at the default
        // gui.session.howtoshownames 0. `name.specific` falls back to the
        // generic name.
        let generic = template.and_then(|template| string(scene, template.generic_name));
        let specific = template
            .and_then(|template| string(scene, template.specific_name))
            .or(generic);
        let primary = specific.unwrap_or_default();
        let secondary = generic.unwrap_or_default();
        let hide_secondary = secondary.is_empty() || primary == secondary;
        self.primary.text(primary);
        self.primary.class(format!(
            "text center sans-bold-stroke-13 gold{}",
            if hide_secondary { " alone" } else { "" }
        ));
        self.secondary.text(&if hide_secondary {
            String::new()
        } else {
            format!("({secondary})")
        });
        self.secondary.hidden(hide_secondary);
        self.strip_name.text(primary);

        self.player
            .text(player.map_or("", |player| player.name.as_str()));
        let color = owner
            .and_then(|owner| view.palette.get(owner))
            .copied()
            .unwrap_or(0);
        self.player_color
            .style(format!("background: {}", css_color(color, BAND_ALPHA)));
        let civ = player.map_or("gaia", |player| player.civ.as_str());
        if civ == "gaia" {
            self.emblem.style(String::new());
            self.phase.style(String::new());
        } else {
            self.emblem
                .style(format!("background-image: var(--gui-emblem-{civ})"));
            // Phases show to observers and mutual allies.
            let revealed = match view.perspective {
                Perspective::Observer => true,
                Perspective::Player(viewed) => owner
                    .is_some_and(|owner| owner == viewed || mutual_allies(scene, owner, viewed)),
            };
            let phase = if revealed {
                owner
                    .and_then(|owner| scene.economy.get(owner))
                    .and_then(|economy| string(scene, economy.phase))
                    .unwrap_or_default()
            } else {
                "hidden"
            };
            self.phase.style(if phase.is_empty() {
                String::new()
            } else {
                format!("background-image: var(--gui-phase-{phase})")
            });
        }

        let rank = template
            .and_then(|template| string(scene, template.rank))
            .filter(|rank| matches!(*rank, "Basic" | "Advanced" | "Elite"));
        match rank {
            Some(rank) => {
                self.rank.style(format!(
                    "background-image: var(--gui-rank-{})",
                    rank.to_ascii_lowercase()
                ));
                self.rank.hidden(false);
            }
            None => self.rank.hidden(true),
        }

        let icon = template
            .and_then(|template| string(scene, template.icon))
            .and_then(|icon| (view.portrait)(icon));
        if icon.is_some() {
            self.portrait_id = Some(id);
        }
        self.icon.src(icon.clone());
        self.strip_icon.src(icon);

        // Before the detail stream has the entity, stream 1's fraction
        // still fills the bar, without its numbers.
        let health = match &groups[detail::HEALTH] {
            Some(Group::Health { hitpoints, max }) if *hitpoints > 0 => Some((
                f64::from(*hitpoints),
                f64::from(*max),
                format!("{hitpoints} / {max}"),
            )),
            Some(_) => None,
            None if dot.hitpoints > 0 => Some((f64::from(dot.hitpoints), 255.0, String::new())),
            None => None,
        };
        let supply = match &groups[detail::SUPPLY] {
            Some(Group::Supply { amount, max }) => {
                let infinite =
                    template.is_some_and(|template| template.flags & INFINITE_SUPPLY != 0);
                Some(if infinite {
                    (1.0, "\u{221e}".to_string())
                } else {
                    (
                        fraction(f64::from(*amount), f64::from(*max)),
                        format!("{amount} / {max}"),
                    )
                })
            }
            _ => None,
        };
        let capture = capture_points(scene, id, &groups);

        // The section placeholders, sectionPosTop, ...Middle and ...Bottom.
        let (health_slot, capture_slot, resource_slot) = match (&health, &supply, &capture) {
            (Some(_), Some(_), _) => ("top", "middle", "bottom"),
            (Some(_), None, _) => ("top", "bottom", "middle"),
            (None, Some(_), _) => ("top", "bottom", "top"),
            (None, None, _) => ("top", "top", "top"),
        };
        self.health_section
            .class(format!("box section section-{health_slot}"));
        self.capture_section
            .class(format!("box section section-{capture_slot}"));
        self.resource_section
            .class(format!("box section section-{resource_slot}"));

        self.health_section.hidden(health.is_none());
        if let Some((hitpoints, max, text)) = &health {
            let width = 100.0 * fraction(*hitpoints, *max);
            self.health_bar.style(format!("width: {width}%"));
            self.health_stats.text(text);
            self.strip_health.style(format!("width: {width}%"));
        } else {
            self.strip_health.style("width: 0".to_string());
        }

        self.capture_section.hidden(capture.is_none());
        if let Some((points, max)) = &capture {
            // The owner's points first, on the left.
            let mut parts: Vec<(usize, f64)> = owner
                .and_then(|owner| points.get(owner).map(|points| (owner, *points)))
                .into_iter()
                .collect();
            parts.extend(
                points
                    .iter()
                    .enumerate()
                    .filter(|(player, _)| Some(*player) != owner)
                    .map(|(player, points)| (player, *points)),
            );
            self.capture.show(&parts, *max, view.palette, false);
            // With health and resources shown, the numbers go to the
            // tooltip only.
            let owned = owner
                .and_then(|owner| points.get(owner))
                .copied()
                .unwrap_or(0.0);
            let known = matches!(groups[detail::CAPTURE], Some(Group::Capture { .. }));
            self.capture_stats
                .text(&if (health.is_some() && supply.is_some()) || !known {
                    String::new()
                } else {
                    format!("{} / {}", owned.ceil(), max.ceil())
                });
        }

        match &groups[detail::EXPERIENCE] {
            Some(Group::Experience {
                experience,
                required,
            }) => {
                let done = fraction(f64::from(*experience), f64::from((*required).max(1)));
                self.experience_bar
                    .style(format!("top: {}%", 100.0 - 100.0 * done));
                self.experience.hidden(false);
            }
            _ => self.experience.hidden(true),
        }

        self.resource_section.hidden(supply.is_none());
        if let Some((fill, text)) = &supply {
            self.resource_bar.style(format!("width: {}%", 100.0 * fill));
            self.resource_stats.text(text);
        }

        let carrying = self.carrying(view, &groups, template, health.is_some());
        match carrying {
            Some((icon, text)) => {
                self.carrying_icon
                    .style(format!("background-image: var(--gui-{icon})"));
                self.carrying_text.text(&text);
                self.carrying_icon.hidden(false);
                self.carrying_text.hidden(false);
            }
            None => {
                self.carrying_icon.hidden(true);
                self.carrying_text.hidden(true);
            }
        }
    }

    /// The icon key and text of the line under the bars, first that
    /// applies: carried resources, trader gain, foundation builders,
    /// gatherers, repair builders.
    fn carrying(
        &self,
        view: &View,
        groups: &Groups,
        template: Option<&TemplateDetails>,
        has_health: bool,
    ) -> Option<(String, String)> {
        let scene = view.scene;
        if let Some(Group::Carrying(carried)) = &groups[detail::CARRYING]
            && let Some(first) = carried.first()
        {
            let kind = string(scene, first.kind).unwrap_or_default();
            return Some((
                format!("resource-{kind}"),
                format!("{} / {}", first.amount, first.capacity),
            ));
        }
        if let Some(Group::Trader { goods, gain }) = &groups[detail::TRADER]
            && *gain > 0
        {
            let kind = string(scene, *goods).unwrap_or_default();
            return Some((format!("resource-{kind}"), gain.to_string()));
        }
        if let Some(Group::Building(building)) = &groups[detail::FOUNDATION] {
            return Some(("repair".to_string(), builders_text(building, scene.turn)));
        }
        let kill_before_gather = template.is_some_and(|template| {
            pick::has_class(template, &scene.strings, KILL_BEFORE_GATHER_CLASS)
        });
        if groups[detail::SUPPLY].is_some()
            && (!kill_before_gather || !has_health)
            && let Some(Group::Gatherers { count, max }) = &groups[detail::GATHERERS]
        {
            return Some(("repair".to_string(), format!("{count} / {max}")));
        }
        // `needsRepair`: below its max hitpoints.
        if let (Some(Group::Building(building)), Some(Group::Health { hitpoints, max })) =
            (&groups[detail::REPAIR], &groups[detail::HEALTH])
            && hitpoints < max
        {
            return Some(("repair".to_string(), builders_text(building, scene.turn)));
        }
        None
    }

    fn show_multiple(&mut self, view: &View, entities: &[u32]) {
        let scene = view.scene;
        let mut health = 0.0;
        let mut max_health = 0.0;
        let mut capture: Vec<f64> = Vec::new();
        let mut max_capture = 0.0;
        // `displayMultiple` trusts that all have the last one's owner.
        let mut owner = None;
        for &id in entities {
            let Some(dot) = scene.dots.get(&id) else {
                continue;
            };
            owner = usize::try_from(dot.owner).ok();
            let groups = entity_groups(scene, id, dot.template);
            match &groups[detail::HEALTH] {
                Some(Group::Health { hitpoints, max }) if *hitpoints > 0 => {
                    health += f64::from(*hitpoints);
                    max_health += f64::from(*max);
                }
                Some(_) => {}
                None if dot.hitpoints > 0 => {
                    health += f64::from(dot.hitpoints) / 255.0;
                    max_health += 1.0;
                }
                None => {}
            }
            if let Some((points, max)) = capture_points(scene, id, &groups) {
                max_capture += max;
                if capture.len() < points.len() {
                    capture.resize(points.len(), 0.0);
                }
                for (sum, points) in capture.iter_mut().zip(points) {
                    *sum += points;
                }
            }
        }

        self.health_multiple.hidden(health <= 0.0);
        let filled = 100.0 * fraction(health, max_health);
        self.health_multiple_bar
            .style(format!("top: {}%", 100.0 - filled));
        self.strip_health.style(format!("width: {filled}%"));

        self.capture_multiple.bar.hidden(max_capture <= 0.0);
        if max_capture > 0.0 {
            // The others from the top, the owner's last, at the bottom.
            let mut parts: Vec<(usize, f64)> = capture
                .iter()
                .enumerate()
                .filter(|(player, _)| Some(*player) != owner)
                .map(|(player, points)| (player, *points))
                .collect();
            if let Some(owner) = owner {
                parts.push((owner, capture.get(owner).copied().unwrap_or(0.0)));
            }
            self.capture_multiple
                .show(&parts, max_capture, view.palette, true);
        }

        let count = entities.len().to_string();
        self.count.text(&count);
        self.strip_name.text(&count);

        let grouped = crate::selection::groups(entities, |id| {
            let dot = scene.dots.get(&id)?;
            Some((dot.owner, group_name(scene, dot.template)?.to_string()))
        });
        self.group_ids.clear();
        let mut first_icon = None;
        for (index, button) in self.groups.iter_mut().enumerate() {
            let Some((_, ids)) = grouped.get(index) else {
                button.button.hidden(true);
                continue;
            };
            let Some(dot) = ids.first().and_then(|id| scene.dots.get(id)) else {
                button.button.hidden(true);
                continue;
            };
            let icon = template_details(scene, dot.template)
                .and_then(|template| string(scene, template.icon))
                .and_then(|icon| (view.portrait)(icon));
            if index == 0 {
                first_icon.clone_from(&icon);
            }
            // The owner's colour shows to observers only, which a viewer
            // always is.
            button.show(icon, player_color(view.palette, dot.owner), ids.len());
            self.group_ids.push(ids.clone());
        }
        self.strip_icon.src(first_icon);
    }
}

impl GarrisonPanel {
    /// `g_SelectionPanels.Garrison` and `updateGarrisonHealthBar`.
    fn show(&mut self, view: &View, entities: &[u32]) {
        let scene = view.scene;
        // (id, template, owner) of every entity the selection holds.
        let mut held: Vec<(u32, u32, i32)> = Vec::new();
        let mut first_holds = false;
        for (index, &id) in entities.iter().enumerate() {
            let Some(dot) = scene.dots.get(&id) else {
                continue;
            };
            let groups = entity_groups(scene, id, dot.template);
            let Some(Group::Garrison {
                entities: garrisoned,
                ..
            }) = &groups[detail::GARRISON]
            else {
                continue;
            };
            first_holds |= index == 0;
            // A stream without owners: most garrisons are the holder's.
            held.extend(garrisoned.iter().map(|entity| {
                (
                    entity.id,
                    entity.template,
                    entity.owner.unwrap_or(dot.owner),
                )
            }));
        }
        let find = |id: u32| held.iter().find(|entity| entity.0 == id).copied();
        let ids: Vec<u32> = held.iter().map(|entity| entity.0).collect();
        let grouped = crate::selection::groups(&ids, |id| {
            let (_, template, owner) = find(id)?;
            Some((owner, group_name(scene, template)?.to_string()))
        });
        self.root.hidden(grouped.is_empty());
        for (index, cell) in self.cells.iter_mut().enumerate() {
            let Some((_, template, owner)) = grouped
                .get(index)
                .and_then(|(_, ids)| ids.first())
                .and_then(|id| find(*id))
            else {
                cell.button.hidden(true);
                continue;
            };
            let icon = template_details(scene, template)
                .and_then(|template| string(scene, template.icon))
                .and_then(|icon| (view.portrait)(icon));
            cell.show(
                icon,
                player_color(view.palette, owner),
                grouped[index].1.len(),
            );
        }

        let mut health = 0.0;
        let mut max_health = 0.0;
        if first_holds {
            for &(id, template, _) in &held {
                if let Some(Group::Health { hitpoints, max }) =
                    &entity_groups(scene, id, template)[detail::HEALTH]
                {
                    health += f64::from(*hitpoints);
                    max_health += f64::from(*max);
                }
            }
        }
        self.health.hidden(health <= 0.0);
        self.health_bar.style(format!(
            "top: {}%",
            100.0 - 100.0 * fraction(health, max_health)
        ));
    }
}

fn create<T: JsCast>(document: &Document, tag: &str) -> Result<T, String> {
    document
        .create_element(tag)
        .ok()
        .and_then(|element| element.dyn_into().ok())
        .ok_or_else(|| format!("cannot create a <{tag}>"))
}

fn string(scene: &Scene, index: u32) -> Option<&str> {
    let index = (index as usize).checked_sub(1)?;
    scene.strings.get(index).map(String::as_str)
}

/// `EntityGroups`' name: the selection group name, else the template.
fn group_name(scene: &Scene, template: u32) -> Option<&str> {
    template_details(scene, template)
        .and_then(|details| string(scene, details.group_name))
        .filter(|name| !name.is_empty())
        .or_else(|| scene.templates.get(template as usize).map(String::as_str))
}

fn player_color(palette: &[u32], owner: i32) -> u32 {
    usize::try_from(owner)
        .ok()
        .and_then(|owner| palette.get(owner))
        .copied()
        .unwrap_or(0)
}

fn template_details(scene: &Scene, template: u32) -> Option<&TemplateDetails> {
    scene.template_details.get(template as usize)?.as_ref()
}

/// The entity's detail groups, none before the detail stream has a state.
fn entity_groups(scene: &Scene, id: u32, template: u32) -> Groups {
    if scene.details.known {
        scene.details.resolved(id, Some(template))
    } else {
        std::array::from_fn(|_| None)
    }
}

/// Each player's capture points and the max, from the detail stream, else
/// stream 1's shares of a max of 1; `None` without Capturable.
fn capture_points(scene: &Scene, id: u32, groups: &Groups) -> Option<(Vec<f64>, f64)> {
    if let Some(Group::Capture { points, max }) = &groups[detail::CAPTURE] {
        return Some((
            points.iter().map(|points| f64::from(*points)).collect(),
            f64::from(*max),
        ));
    }
    let shares = scene.captures.get(&id)?;
    Some((
        shares
            .iter()
            .map(|share| f64::from(*share) / 255.0)
            .collect(),
        1.0,
    ))
}

fn mutual_allies(scene: &Scene, a: usize, b: usize) -> bool {
    let ally = |from: usize, to: usize| {
        scene
            .players
            .get(from)
            .and_then(|player| player.stances.get(to))
            == Some(&Stance::Ally)
    };
    ally(a, b) && ally(b, a)
}

/// "(builders)\nm:ss", or nothing while nobody builds.
fn builders_text(building: &detail::Building, turn: u32) -> String {
    if building.builders == 0 {
        return String::new();
    }
    // `FormatMillisecondsIntoDateStringGMT` with "m:ss" drops the fraction.
    let seconds = building.seconds_remaining(turn).max(0.0) as u64;
    format!(
        "({})\n{}:{:02}",
        building.builders,
        seconds / 60,
        seconds % 60
    )
}

/// `Math.max(0, Math.min(1, value / max))`.
fn fraction(value: f64, max: f64) -> f64 {
    if max > 0.0 {
        (value / max).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// 0xFFRRGGBB at `alpha`.
fn css_color(color: u32, alpha: f64) -> String {
    format!(
        "rgba({}, {}, {}, {alpha:.3})",
        color >> 16 & 0xFF,
        color >> 8 & 0xFF,
        color & 0xFF
    )
}
