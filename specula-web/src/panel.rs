//! The selection details panel, a port of gui/session/selection_details.js
//! (`displaySingle`, `displayMultiple`) and of the selection panel's group
//! buttons (`g_SelectionPanels.Selection` in selection_panels.js), garrison
//! buttons (`g_SelectionPanels.Garrison`), production queue
//! (`g_SelectionPanels.Queue`) and training and research buttons
//! (`g_SelectionPanels.Training` and `Research`), drawn into the page's
//! `#details`, with the engine's tooltips where their data is sent.
//!
//! Each element keeps what was last written to it, so an update that
//! changes nothing touches no DOM.

use std::cell::RefCell;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::Document;
use web_sys::HtmlElement;

use crate::detail;
use crate::detail::DataKey;
use crate::detail::Group;
use crate::detail::Groups;
use crate::detail::ListItem;
use crate::diplomacy::Perspective;
use crate::pick;
use crate::stream::Scene;
use crate::stream::Stance;
use crate::stream::TemplateDetails;
use crate::tips;
use crate::tips::DataFiles;
use crate::tooltip;

/// `g_SelectionPanels.Selection.getMaxNumberOfItems`.
const MAX_GROUPS: usize = 16;
/// Its `rowLength`.
const GROUP_ROW: usize = 4;
/// `setPanelObjectPosition`: a 38 px button and a 1 px margin.
const GROUP_STEP: usize = 39;
/// `g_SelectionPanels.Garrison.getMaxNumberOfItems`, in rows of
/// `GROUP_ROW`.
const MAX_GARRISONED: usize = 12;
/// `g_SelectionPanels.Queue.getMaxNumberOfItems`.
const MAX_QUEUED: usize = 16;
/// `rowLength || 8` in `setupUnitPanel`.
const QUEUE_ROW: usize = 8;
/// `g_SelectionPanels.Training.getMaxNumberOfItems` with no other right
/// panel drawn, in rows of its `rowLength`.
const MAX_TRAINABLE: usize = 40;
const TRAINING_ROW: usize = 10;
/// `g_SelectionPanels.Research.getMaxNumberOfItems`: columns of a pair's
/// top over its bottom.
const MAX_RESEARCHABLE: usize = 10;
/// A research column's bottom button, a row below its top.
const RESEARCH_BOTTOM: usize = 39;
/// `resourcesToAlphaMask`: the red's alpha, of 255.
const NEEDED_ALPHA_BASE: f64 = 50.0;
const NEEDED_ALPHA_MAX: f64 = 125.0;
/// Pack and upgrade progress when not running.
const PROGRESS_NONE: u8 = 255;
/// The slider's `3 3 35 35` in its button.
const SLIDER_INSET: f64 = 3.0;
const SLIDER_SIDE: f64 = 32.0;
/// `captureBar[n]` and `captureBarMultiple[n]`, `g_MaxPlayers + 1`.
const CAPTURE_SEGMENTS: usize = 9;
/// `getPlayerColor(id, 128)` and `(id, 160)`, as CSS alphas.
pub(crate) const BAND_ALPHA: f64 = 128.0 / 255.0;
const GROUP_ALPHA: f64 = 160.0 / 255.0;
/// `ResourceSupply`'s `isInfinite`, the hook's template flag.
const INFINITE_SUPPLY: u32 = 1024;
/// `KillBeforeGather` is in the fauna templates only, which all have it.
const KILL_BEFORE_GATHER_CLASS: &str = "Animal";

/// An element and what was last written to it.
pub(crate) struct Out {
    pub(crate) element: HtmlElement,
    text: Option<String>,
    style: Option<String>,
    class: Option<String>,
    src: Option<String>,
    hidden: Option<bool>,
    tooltip: Option<String>,
}

impl Out {
    pub(crate) fn new(element: HtmlElement) -> Self {
        Out {
            element,
            text: None,
            style: None,
            class: None,
            src: None,
            hidden: None,
            tooltip: None,
        }
    }

    pub(crate) fn find(document: &Document, id: &str) -> Result<Self, String> {
        Ok(Out::new(crate::element(document, id)?))
    }

    pub(crate) fn text(&mut self, text: &str) {
        if self.text.as_deref() != Some(text) {
            self.element.set_text_content(Some(text));
            self.text = Some(text.to_string());
        }
    }

    pub(crate) fn style(&mut self, style: String) {
        if self.style.as_ref() != Some(&style) {
            let _ = self.element.set_attribute("style", &style);
            self.style = Some(style);
        }
    }

    /// The element's classes beyond those of the page.
    pub(crate) fn class(&mut self, class: String) {
        if self.class.as_ref() != Some(&class) {
            let _ = self.element.set_attribute("class", &class);
            self.class = Some(class);
        }
    }

    pub(crate) fn src(&mut self, src: Option<String>) {
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

    /// The engine markup `tooltip.rs` shows over it, none when empty.
    pub(crate) fn tooltip(&mut self, text: &str) {
        if self.tooltip.as_deref() != Some(text) {
            if text.is_empty() {
                let _ = self.element.remove_attribute(tooltip::ATTRIBUTE);
            } else {
                let _ = self.element.set_attribute(tooltip::ATTRIBUTE, text);
            }
            self.tooltip = Some(text.to_string());
        }
    }

    pub(crate) fn hidden(&mut self, hidden: bool) {
        if self.hidden != Some(hidden) {
            self.element.set_hidden(hidden);
            self.hidden = Some(hidden);
        }
    }
}

/// A bar's foreground and the capture bar's segments, by player.
pub(crate) struct CaptureBar {
    bar: Out,
    segments: Vec<Out>,
}

impl CaptureBar {
    fn find(document: &Document, bar: &str) -> Result<Self, String> {
        CaptureBar::new(document, crate::element(document, bar)?)
    }

    /// Over `bar`, which holds a `.segments`.
    pub(crate) fn new(document: &Document, bar: HtmlElement) -> Result<Self, String> {
        let bar = Out::new(bar);
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
    pub(crate) fn show(
        &mut self,
        parts: &[(usize, f64)],
        max: f64,
        palette: &[u32],
        vertical: bool,
    ) {
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
    /// The `i`-th of a grid, a `tag` of `class` at `setPanelObjectPosition`,
    /// with tooltips of `tooltip_style`.
    fn new(
        document: &Document,
        grid: &HtmlElement,
        tag: &str,
        class: &str,
        tooltip_style: &str,
        i: usize,
    ) -> Result<Self, String> {
        let button: HtmlElement = create(document, tag)?;
        if tag == "button" {
            let _ = button.set_attribute("type", "button");
        }
        let _ = button.set_attribute("class", class);
        let _ = button.set_attribute(tooltip::STYLE_ATTRIBUTE, tooltip_style);
        let _ = button.set_attribute("style", &grid_position(i, GROUP_ROW));
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
    attack: Out,
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
    /// `None` on a page from before the queue panel.
    queue: Option<QueuePanel>,
    /// `None` on a page from before the production panel.
    commands: Option<CommandsPanel>,
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

/// `unitQueuePanel`.
struct QueuePanel {
    root: Out,
    time: Out,
    /// The first item's progress, drawn only in the folded phone strip.
    progress: Out,
    cells: Vec<QueueCell>,
    /// What each shown cell is, and the queues of the selection by
    /// producer, for `tick`.
    shown: Vec<Queued>,
    queues: Vec<(u32, detail::Queue)>,
}

/// One `unitQueueButton`.
struct QueueCell {
    cell: Out,
    icon: Out,
    slider: Out,
    paused: Out,
    count: Out,
}

/// `unitCommands`' training, construction and research buttons.
struct CommandsPanel {
    root: Out,
    /// Training's buttons, then construction's in the cells after them, as
    /// `getNumberOfRightPanelButtons` places them.
    training: Vec<CommandCell>,
    research: Vec<ResearchColumn>,
}

/// One `unitTrainingButton` or `unitResearchButton`.
struct CommandCell {
    cell: Out,
    icon: Out,
    tint: Out,
    count: Out,
}

/// The research buttons of one `unitResearchPair` place.
struct ResearchColumn {
    column: Out,
    top: CommandCell,
    bottom: CommandCell,
    pair: Out,
}

/// `setupButton`'s sprite modifier.
#[derive(Clone, Copy)]
enum Modifier {
    None,
    /// "color:0 0 0 127:grayscale:".
    Grey,
    /// `resourcesToAlphaMask`, the alpha as a fraction.
    Needed(f64),
}

/// A selected entity, as the training and research panels read it.
struct Producer<'a> {
    owner: i32,
    upgrading: bool,
    queued: usize,
    trainable: &'a [ListItem],
    researchable: &'a [ListItem],
    buildable: &'a [ListItem],
}

/// A technology of a research slot.
#[derive(Clone, Copy)]
struct Tech<'a> {
    name: &'a str,
    item: &'a ListItem,
}

/// An item of `Researcher.GetTechnologiesList`.
#[derive(Clone, Copy)]
enum Slot<'a> {
    Empty,
    Single(Tech<'a>),
    Pair { top: Tech<'a>, bottom: Tech<'a> },
}

/// An item of `getItems`: its producer, its index in that queue, and
/// whether it is a ghost repeating an autoqueue.
#[derive(Clone, Copy)]
struct Queued {
    producer: u32,
    index: usize,
    ghost: bool,
}

/// What a click on the panel asks of the page.
pub enum Request {
    Follow(u32),
    Group {
        ids: Vec<u32>,
        remove: bool,
    },
    /// A hero button: `selection.reset` unless `add`, then `addList`.
    Select {
        id: u32,
        add: bool,
    },
    /// `selectAndMoveTo`.
    SelectAndFollow(u32),
}

/// What one update reads.
pub struct View<'a> {
    pub scene: &'a Scene,
    pub selected: &'a [u32],
    pub palette: &'a [u32],
    pub perspective: Perspective,
    /// Identity's Icon -> where an `<img>` finds its portrait.
    pub portrait: &'a dyn Fn(&str) -> Option<String>,
    /// A technology's name -> where an `<img>` finds its icon.
    pub technology_portrait: &'a dyn Fn(&str) -> Option<String>,
    /// An entity template's name -> where an `<img>` finds its icon, for
    /// a template no entity of which has been seen.
    pub template_portrait: &'a dyn Fn(&str) -> Option<String>,
    /// The JSON the data table names.
    pub data: &'a DataFiles,
    /// The "Detailed tooltips" setting, the engine's showdetailedtooltips.
    pub detailed: bool,
}

impl View<'_> {
    /// A template's `GetTemplateData` for `player`; `name` is the 1-based
    /// string of its name.
    fn template_data(&self, name: u32, player: i32) -> Option<JsValue> {
        let hash = self
            .scene
            .details
            .data
            .get(&DataKey::Template { name, player })?;
        self.data.get(*hash)
    }

    /// A technology's data for `player`'s civ; `name` is its 1-based
    /// string.
    fn technology_data(&self, name: u32, player: i32) -> Option<JsValue> {
        let civ = usize::try_from(player)
            .ok()
            .and_then(|player| self.scene.players.get(player))?;
        let civ = string_index(self.scene, &civ.civ)?;
        let hash = self
            .scene
            .details
            .data
            .get(&DataKey::Technology { name, civ })?;
        self.data.get(*hash)
    }
}

impl DetailsPanel {
    pub fn new(document: &Document) -> Result<Self, String> {
        let find = |id: &str| Out::find(document, id);
        let grid: HtmlElement = crate::element(document, "details-groups")?;
        let groups = (0..MAX_GROUPS)
            .map(|i| GroupButton::new(document, &grid, "button", "group", "sessionToolTipBold", i))
            .collect::<Result<_, _>>()?;
        let garrison = match crate::element::<HtmlElement>(document, "garrison-cells") {
            Ok(grid) => Some(GarrisonPanel {
                root: find("garrison")?,
                cells: (0..MAX_GARRISONED)
                    .map(|i| {
                        GroupButton::new(
                            document,
                            &grid,
                            "div",
                            "cell",
                            "sessionToolTipBottomBold",
                            i,
                        )
                    })
                    .collect::<Result<_, _>>()?,
                health: find("garrison-health")?,
                health_bar: find("garrison-health-bar")?,
            }),
            Err(_) => None,
        };
        let commands = match (
            crate::element::<HtmlElement>(document, "commands-training"),
            crate::element::<HtmlElement>(document, "commands-research"),
        ) {
            (Ok(training), Ok(research)) => Some(CommandsPanel {
                root: find("commands")?,
                training: (0..MAX_TRAINABLE)
                    .map(|i| CommandCell::new(document, &training, &grid_position(i, TRAINING_ROW)))
                    .collect::<Result<_, _>>()?,
                research: (0..MAX_RESEARCHABLE)
                    .map(|i| ResearchColumn::new(document, &research, i))
                    .collect::<Result<_, _>>()?,
            }),
            _ => None,
        };
        let queue = match crate::element::<HtmlElement>(document, "queue-cells") {
            Ok(grid) => Some(QueuePanel {
                root: find("queue")?,
                time: find("queue-time")?,
                progress: find("queue-progress")?,
                cells: (0..MAX_QUEUED)
                    .map(|i| QueueCell::new(document, &grid, i))
                    .collect::<Result<_, _>>()?,
                shown: Vec::new(),
                queues: Vec::new(),
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
            attack: find("details-attack")?,
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
            queue,
            commands,
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
        if let Some(queue) = &mut self.queue {
            queue.show(view, &entities);
            queue.tick(f64::from(view.scene.turn));
        }
        if let Some(commands) = &mut self.commands {
            commands.show(view, &entities);
        }
    }

    /// The queue's progress and time, between turns: `turn` is the
    /// interpolated turn the frame shows.
    pub fn tick(&mut self, turn: f64) {
        if let Some(queue) = &mut self.queue {
            queue.tick(turn);
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
                self.rank.tooltip(&format!("{rank} Rank"));
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
        self.portrait
            .tooltip(&portrait_tooltip(view, dot.template, dot.owner, primary));

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
        self.health_section.tooltip("Health");
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
            let small = health.is_some() && supply.is_some();
            self.capture_stats.text(&if small || !known {
                String::new()
            } else {
                format!("{} / {}", owned.ceil(), max.ceil())
            });
            self.capture_section.tooltip(&if small && known {
                tooltip::current_capture(owned, *max, None)
            } else {
                "Capture Points".to_string()
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
                let label = tooltip::header_font("Experience:");
                self.experience.tooltip(&if experience < required {
                    format!("{label} {experience} / {required}")
                } else {
                    format!("{label} {experience}")
                });
                self.experience.hidden(false);
            }
            _ => self.experience.hidden(true),
        }

        self.resource_section.hidden(supply.is_none());
        if let Some((fill, text)) = &supply {
            self.resource_bar.style(format!("width: {}%", 100.0 * fill));
            self.resource_stats.text(text);
        }

        // `attackAndResistanceStats`, from the stats the detail stream
        // names once they have arrived.
        let stats = match &groups[detail::STATS] {
            Some(Group::Stats(hash)) => view.data.get(*hash),
            _ => None,
        };
        let stats_tooltip = stats.map_or_else(String::new, |stats| {
            // `calculateCarriedResources`: everything carried, and a
            // trader's gain.
            let mut carried: Vec<(String, f64)> = Vec::new();
            if let Some(Group::Carrying(list)) = &groups[detail::CARRYING] {
                for item in list {
                    let code = string(scene, item.kind).unwrap_or_default().to_string();
                    carried.push((code, f64::from(item.amount)));
                }
            }
            if let Some(Group::Trader { goods, gain }) = &groups[detail::TRADER] {
                let code = string(scene, *goods).unwrap_or_default().to_string();
                carried.push((code, f64::from(*gain)));
            }
            join_lines(vec![
                tips::attack_tooltip(&stats),
                tips::healer_tooltip(&stats),
                tips::resistance_tooltip(&stats),
                tips::gather_tooltip(&stats),
                tips::speed_tooltip(&stats),
                tips::garrison_tooltip(&stats),
                tips::turrets_tooltip(&stats),
                tips::population_bonus_tooltip(&stats),
                tips::projectiles_tooltip(&stats),
                tips::rates_tooltip(&stats, "resourceTrickle", "Resource Trickle:"),
                tips::rates_tooltip(&stats, "upkeep", "Upkeep:"),
                tips::loot_tooltip(&stats, &carried),
            ])
        });
        self.attack.hidden(stats_tooltip.is_empty());
        self.attack.tooltip(&stats_tooltip);

        let carrying = self.carrying(view, &groups, template, health.is_some());
        match carrying {
            Some((icon, text, tip)) => {
                self.carrying_icon
                    .style(format!("background-image: var(--gui-{icon})"));
                self.carrying_icon.tooltip(&tip);
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

    /// The icon key, text and tooltip of the line under the bars, first
    /// that applies: carried resources, trader gain, foundation builders,
    /// gatherers, repair builders.
    fn carrying(
        &self,
        view: &View,
        groups: &Groups,
        template: Option<&TemplateDetails>,
        has_health: bool,
    ) -> Option<(String, String, String)> {
        let scene = view.scene;
        if let Some(Group::Carrying(carried)) = &groups[detail::CARRYING]
            && let Some(first) = carried.first()
        {
            let kind = string(scene, first.kind).unwrap_or_default();
            return Some((
                format!("resource-{kind}"),
                format!("{} / {}", first.amount, first.capacity),
                String::new(),
            ));
        }
        if let Some(Group::Trader { goods, gain }) = &groups[detail::TRADER]
            && *gain > 0
        {
            let kind = string(scene, *goods).unwrap_or_default();
            // `getTradingTooltip` parts the gain by player, which the hook
            // sums.
            return Some((
                format!("resource-{kind}"),
                gain.to_string(),
                format!("Gain: {gain}"),
            ));
        }
        if let Some(Group::Building(building)) = &groups[detail::FOUNDATION] {
            return Some((
                "repair".to_string(),
                builders_text(building, scene.turn),
                build_time_tooltip(building, scene.turn, "builders", "build"),
            ));
        }
        let kill_before_gather = template.is_some_and(|template| {
            pick::has_class(template, &scene.strings, KILL_BEFORE_GATHER_CLASS)
        });
        if groups[detail::SUPPLY].is_some()
            && (!kill_before_gather || !has_health)
            && let Some(Group::Gatherers { count, max }) = &groups[detail::GATHERERS]
        {
            return Some((
                "repair".to_string(),
                format!("{count} / {max}"),
                "Current/max gatherers".to_string(),
            ));
        }
        // `needsRepair`: below its max hitpoints.
        if let (Some(Group::Building(building)), Some(Group::Health { hitpoints, max })) =
            (&groups[detail::REPAIR], &groups[detail::HEALTH])
            && hitpoints < max
        {
            return Some((
                "repair".to_string(),
                builders_text(building, scene.turn),
                build_time_tooltip(building, scene.turn, "repairers", "repair"),
            ));
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
        self.health_multiple
            .tooltip(&tooltip::current_health(health, max_health, None));
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
            let owned = owner
                .and_then(|owner| capture.get(owner))
                .copied()
                .unwrap_or(0.0);
            self.capture_multiple.bar.tooltip(&tooltip::current_health(
                owned,
                max_capture,
                Some("Capture Points:"),
            ));
        }

        let count = entities.len().to_string();
        self.count.text(&count);
        self.count.tooltip(&units_tooltip(scene, entities));
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
            button
                .button
                .tooltip(&group_tooltip(scene, dot.template, dot.owner, ids));
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
            // An observer cannot unload, so only the names and the owner.
            cell.button.tooltip(&format!(
                "{}\nPlayer: {}",
                template_names(scene, template),
                player_name(scene, owner)
            ));
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
        self.health
            .tooltip(&tooltip::current_health(health, max_health, None));
        // A variable, since the folded phone strip lays the bar flat.
        self.health_bar.style(format!(
            "--lost: {}%",
            100.0 - 100.0 * fraction(health, max_health)
        ));
    }
}

impl QueueCell {
    fn new(document: &Document, grid: &HtmlElement, i: usize) -> Result<Self, String> {
        let cell: HtmlElement = create(document, "div")?;
        let _ = cell.set_attribute("class", "item");
        let _ = cell.set_attribute("style", &grid_position(i, QUEUE_ROW));
        let _ = cell.set_attribute(tooltip::STYLE_ATTRIBUTE, "sessionToolTipBottom");
        let icon: HtmlElement = create(document, "img")?;
        let _ = icon.set_attribute("alt", "");
        let slider: HtmlElement = create(document, "div")?;
        let _ = slider.set_attribute("class", "slider");
        let paused: HtmlElement = create(document, "div")?;
        let _ = paused.set_attribute("class", "paused");
        // groupIconsText is z 20, over the paused icon.
        let count: HtmlElement = create(document, "div")?;
        let _ = count.set_attribute("class", "text center sans-bold-stroke-14");
        for child in [&icon, &slider, &paused, &count] {
            cell.append_child(child)
                .map_err(|_| "cannot build a queue button")?;
        }
        grid.append_child(&cell)
            .map_err(|_| "cannot add a queue button")?;
        let mut cell = QueueCell {
            cell: Out::new(cell),
            icon: Out::new(icon),
            slider: Out::new(slider),
            paused: Out::new(paused),
            count: Out::new(count),
        };
        cell.cell.hidden(true);
        Ok(cell)
    }
}

impl QueuePanel {
    /// `g_SelectionPanels.Queue`'s `getItems`, `resizePanel` and
    /// `setupButton`, but for the progress, which `tick` draws.
    fn show(&mut self, view: &View, entities: &[u32]) {
        let scene = view.scene;
        let queues: Vec<(u32, detail::Queue)> = entities
            .iter()
            .filter_map(|&id| {
                let dot = scene.dots.get(&id)?;
                match &entity_groups(scene, id, dot.template)[detail::QUEUE] {
                    Some(Group::Queue(queue)) => Some((id, queue.clone())),
                    _ => None,
                }
            })
            .collect();
        // The first item of every producer before the second of any.
        let mut items = Vec::new();
        let longest = queues
            .iter()
            .map(|(_, queue)| queue.items.len())
            .max()
            .unwrap_or(0);
        for index in 0..longest {
            for (producer, queue) in &queues {
                if index < queue.items.len() {
                    items.push(Queued {
                        producer: *producer,
                        index,
                        ghost: false,
                    });
                }
            }
        }
        let repeat: Vec<Queued> = items
            .iter()
            .filter(|item| {
                queue_of(&queues, item.producer)
                    .is_some_and(|queue| queue.autoqueue && !queue.items[item.index].technology)
            })
            .map(|item| Queued {
                ghost: true,
                ..*item
            })
            .collect();
        if !repeat.is_empty() {
            let mut i = 0;
            while items.len() < MAX_QUEUED {
                items.push(repeat[i % repeat.len()]);
                i += 1;
            }
        }
        items.truncate(MAX_QUEUED);

        self.root.hidden(items.is_empty());
        let rows = items.len().div_ceil(QUEUE_ROW).max(1);
        self.root.style(format!("--rows: {rows}"));
        for (i, cell) in self.cells.iter_mut().enumerate() {
            let Some(item) = items.get(i) else {
                cell.cell.hidden(true);
                continue;
            };
            let Some(queued) =
                queue_of(&queues, item.producer).and_then(|queue| queue.items.get(item.index))
            else {
                cell.cell.hidden(true);
                continue;
            };
            let name = string(scene, queued.name).unwrap_or_default();
            let icon = if queued.technology {
                (view.technology_portrait)(name)
            } else {
                unit_portrait(view, name)
            };
            cell.icon.src(icon);
            cell.count.text(&if queued.count > 1 {
                queued.count.to_string()
            } else {
                String::new()
            });
            // The tint follows `paused` before `ghost`, the stop icon
            // only real items.
            let mut class = String::from("item");
            if item.ghost {
                class.push_str(" ghost");
            }
            if queued.paused {
                class.push_str(" paused");
            }
            cell.cell.class(class);
            let paused = queued.paused && !item.ghost;
            cell.paused.hidden(!paused);
            let owner = scene.dots.get(&item.producer).map_or(-1, |dot| dot.owner);
            cell.cell
                .tooltip(&queue_tooltip(view, queued, item.ghost, owner));
            if item.ghost {
                cell.slider.style(format!("top: {SLIDER_INSET}px"));
            }
            cell.cell.hidden(false);
        }
        self.shown = items;
        self.queues = queues;
    }

    /// The sliders and `queueTimeRemaining` at `turn`.
    fn tick(&mut self, turn: f64) {
        let mut time = String::new();
        // The undone part, as the sliders show it; nothing when no item runs.
        let mut undone = 0.0;
        for (i, (item, cell)) in self.shown.iter().zip(&mut self.cells).enumerate() {
            if item.ghost {
                continue;
            }
            let Some(queue) = queue_of(&self.queues, item.producer) else {
                continue;
            };
            let Some(queued) = queue.items.get(item.index) else {
                continue;
            };
            let remaining = queue.remaining_ms(item.index, turn);
            let total = f64::from(queued.total) * 100.0;
            let progress = if total > 0.0 {
                (1.0 - remaining / total).clamp(0.0, 1.0)
            } else {
                0.0
            };
            cell.slider.style(format!(
                "top: {}px",
                SLIDER_INSET + (progress * SLIDER_SIDE).round()
            ));
            if i == 0 {
                time = minutes_seconds(remaining / 1000.0);
                undone = 1.0 - progress;
            }
        }
        self.time.text(&time);
        self.progress
            .style(format!("width: {:.1}%", undone * 100.0));
    }
}

impl CommandCell {
    fn new(document: &Document, parent: &HtmlElement, position: &str) -> Result<Self, String> {
        let cell: HtmlElement = create(document, "div")?;
        let _ = cell.set_attribute("class", "item");
        let _ = cell.set_attribute("style", position);
        let _ = cell.set_attribute(tooltip::STYLE_ATTRIBUTE, "sessionToolTipBottom");
        let icon: HtmlElement = create(document, "img")?;
        let _ = icon.set_attribute("alt", "");
        let tint: HtmlElement = create(document, "div")?;
        let _ = tint.set_attribute("class", "tint");
        let count: HtmlElement = create(document, "div")?;
        let _ = count.set_attribute("class", "text center sans-bold-stroke-14");
        for child in [&icon, &tint, &count] {
            cell.append_child(child)
                .map_err(|_| "cannot build a production button")?;
        }
        parent
            .append_child(&cell)
            .map_err(|_| "cannot add a production button")?;
        let mut cell = CommandCell {
            cell: Out::new(cell),
            icon: Out::new(icon),
            tint: Out::new(tint),
            count: Out::new(count),
        };
        cell.cell.hidden(true);
        Ok(cell)
    }

    fn show(&mut self, icon: Option<String>, modifier: Modifier, count: u32) {
        self.icon.src(icon);
        self.cell.class(String::from(match modifier {
            Modifier::Grey => "item grey",
            _ => "item",
        }));
        self.tint.style(match modifier {
            Modifier::Needed(alpha) => format!("background: rgba(255, 0, 0, {alpha:.3})"),
            _ => String::new(),
        });
        self.count.text(&if count > 1 {
            count.to_string()
        } else {
            String::new()
        });
        self.cell.hidden(false);
    }
}

impl ResearchColumn {
    fn new(document: &Document, parent: &HtmlElement, i: usize) -> Result<Self, String> {
        let column: HtmlElement = create(document, "div")?;
        let _ = column.set_attribute("class", "column");
        let _ = column.set_attribute("style", &grid_position(i, MAX_RESEARCHABLE));
        parent
            .append_child(&column)
            .map_err(|_| "cannot add a research column")?;
        let top = CommandCell::new(document, &column, "left: 0; top: 0")?;
        let bottom = CommandCell::new(
            document,
            &column,
            &format!("left: 0; top: {RESEARCH_BOTTOM}px"),
        )?;
        // Over both buttons, as the pairs come after them in the XML.
        let pair: HtmlElement = create(document, "div")?;
        let _ = pair.set_attribute("class", "pair");
        column
            .append_child(&pair)
            .map_err(|_| "cannot add a research pair")?;
        let mut column = ResearchColumn {
            column: Out::new(column),
            top,
            bottom,
            pair: Out::new(pair),
        };
        column.column.hidden(true);
        Ok(column)
    }
}

impl CommandsPanel {
    /// `setupUnitPanel` of `Training`, `Construction` and `Research`, for an
    /// observer.
    fn show(&mut self, view: &View, entities: &[u32]) {
        let scene = view.scene;
        let mut producers = Vec::new();
        for &id in entities {
            let Some(dot) = scene.dots.get(&id) else {
                continue;
            };
            let groups = entity_groups(scene, id, dot.template);
            let list = |slot: usize| match &groups[slot] {
                Some(Group::List(index)) => scene
                    .details
                    .lists
                    .get(*index as usize)
                    .map_or(&[][..], Vec::as_slice),
                _ => &[][..],
            };
            producers.push(Producer {
                owner: dot.owner,
                upgrading: matches!(
                    groups[detail::PROGRESS],
                    Some(Group::Progress { upgrade, .. }) if upgrade != PROGRESS_NONE
                ),
                queued: match &groups[detail::QUEUE] {
                    Some(Group::Queue(queue)) => queue.items.len(),
                    _ => 0,
                },
                trainable: list(detail::TRAINABLE),
                researchable: list(detail::RESEARCHABLE),
                buildable: list(detail::BUILDABLE),
            });
        }
        // `data.player`: the first selected entity's owner.
        let player = producers.first().map_or(-1, |producer| producer.owner);
        let all_upgrading = producers.iter().all(|producer| producer.upgrading);

        let trainable = list_items(
            scene,
            &producers,
            player,
            |producer| producer.trainable,
            MAX_TRAINABLE,
        );
        let buildable = list_items(
            scene,
            &producers,
            player,
            |producer| producer.buildable,
            MAX_TRAINABLE - trainable.len(),
        );
        let (training_cells, construction_cells) = self.training.split_at_mut(trainable.len());
        let mut drawn = false;
        for (cell, &(name, item)) in training_cells.iter_mut().zip(&trainable) {
            if item.flags & ListItem::HIDDEN != 0 {
                cell.cell.hidden(true);
                continue;
            }
            // `getTrainingStatus` without the batch key: one each in the
            // producers that can train it, as far as the limit allows.
            let producing = producers
                .iter()
                .filter(|producer| {
                    !producer.upgrading
                        && producer
                            .trainable
                            .iter()
                            .any(|other| string(scene, other.name) == Some(name))
                })
                .count() as u32;
            let count = item
                .can_be_added
                .map_or(producing, |limit| producing.min(limit));
            // `multiplyEntityCosts`: floored after the multiplication.
            let short = needed_amounts(scene, player, item, |cost| {
                (u64::from(cost) * u64::from(count) / 100) as f64
            });
            let modifier = if all_upgrading {
                Modifier::Grey
            } else if item.flags & ListItem::UNKNOWN != 0 {
                Modifier::None
            } else if item.flags & ListItem::REQUIREMENTS_MET == 0 || item.can_be_added == Some(0) {
                Modifier::Grey
            } else {
                needed(&short)
            };
            cell.show(unit_portrait(view, name), modifier, count);
            cell.cell
                .tooltip(&training_tooltip(view, item, player, count, &short));
            drawn = true;
        }

        for (i, cell) in construction_cells.iter_mut().enumerate() {
            let Some(&(name, item)) = buildable.get(i) else {
                cell.cell.hidden(true);
                continue;
            };
            if item.flags & ListItem::HIDDEN != 0 {
                cell.cell.hidden(true);
                continue;
            }
            // `multiplyEntityCosts(template, 1)`.
            let short = needed_amounts(scene, player, item, |cost| (cost / 100) as f64);
            let modifier = if item.flags & ListItem::UNKNOWN != 0 {
                Modifier::None
            } else if item.flags & ListItem::REQUIREMENTS_MET == 0 || item.can_be_added == Some(0) {
                Modifier::Grey
            } else {
                needed(&short)
            };
            cell.show(unit_portrait(view, name), modifier, 0);
            cell.cell
                .tooltip(&construction_tooltip(view, item, player, &short));
            drawn = true;
        }

        let research = research_items(scene, &producers, trainable.len() + buildable.len());
        for (i, column) in self.research.iter_mut().enumerate() {
            let Some(&(slot, producer)) = research.get(i) else {
                column.column.hidden(true);
                continue;
            };
            let techs: Vec<Tech> = match slot {
                Slot::Empty => Vec::new(),
                Slot::Single(tech) => vec![tech],
                Slot::Pair { top, bottom } => vec![bottom, top],
            };
            // A hidden one is left out, and what remains starts from the
            // bottom; the pair sign needs both.
            let shown: Vec<Tech> = techs
                .iter()
                .copied()
                .filter(|tech| tech.item.flags & ListItem::HIDDEN == 0)
                .collect();
            column.column.hidden(shown.is_empty());
            drawn |= !shown.is_empty();
            column
                .pair
                .hidden(!matches!(slot, Slot::Pair { .. }) || shown.len() < 2);
            let upgrading = producers
                .get(producer)
                .is_some_and(|producer| producer.upgrading);
            for (index, cell) in [&mut column.bottom, &mut column.top]
                .into_iter()
                .enumerate()
            {
                let Some(tech) = shown.get(index) else {
                    cell.cell.hidden(true);
                    continue;
                };
                let short =
                    needed_amounts(scene, player, tech.item, |cost| f64::from(cost) / 100.0);
                let modifier = if upgrading {
                    Modifier::Grey
                } else if tech.item.flags & ListItem::UNKNOWN != 0 {
                    Modifier::None
                } else if tech.item.flags & ListItem::REQUIREMENTS_MET == 0 {
                    Modifier::Grey
                } else {
                    needed(&short)
                };
                cell.show((view.technology_portrait)(tech.name), modifier, 0);
                let owner = producers
                    .get(producer)
                    .map_or(player, |producer| producer.owner);
                cell.cell
                    .tooltip(&research_tooltip(view, tech.item, owner, &short));
            }
        }

        // Unlike vanilla's always-framed `unitCommands`, an empty frame
        // (and on phones its fold tab) would only take space here. Lists
        // of only hidden items or empty slots draw nothing either.
        self.root.hidden(!drawn);
    }
}

/// `getAllTrainableEntities` or `GetAllBuildableEntities`: every
/// producer's `list` in selection order, duplicates dropped, at most
/// `max`, each read as the first selected entity's owner reads it where a
/// producer of that owner lists it.
fn list_items<'a>(
    scene: &'a Scene,
    producers: &[Producer<'a>],
    player: i32,
    list: impl Fn(&Producer<'a>) -> &'a [ListItem],
    max: usize,
) -> Vec<(&'a str, &'a ListItem)> {
    let mut items: Vec<(&str, &ListItem, i32)> = Vec::new();
    for producer in producers {
        for item in list(producer) {
            let Some(name) = string(scene, item.name).filter(|name| !name.is_empty()) else {
                continue;
            };
            match items.iter_mut().find(|(other, _, _)| *other == name) {
                Some(entry) => {
                    if entry.2 != player && producer.owner == player {
                        *entry = (name, item, producer.owner);
                    }
                }
                None => items.push((name, item, producer.owner)),
            }
        }
    }
    items.truncate(max);
    items
        .into_iter()
        .map(|(name, item, _)| (name, item))
        .collect()
}

/// A list's slots: a pair is its top, flagged, then its bottom; an older
/// stream sends a pair as one "top bottom" name.
fn research_slots<'a>(scene: &'a Scene, list: &'a [ListItem]) -> Vec<Slot<'a>> {
    let mut slots = Vec::new();
    let mut items = list.iter();
    while let Some(item) = items.next() {
        let name = string(scene, item.name).unwrap_or_default();
        let tech = |name| Tech { name, item };
        if item.flags & ListItem::PAIR_TOP != 0 {
            let Some(bottom) = items.next() else {
                break;
            };
            slots.push(Slot::Pair {
                top: tech(name),
                bottom: Tech {
                    name: string(scene, bottom.name).unwrap_or_default(),
                    item: bottom,
                },
            });
        } else if name.is_empty() {
            slots.push(Slot::Empty);
        } else if let Some((top, bottom)) = name
            .split_once(' ')
            .filter(|_| item.flags & ListItem::UNKNOWN != 0)
        {
            slots.push(Slot::Pair {
                top: tech(top),
                bottom: tech(bottom),
            });
        } else {
            slots.push(Slot::Single(tech(name)));
        }
    }
    slots
}

/// The same technology or pair at the same costs, which are the
/// producer's `techCostMultiplier` applied.
fn same_slot(a: &Slot, b: &Slot) -> bool {
    let same = |a: &Tech, b: &Tech| a.name == b.name && a.item.costs == b.item.costs;
    match (a, b) {
        (Slot::Single(a), Slot::Single(b)) => same(a, b),
        (
            Slot::Pair {
                top: a_top,
                bottom: a_bottom,
            },
            Slot::Pair {
                top: b_top,
                bottom: b_bottom,
            },
        ) => same(a_top, b_top) && same(a_bottom, b_bottom),
        _ => false,
    }
}

/// `g_SelectionPanels.Research.getItems`: each slot and the index of its
/// producer in `producers`, at most `MAX_RESEARCHABLE`. `training` is the
/// training buttons shown (`getNumberOfRightPanelButtons`).
fn research_items<'a>(
    scene: &'a Scene,
    producers: &[Producer<'a>],
    training: usize,
) -> Vec<(Slot<'a>, usize)> {
    if let [producer] = producers {
        let mut items: Vec<(Slot, usize)> = research_slots(scene, producer.researchable)
            .into_iter()
            .map(|slot| (slot, 0))
            .collect();
        items.truncate(MAX_RESEARCHABLE);
        return items;
    }
    // Not upgrading first, then the shortest queue; a stable sort, as
    // Array.prototype.sort.
    let mut order: Vec<usize> = (0..producers.len()).collect();
    order.sort_by_key(|&index| (producers[index].upgrading, producers[index].queued));
    let mut items: Vec<(Slot, usize)> = Vec::new();
    for index in order {
        let filtered: Vec<Slot> = research_slots(scene, producers[index].researchable)
            .into_iter()
            .filter(|slot| !matches!(slot, Slot::Empty))
            .filter(|slot| !items.iter().any(|(item, _)| same_slot(item, slot)))
            .collect();
        let pairs = filtered
            .iter()
            .any(|slot| matches!(slot, Slot::Pair { .. }));
        if filtered.len() + items.len() <= MAX_RESEARCHABLE
            && training <= MAX_RESEARCHABLE * if pairs { 1 } else { 2 }
        {
            items.extend(filtered.into_iter().map(|slot| (slot, index)));
        }
    }
    items
}

/// `GetNeededResources` of `item`'s costs, each as `amount` gives it from
/// the cost in 1/100, against `player`'s stockpile: what each resource
/// lacks, none when it can be paid.
fn needed_amounts(
    scene: &Scene,
    player: i32,
    item: &ListItem,
    amount: impl Fn(u32) -> f64,
) -> Vec<(String, f64)> {
    let Some(economy) = usize::try_from(player)
        .ok()
        .and_then(|player| scene.economy.get(player))
    else {
        return Vec::new();
    };
    let mut short = Vec::new();
    for &(resource, cost) in &item.costs {
        // A type the player has no count of is never needed.
        let Some(stock) = economy
            .resources
            .iter()
            .find(|stock| stock.code == resource)
        else {
            continue;
        };
        let amount = amount(cost);
        let stock = f64::from(stock.amount);
        if amount > stock {
            short.push((
                string(scene, resource).unwrap_or_default().to_string(),
                amount - stock,
            ));
        }
    }
    short
}

/// `resourcesToAlphaMask`'s tint of what `needed_amounts` found.
fn needed(short: &[(String, f64)]) -> Modifier {
    if short.is_empty() {
        return Modifier::None;
    }
    let total: f64 = short.iter().map(|(_, amount)| amount).sum();
    Modifier::Needed(NEEDED_ALPHA_MAX.min((total / 10.0).round() + NEEDED_ALPHA_BASE) / 255.0)
}

/// `g_SelectionPanels.Training.setupButton`'s tooltip for an observer:
/// without the hotkey, the limits (only what can be added is sent), the
/// template viewer line (V9) and the batch line.
fn training_tooltip(
    view: &View,
    item: &ListItem,
    player: i32,
    count: u32,
    short: &[(String, f64)],
) -> String {
    let Some(data) = view.template_data(item.name, player) else {
        return String::new();
    };
    let mut lines = vec![
        tips::entity_names_formatted(&data),
        tips::visible_classes(&data),
        tips::auras_tooltip(&data),
        tips::entity_tooltip(&data),
        tips::cost_tooltip(&data, f64::from(count), None),
    ];
    if view.detailed {
        lines.extend(tips::detailed_lines(&data));
    }
    if item.flags & ListItem::REQUIREMENTS_MET == 0 {
        let technology = |name: &str| {
            let name = string_index(view.scene, name)?;
            view.technology_data(name, player)
        };
        lines.push(tips::requirements_tooltip(&data, &technology));
    }
    lines.push(tips::needed_resources_tooltip(short));
    join_lines(lines)
}

/// `g_SelectionPanels.Construction.setupButton`'s tooltip for an observer:
/// without the limits (only what can be added is sent) and the template
/// viewer line (V9).
fn construction_tooltip(
    view: &View,
    item: &ListItem,
    player: i32,
    short: &[(String, f64)],
) -> String {
    let Some(data) = view.template_data(item.name, player) else {
        return String::new();
    };
    let mut lines = vec![
        tips::entity_names_formatted(&data),
        tips::visible_classes(&data),
        tips::auras_tooltip(&data),
        tips::entity_tooltip(&data),
        tips::cost_tooltip(&data, 1.0, None),
        tips::dropsite_tooltip(&data),
        tips::garrison_tooltip(&data),
        tips::turrets_tooltip(&data),
        tips::population_bonus_tooltip(&data),
    ];
    if item.flags & ListItem::REQUIREMENTS_MET == 0 {
        let technology = |name: &str| {
            let name = string_index(view.scene, name)?;
            view.technology_data(name, player)
        };
        lines.push(tips::requirements_tooltip(&data, &technology));
    }
    lines.push(tips::needed_resources_tooltip(short));
    join_lines(lines)
}

/// `g_SelectionPanels.Research.setupButton`'s tooltip, its costs those of
/// the list (the producer's multiplier applied), and without the entity
/// counts a requirement still lacks (`classCounts` is not sent) or the
/// template viewer line (V9).
fn research_tooltip(view: &View, item: &ListItem, owner: i32, short: &[(String, f64)]) -> String {
    let Some(data) = view.technology_data(item.name, owner) else {
        return String::new();
    };
    let costs: Vec<(String, f64)> = item
        .costs
        .iter()
        .map(|(resource, cost)| {
            (
                string(view.scene, *resource)
                    .unwrap_or_default()
                    .to_string(),
                f64::from(*cost) / 100.0,
            )
        })
        .collect();
    let mut lines = vec![
        tips::entity_names_formatted(&data),
        tips::entity_tooltip(&data),
        tips::cost_tooltip(&data, 1.0, Some(&costs)),
    ];
    if item.flags & ListItem::REQUIREMENTS_MET == 0 {
        let requirements = tips::requirements_text(&data);
        if !requirements.is_empty() {
            lines.push(tooltip::objection_font(&requirements));
        }
    }
    lines.push(tips::needed_resources_tooltip(short));
    join_lines(lines)
}

/// `displaySingle`'s `iconBorder` tooltip, from the owner's template data
/// where the engine reads the viewer's, and without the template viewer
/// line (V9).
fn portrait_tooltip(view: &View, template: u32, owner: i32, primary: &str) -> String {
    let scene = view.scene;
    let data = scene
        .templates
        .get(template as usize)
        .and_then(|name| string_index(scene, name))
        .and_then(|name| view.template_data(name, owner));
    let mut lines = vec![tooltip::tags(primary, &[("font", "sans-bold-16")])];
    if let Some(data) = &data {
        lines.extend([
            tips::visible_classes(data),
            tips::auras_tooltip(data),
            tips::entity_tooltip(data),
            tips::treasure_tooltip(data),
        ]);
    }
    let unit = template_details(scene, template)
        .is_some_and(|details| pick::has_class(details, &scene.strings, "Unit"));
    lines.push(
        if unit {
            "Left-click to follow."
        } else {
            "Left-click to focus."
        }
        .to_string(),
    );
    join_lines(lines)
}

fn join_lines(lines: Vec<String>) -> String {
    lines
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The 1-based string index of `text`, if the stream has it.
fn string_index(scene: &Scene, text: &str) -> Option<u32> {
    scene
        .strings
        .iter()
        .position(|other| other == text)
        .map(|index| index as u32 + 1)
}

/// A unit template's portrait: from the details of a template seen in the
/// match, else from the bake's table.
fn unit_portrait(view: &View, name: &str) -> Option<String> {
    let scene = view.scene;
    template_index(scene, name)
        .and_then(|template| template_details(scene, template))
        .and_then(|template| string(scene, template.icon))
        .and_then(|icon| (view.portrait)(icon))
        .or_else(|| (view.template_portrait)(name))
}

fn queue_of(queues: &[(u32, detail::Queue)], producer: u32) -> Option<&detail::Queue> {
    queues
        .iter()
        .find(|(id, _)| *id == producer)
        .map(|(_, queue)| queue)
}

/// `getEntityNames` of a template the template table has details for.
fn template_names(scene: &Scene, template: u32) -> String {
    let details = template_details(scene, template);
    let name = |index: Option<u32>| {
        index
            .and_then(|index| string(scene, index))
            .unwrap_or_default()
    };
    tooltip::entity_names(
        name(details.map(|details| details.generic_name)),
        name(details.map(|details| details.specific_name)),
    )
}

fn player_name(scene: &Scene, owner: i32) -> &str {
    usize::try_from(owner)
        .ok()
        .and_then(|owner| scene.players.get(owner))
        .map_or("", |player| player.name.as_str())
}

/// The resources of `carrying` and of a trader's goods, by type in the
/// order first met, as `setupButton` and `calculateCarriedResources` sum
/// them.
fn add_carried(scene: &Scene, groups: &Groups, carried: &mut Vec<(String, u32)>) {
    let mut add = |kind: u32, amount: u32| {
        let kind = string(scene, kind).unwrap_or_default().to_string();
        match carried.iter_mut().find(|(code, _)| *code == kind) {
            Some((_, sum)) => *sum += amount,
            None => carried.push((kind, amount)),
        }
    };
    if let Some(Group::Carrying(list)) = &groups[detail::CARRYING]
        && let Some(first) = list.first()
    {
        add(first.kind, first.amount);
    }
    if let Some(Group::Trader { goods, gain }) = &groups[detail::TRADER]
        && *gain > 0
    {
        add(*goods, *gain);
    }
}

/// `g_SelectionPanels.Selection.setupButton`'s tooltip, for an observer.
fn group_tooltip(scene: &Scene, template: u32, owner: i32, ids: &[u32]) -> String {
    let mut tooltip = template_names(scene, template);
    let mut carried = Vec::new();
    for &id in ids {
        if let Some(dot) = scene.dots.get(&id) {
            add_carried(scene, &entity_groups(scene, id, dot.template), &mut carried);
        }
    }
    if !carried.is_empty() {
        tooltip.push('\n');
        tooltip.push_str(
            &carried
                .iter()
                .map(|(code, amount)| format!("{}{amount}", tooltip::resource_icon(code)))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    tooltip.push_str(&format!("\nPlayer: {}", player_name(scene, owner)));
    tooltip
}

/// `displayMultiple`'s `numberOfUnits` tooltip. Garrisonable sizes and
/// loot are not sent, so only holders count to the garrison size and the
/// loot line has the carried resources alone.
fn units_tooltip(scene: &Scene, entities: &[u32]) -> String {
    let mut garrison_size = 0;
    let mut carried = Vec::new();
    for &id in entities {
        let Some(dot) = scene.dots.get(&id) else {
            continue;
        };
        let groups = entity_groups(scene, id, dot.template);
        add_carried(scene, &groups, &mut carried);
        if let Some(Group::Garrison { occupied, .. }) = &groups[detail::GARRISON] {
            garrison_size += occupied;
        }
    }
    let list = |carried: &[(String, u32)]| {
        carried
            .iter()
            .filter(|(_, amount)| *amount != 0)
            .map(|(code, amount)| format!("{} {amount}", tooltip::resource_icon(code)))
            .collect::<Vec<_>>()
            .join("  ")
    };
    let mut tooltip = String::new();
    if garrison_size > 0 {
        tooltip = format!(
            "{}: {}\n",
            tooltip::header_font("Garrison Size"),
            tooltip::body_font(&garrison_size.to_string())
        );
    }
    if !carried.is_empty() {
        tooltip = format!(
            "{} {}\n",
            tooltip::header_font("Carrying:"),
            tooltip::body_font(&list(&carried))
        );
        tooltip.push_str(&format!(
            "{} {}",
            tooltip::header_font("Loot:"),
            tooltip::body_font(&list(&carried))
        ));
    }
    tooltip
}

/// `g_SelectionPanels.Queue.setupButton`'s tooltip, without the template
/// viewer line (V9): none until the item's data has arrived.
fn queue_tooltip(view: &View, queued: &detail::QueueItem, ghost: bool, owner: i32) -> String {
    let scene = view.scene;
    let name = string(scene, queued.name).unwrap_or_default();
    let names = if queued.technology {
        view.technology_data(queued.name, owner)
            .map(|data| tips::entity_names(&data))
    } else {
        view.template_data(queued.name, owner)
            .map(|data| tips::entity_names(&data))
            .or_else(|| template_index(scene, name).map(|template| template_names(scene, template)))
    };
    let Some(names) = names else {
        return String::new();
    };
    let mut lines = vec![names];
    if ghost {
        lines.push("The auto-queue will try to train this item later.".to_string());
    }
    if queued.missing_population > 0 {
        lines.push(tooltip::objection_font("Insufficient population capacity:"));
        lines.push(format!(
            "{} {}",
            tooltip::resource_icon("population"),
            queued.missing_population
        ));
    }
    lines.join("\n")
}

/// The template table's index of a template name.
fn template_index(scene: &Scene, name: &str) -> Option<u32> {
    let index = scene
        .templates
        .iter()
        .position(|template| template == name)?;
    u32::try_from(index).ok()
}

/// `setPanelObjectPosition` of the `i`-th button, 38 px and a 1 px margin.
fn grid_position(i: usize, row: usize) -> String {
    format!(
        "left: {}px; top: {}px",
        i % row * GROUP_STEP,
        i / row * GROUP_STEP
    )
}

pub(crate) fn create<T: JsCast>(document: &Document, tag: &str) -> Result<T, String> {
    document
        .create_element(tag)
        .ok()
        .and_then(|element| element.dyn_into().ok())
        .ok_or_else(|| format!("cannot create a <{tag}>"))
}

pub(crate) fn string(scene: &Scene, index: u32) -> Option<&str> {
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

pub(crate) fn template_details(scene: &Scene, template: u32) -> Option<&TemplateDetails> {
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
    format!(
        "({})\n{}",
        building.builders,
        minutes_seconds(building.seconds_remaining(turn))
    )
}

/// `getBuildTimeTooltip` (`noun` "builders", `kind` "build") and
/// `getRepairTimeTooltip` ("repairers", "repair"), without the line of
/// what another worker would save, as the hook does not send
/// `timeRemainingNew`.
fn build_time_tooltip(building: &detail::Building, turn: u32, noun: &str, kind: &str) -> String {
    let mut lines = vec![format!(
        "{} {}",
        tooltip::header_font(&format!("Number of {noun}:")),
        building.builders
    )];
    if building.builders > 0 {
        lines.push(format!(
            "{} {}",
            tooltip::header_font(&format!("Remaining {kind} time:")),
            tooltip::seconds_string(building.seconds_remaining(turn).floor() as i64)
        ));
    }
    lines.join("\n")
}

/// `FormatMillisecondsIntoDateStringGMT` with "m:ss", which drops the
/// fraction.
fn minutes_seconds(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
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
pub(crate) fn css_color(color: u32, alpha: f64) -> String {
    format!(
        "rgba({}, {}, {}, {alpha:.3})",
        color >> 16 & 0xFF,
        color >> 8 & 0xFF,
        color & 0xFF
    )
}
