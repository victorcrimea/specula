//! The started technologies of the player of `/play`, a port of
//! gui/session/ResearchProgress.js, drawn into the page's `#research` under
//! the clock as the engine stacks them under its overlay counters.
//!
//! `GetStartedResearch` is read from the detail stream's production
//! queues. Their order across buildings, which the engine takes from when
//! each was queued, is not sent, so a technology goes last when it is first
//! seen started (CONTROL-IMPL.md).

use std::cell::RefCell;

use web_sys::Document;
use web_sys::HtmlElement;

use crate::detail;
use crate::detail::Group;
use crate::lang;
use crate::panel::Out;
use crate::panel::Request;
use crate::panel::SLIDER_INSET;
use crate::panel::SLIDER_SIDE;
use crate::panel::View;
use crate::panel::create;
use crate::panel::entity_groups;
use crate::panel::minutes_seconds;
use crate::panel::string;
use crate::tips;
use crate::tooltip;

/// `<repeat count="10">` of ResearchProgress.xml.
const MAX_BUTTONS: usize = 10;

/// One `researchStartedButton[n]`.
struct Cell {
    cell: Out,
    icon: Out,
    slider: Out,
    paused: Out,
    time: Out,
}

impl Cell {
    fn new(document: &Document, parent: &HtmlElement) -> Result<Self, String> {
        let cell: HtmlElement = create(document, "button")?;
        let _ = cell.set_attribute("type", "button");
        let _ = cell.set_attribute("class", "item");
        let _ = cell.set_attribute(tooltip::STYLE_ATTRIBUTE, "sessionToolTipBottom");
        let icon: HtmlElement = create(document, "img")?;
        let _ = icon.set_attribute("alt", "");
        let slider: HtmlElement = create(document, "div")?;
        let _ = slider.set_attribute("class", "slider");
        let time: HtmlElement = create(document, "div")?;
        let _ = time.set_attribute("class", "text center sans-stroke-14");
        let paused: HtmlElement = create(document, "div")?;
        let _ = paused.set_attribute("class", "paused");
        for child in [&icon, &slider, &time, &paused] {
            cell.append_child(child)
                .map_err(|_| "cannot build a research button")?;
        }
        parent
            .append_child(&cell)
            .map_err(|_| "cannot add a research button")?;
        let mut cell = Cell {
            cell: Out::new(cell),
            icon: Out::new(icon),
            slider: Out::new(slider),
            paused: Out::new(paused),
            time: Out::new(time),
        };
        cell.cell.hidden(true);
        Ok(cell)
    }
}

/// A started technology: its researcher and where it is in its queue.
struct Started {
    name: u32,
    producer: u32,
    index: usize,
    queue: detail::Queue,
}

pub struct ResearchProgress {
    root: Out,
    cells: Vec<Cell>,
    /// The technologies' names in the order they were first seen started.
    order: Vec<u32>,
    match_id: u64,
    shown: Vec<Started>,
    requests: RefCell<Vec<Request>>,
}

impl ResearchProgress {
    pub fn new(document: &Document) -> Result<Self, String> {
        let root: HtmlElement = crate::element(document, "research")?;
        let cells = (0..MAX_BUTTONS)
            .map(|_| Cell::new(document, &root))
            .collect::<Result<_, _>>()?;
        Ok(ResearchProgress {
            root: Out::new(root),
            cells,
            order: Vec::new(),
            match_id: 0,
            shown: Vec::new(),
            requests: RefCell::new(Vec::new()),
        })
    }

    /// The buttons, for the page to wire.
    pub fn buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.cells.iter().map(|cell| &cell.cell.element)
    }

    /// `ResearchProgressButton.onPress`: `selectAndMoveTo(researcher)`.
    pub fn clicked(&self, index: usize) {
        if let Some(started) = self.shown.get(index) {
            self.requests
                .borrow_mut()
                .push(Request::SelectAndFollow(started.producer));
        }
    }

    pub fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }

    /// `updateResearchProgress`, but for the progress, which `tick` draws.
    pub fn update(&mut self, view: &View) {
        let scene = view.scene;
        if scene.match_id != self.match_id {
            self.match_id = scene.match_id;
            self.order.clear();
        }
        let started = match view.player {
            Some(player) => started(view, player),
            None => Vec::new(),
        };
        self.order
            .retain(|name| started.iter().any(|started| started.name == *name));
        for item in &started {
            if !self.order.contains(&item.name) {
                self.order.push(item.name);
            }
        }
        let mut shown: Vec<Started> = started;
        shown.sort_by_key(|item| self.order.iter().position(|name| *name == item.name));
        shown.truncate(MAX_BUTTONS);
        let player = view.player.unwrap_or(-1);
        for (i, cell) in self.cells.iter_mut().enumerate() {
            let Some(item) = shown.get(i) else {
                cell.cell.hidden(true);
                continue;
            };
            let paused = item.queue.items[item.index].paused;
            let name = string(scene, item.name).unwrap_or_default();
            cell.icon.src((view.technology_portrait)(name));
            cell.cell.class(if paused {
                "item paused".to_string()
            } else {
                "item".to_string()
            });
            cell.paused.hidden(!paused);
            let mut lines: Vec<String> = view
                .technology_data(item.name, player)
                .map(|data| tips::entity_names(&data))
                .into_iter()
                .collect();
            if paused {
                lines.push(lang::translate("This item is paused."));
            }
            cell.cell.tooltip(&lines.join("\n"));
            cell.cell.hidden(false);
        }
        self.root.hidden(shown.is_empty());
        self.shown = shown;
    }

    /// The sliders and countdowns at `turn`, the interpolated turn the
    /// frame shows.
    pub fn tick(&mut self, turn: f64) {
        for (item, cell) in self.shown.iter().zip(&mut self.cells) {
            let remaining = item.queue.remaining_ms(item.index, turn);
            let total = f64::from(item.queue.items[item.index].total) * 100.0;
            let progress = if total > 0.0 {
                (1.0 - remaining / total).clamp(0.0, 1.0)
            } else {
                0.0
            };
            cell.slider.style(format!(
                "top: {}px",
                SLIDER_INSET + (progress * SLIDER_SIDE).round()
            ));
            cell.time.text(&minutes_seconds(remaining / 1000.0));
        }
    }
}

/// `GetBasicInfoOfStartedTechs` for `player`. A technology is started
/// from its first `Progress`, which only the first item gets, and stays so
/// when paused or moved down the queue.
fn started(view: &View, player: i32) -> Vec<Started> {
    let scene = view.scene;
    if !scene.details.known || scene.details.match_id != scene.match_id {
        return Vec::new();
    }
    let mut started = Vec::new();
    for (&id, dot) in &scene.dots {
        if dot.owner != player || !scene.details.entities.contains_key(&id) {
            continue;
        }
        let Some(Group::Queue(queue)) = &entity_groups(scene, id, dot.template)[detail::QUEUE]
        else {
            continue;
        };
        for (index, item) in queue.items.iter().enumerate() {
            if item.technology && ((index == 0 && !item.paused) || item.remaining < item.total) {
                started.push(Started {
                    name: item.name,
                    producer: id,
                    index,
                    queue: queue.clone(),
                });
            }
        }
    }
    started
}
