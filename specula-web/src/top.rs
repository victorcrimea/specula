//! The top panel, a port of gui/session/top_panel/ (`CounterManager`,
//! `CounterResource`, `CounterPopulation`) and of the hero buttons
//! (`PanelEntityManager`, `PanelEntity`), drawn into the page's
//! `#top-panel`. The engine's tooltips are left out.
//!
//! The engine hides the counters from observers and lists every player's
//! values in their tooltip; here the Observer perspective shows one row of
//! counters per player instead.

use std::cell::RefCell;

use web_sys::Document;
use web_sys::HtmlElement;

use crate::diplomacy::Perspective;
use crate::panel::BAND_ALPHA;
use crate::panel::CaptureBar;
use crate::panel::Out;
use crate::panel::Request;
use crate::panel::create;
use crate::panel::css_color;
use crate::panel::string;
use crate::panel::template_details;
use crate::pick;
use crate::stream::Economy;
use crate::stream::PlayerState;
use crate::stream::Scene;

/// `g_ResourceData.GetCodes()` for the public mod, in its order.
const RESOURCES: [&str; 4] = ["food", "wood", "stone", "metal"];
/// `horizontallySpaceObjects("resourceCounts", 5)`: 73 px counters with
/// the counter count as the margin.
const COUNTER_STEP: usize = 78;
const COUNTER_MARGIN: usize = 5;
/// The `panelEntityButton[n]` repeat.
const MAX_HEROES: usize = 19;
/// `g_PanelEntityOrder`.
const HERO_ORDER: [&str; 2] = ["Hero", "Relic"];
/// `coloredText`'s gatherer colours: `gold` (gui/common/setup.xml) and
/// `200 200 200` at zero.
const GATHERERS: &str = "rgb(237, 227, 167)";
const GATHERERS_ZERO: &str = "rgb(200, 200, 200)";
/// `startColorFade(name, 100, 0, colorFade_attackUnit, ...)` and
/// `g_FadeAttackUnit` (gui/common/colorFades.js).
const FADE_TICK_MS: f64 = 100.0;
const FADE_BLINKING_TICKS: u32 = 50;
const FADE_BLINKING_INTERVAL: u32 = 5;
const FADE_GB_RATE: f64 = 3.0;
const FADE_OUT_START: i32 = 100;
const FADE_OPACITY_RATE: i32 = 3;

/// One `resource[n]` counter: its count and gatherers.
struct Counter {
    count: Out,
    stats: Out,
}

impl Counter {
    /// The `i`-th counter in `parent`, showing `icon` (`resource-<code>`).
    fn new(
        document: &Document,
        parent: &HtmlElement,
        i: usize,
        icon: &str,
    ) -> Result<Self, String> {
        let counter: HtmlElement = create(document, "div")?;
        let _ = counter.set_attribute("class", "counter");
        let _ = counter.set_attribute(
            "style",
            &format!("left: {}px", i * COUNTER_STEP + COUNTER_MARGIN),
        );
        let image: HtmlElement = create(document, "div")?;
        let _ = image.set_attribute("class", "icon");
        let _ = image.set_attribute("style", &format!("background-image: var(--gui-{icon})"));
        let count: HtmlElement = create(document, "div")?;
        let _ = count.set_attribute("class", "count text sans-bold-stroke-14");
        let stats: HtmlElement = create(document, "div")?;
        let _ = stats.set_attribute("class", "gatherers text sans-bold-stroke-13");
        for child in [&image, &count, &stats] {
            counter
                .append_child(child)
                .map_err(|_| "cannot build a counter")?;
        }
        parent
            .append_child(&counter)
            .map_err(|_| "cannot add a counter")?;
        Ok(Counter {
            count: Out::new(count),
            stats: Out::new(stats),
        })
    }

    fn show(&mut self, count: &str, gatherers: u32) {
        self.count.text(count);
        self.stats.text(&gatherers.to_string());
        self.stats.style(format!(
            "color: {}",
            if gatherers > 0 {
                GATHERERS
            } else {
                GATHERERS_ZERO
            }
        ));
    }
}

/// The resource counters and the population counter, in that order.
struct Counters(Vec<Counter>);

impl Counters {
    fn new(document: &Document, parent: &HtmlElement) -> Result<Self, String> {
        let mut counters = Vec::new();
        for (i, code) in RESOURCES.iter().enumerate() {
            counters.push(Counter::new(
                document,
                parent,
                i,
                &format!("resource-{code}"),
            )?);
        }
        counters.push(Counter::new(
            document,
            parent,
            RESOURCES.len(),
            "resource-population",
        )?);
        Ok(Counters(counters))
    }

    /// `CounterResource.rebuild` and `CounterPopulation.rebuild`.
    fn show(&mut self, scene: &Scene, economy: &Economy) {
        let resource = |code: &str| {
            economy
                .resources
                .iter()
                .find(|resource| string(scene, resource.code) == Some(code))
        };
        for (counter, code) in self.0.iter_mut().zip(RESOURCES) {
            let (amount, gatherers) =
                resource(code).map_or((0, 0), |resource| (resource.amount, resource.gatherers));
            counter.show(&abbreviate(amount), gatherers);
        }
        let gatherers = economy
            .resources
            .iter()
            .map(|resource| resource.gatherers)
            .sum();
        if let Some(population) = self.0.last_mut() {
            population.show(
                &format!(
                    "{}/{} ({})",
                    economy.population, economy.population_limit, economy.population_max
                ),
                gatherers,
            );
        }
    }
}

/// An Observer row: the player's colour band with the phase emblem and
/// name, as the details panel's civ band, then the player's counters.
struct PlayerRow {
    row: Out,
    color: Out,
    phase: Out,
    name: Out,
    counters: Counters,
}

impl PlayerRow {
    fn new(document: &Document, parent: &HtmlElement, i: usize) -> Result<Self, String> {
        let row: HtmlElement = create(document, "div")?;
        let _ = row.set_attribute("class", "row");
        let _ = row.set_attribute("style", &format!("--row: {i}"));
        let band: HtmlElement = create(document, "div")?;
        let _ = band.set_attribute("class", "band");
        let color: HtmlElement = create(document, "div")?;
        let _ = color.set_attribute("class", "fill");
        let phase: HtmlElement = create(document, "div")?;
        let _ = phase.set_attribute("class", "fill phase");
        let name: HtmlElement = create(document, "div")?;
        let _ = name.set_attribute("class", "name text center sans-bold-stroke-14");
        for child in [&color, &phase, &name] {
            band.append_child(child)
                .map_err(|_| "cannot build a player band")?;
        }
        let counters: HtmlElement = create(document, "div")?;
        let _ = counters.set_attribute("class", "counters");
        for child in [&band, &counters] {
            row.append_child(child)
                .map_err(|_| "cannot build a player row")?;
        }
        parent
            .append_child(&row)
            .map_err(|_| "cannot add a player row")?;
        Ok(PlayerRow {
            counters: Counters::new(document, &counters)?,
            row: Out::new(row),
            color: Out::new(color),
            phase: Out::new(phase),
            name: Out::new(name),
        })
    }
}

/// One `panelEntityButton[n]`.
struct HeroButton {
    button: Out,
    icon: Out,
    hit: Out,
    health: Out,
    health_bar: Out,
    capture: Out,
    capture_bar: CaptureBar,
}

impl HeroButton {
    fn new(document: &Document, parent: &HtmlElement) -> Result<Self, String> {
        let button: HtmlElement = create(document, "button")?;
        let _ = button.set_attribute("type", "button");
        let _ = button.set_attribute("class", "hero");
        let icon: HtmlElement = create(document, "img")?;
        let _ = icon.set_attribute("alt", "");
        let hit: HtmlElement = create(document, "div")?;
        let _ = hit.set_attribute("class", "hit");
        let health: HtmlElement = create(document, "div")?;
        let _ = health.set_attribute("class", "bar health");
        let health_bar: HtmlElement = create(document, "div")?;
        let _ = health_bar.set_attribute("class", "health-fg");
        for class in ["border", "health-bg"] {
            let part: HtmlElement = create(document, "div")?;
            let _ = part.set_attribute("class", class);
            health
                .append_child(&part)
                .map_err(|_| "cannot build a health bar")?;
        }
        let shine: HtmlElement = create(document, "div")?;
        let _ = shine.set_attribute("class", "shine horizontal");
        for part in [&health_bar, &shine] {
            health
                .append_child(part)
                .map_err(|_| "cannot build a health bar")?;
        }
        let capture: HtmlElement = create(document, "div")?;
        let _ = capture.set_attribute("class", "bar capture");
        for class in ["border", "segments", "shine horizontal"] {
            let part: HtmlElement = create(document, "div")?;
            let _ = part.set_attribute("class", class);
            capture
                .append_child(&part)
                .map_err(|_| "cannot build a capture bar")?;
        }
        for child in [&icon, &hit, &health, &capture] {
            button
                .append_child(child)
                .map_err(|_| "cannot build a hero button")?;
        }
        parent
            .append_child(&button)
            .map_err(|_| "cannot add a hero button")?;
        let mut hero = HeroButton {
            capture_bar: CaptureBar::new(document, capture.clone())?,
            button: Out::new(button),
            icon: Out::new(icon),
            hit: Out::new(hit),
            health: Out::new(health),
            health_bar: Out::new(health_bar),
            capture: Out::new(capture),
        };
        hero.button.hidden(true);
        Ok(hero)
    }
}

/// `colorFade_attackUnit` on a button's hit overlay: a red blink, then a
/// fade through grey to nothing.
struct Fade {
    ticks: u32,
    r: i32,
    g: i32,
    /// Opacity in 1/255.
    o: i32,
    /// When the next tick runs, on the frame clock.
    next: f64,
}

impl Fade {
    fn new(now: f64) -> Self {
        Fade {
            ticks: 0,
            r: 0,
            g: 0,
            o: 100,
            next: now,
        }
    }

    /// `restartColorFade` with `smoothColorFadeRestart_attackUnit`: in the
    /// blinking phase it keeps blinking, else it starts over.
    fn restart(&mut self) {
        if self.ticks < FADE_BLINKING_TICKS {
            self.ticks %= FADE_BLINKING_INTERVAL * 2;
        } else {
            self.ticks = 0;
            self.r = 0;
            self.g = 0;
            self.o = 100;
        }
    }

    /// One `fadeColorTick`; false once the fade is over.
    fn step(&mut self) -> bool {
        if self.ticks == 0 {
            self.r = 175;
        }
        if self.ticks < FADE_BLINKING_TICKS {
            if self.ticks.is_multiple_of(FADE_BLINKING_INTERVAL) {
                self.g = if self.g == 0 { 255 } else { 0 };
            }
        } else if self.ticks >= FADE_BLINKING_TICKS + FADE_BLINKING_INTERVAL {
            let since = f64::from(self.ticks - FADE_BLINKING_TICKS);
            self.g = (self.g + (FADE_GB_RATE * since.sqrt()).round() as i32).min(255);
            if self.g > FADE_OUT_START {
                self.o = if self.o > FADE_OPACITY_RATE {
                    self.o - FADE_OPACITY_RATE
                } else {
                    0
                };
            }
            if self.o == 0 {
                return false;
            }
        }
        self.ticks += 1;
        true
    }

    fn color(&self) -> String {
        format!(
            "background: rgba({}, {}, {}, {:.3})",
            self.r,
            self.g,
            self.g,
            f64::from(self.o) / 255.0
        )
    }
}

/// A `PanelEntity`: what its button last showed, kept while the entity is
/// out of the world (garrisoned), as `GetEntityState` still has it then.
struct Hero {
    id: u32,
    order_key: i32,
    icon: Option<String>,
    owner: i32,
    /// Stream 1's fraction in 1/255; 0 without Health.
    hitpoints: u8,
    /// Each player's share in 1/255; `None` without Capturable.
    capture: Option<Vec<u8>>,
    fade: Option<Fade>,
}

/// What one update reads.
pub struct View<'a> {
    pub scene: &'a Scene,
    pub palette: &'a [u32],
    pub perspective: Perspective,
    /// Identity's Icon -> where an `<img>` finds its portrait.
    pub portrait: &'a dyn Fn(&str) -> Option<String>,
}

/// The `#top-panel`. Its buttons ask for selection and camera changes,
/// which the page takes with `take_requests`.
pub struct TopPanel {
    document: Document,
    /// The page's root, which carries the row count for the layout of
    /// everything under the bar.
    root: Option<web_sys::Element>,
    rows_shown: Option<usize>,
    counters_box: Out,
    counters: Counters,
    players_box: Out,
    players_parent: HtmlElement,
    players: Vec<PlayerRow>,
    buttons: Vec<HeroButton>,
    heroes: Vec<Hero>,
    requests: RefCell<Vec<Request>>,
}

impl TopPanel {
    pub fn new(document: &Document) -> Result<Self, String> {
        let counters_parent: HtmlElement = crate::element(document, "top-counters")?;
        let players_parent: HtmlElement = crate::element(document, "top-players")?;
        let heroes_parent: HtmlElement = crate::element(document, "heroes")?;
        let buttons = (0..MAX_HEROES)
            .map(|_| HeroButton::new(document, &heroes_parent))
            .collect::<Result<_, _>>()?;
        Ok(TopPanel {
            document: document.clone(),
            root: document.document_element(),
            rows_shown: None,
            counters_box: Out::new(counters_parent.clone()),
            counters: Counters::new(document, &counters_parent)?,
            players_box: Out::new(players_parent.clone()),
            players_parent,
            players: Vec::new(),
            buttons,
            heroes: Vec::new(),
            requests: RefCell::new(Vec::new()),
        })
    }

    /// The hero buttons, for the page to wire.
    pub fn hero_buttons(&self) -> impl Iterator<Item = &HtmlElement> {
        self.buttons.iter().map(|button| &button.button.element)
    }

    /// `PanelEntity.onPress`; `add` for the selection.add modifier (Shift).
    pub fn hero_clicked(&self, index: usize, add: bool) {
        if let Some(hero) = self.heroes.get(index) {
            self.requests
                .borrow_mut()
                .push(Request::Select { id: hero.id, add });
        }
    }

    /// `PanelEntity.onDoublePress`.
    pub fn hero_double_clicked(&self, index: usize) {
        if let Some(hero) = self.heroes.get(index) {
            self.requests
                .borrow_mut()
                .push(Request::SelectAndFollow(hero.id));
        }
    }

    pub fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.borrow_mut())
    }

    /// `CounterManager.rebuild` and `PanelEntityManager.update`; `now` is
    /// the frame clock, for the attack fades.
    pub fn update(&mut self, view: &View, now: f64) {
        let scene = view.scene;
        let none = Economy::default();
        let rows = match view.perspective {
            Perspective::Player(id) => {
                self.counters
                    .show(scene, scene.economy.get(id).unwrap_or(&none));
                0
            }
            Perspective::Observer => {
                let count = scene.players.len().saturating_sub(1);
                if let Err(error) = self.show_players(view, count) {
                    web_sys::console::error_1(&format!("top panel: {error}").into());
                }
                count.max(1)
            }
        };
        self.counters_box.hidden(rows > 0);
        self.players_box.hidden(rows == 0);
        if self.rows_shown != Some(rows) {
            self.rows_shown = Some(rows);
            if let Some(root) = &self.root {
                // Nothing else styles the root.
                let _ = root.set_attribute("style", &format!("--top-rows: {rows}"));
            }
        }
        self.update_heroes(view, now);
    }

    /// The attack fades, every frame.
    pub fn tick(&mut self, now: f64) {
        for (hero, button) in self.heroes.iter_mut().zip(&mut self.buttons) {
            let Some(fade) = &mut hero.fade else {
                continue;
            };
            let mut running = true;
            while running && now >= fade.next {
                running = fade.step();
                fade.next += FADE_TICK_MS;
            }
            if running {
                button.hit.style(fade.color());
            } else {
                hero.fade = None;
            }
            button.hit.hidden(!running);
        }
    }

    fn show_players(&mut self, view: &View, count: usize) -> Result<(), String> {
        let scene = view.scene;
        while self.players.len() < count {
            let row = PlayerRow::new(&self.document, &self.players_parent, self.players.len())?;
            self.players.push(row);
        }
        let none = Economy::default();
        for (index, row) in self.players.iter_mut().enumerate() {
            let id = index + 1;
            let Some(player) = scene.players.get(id) else {
                row.row.hidden(true);
                continue;
            };
            row.row.hidden(false);
            row.row.class(if player.state == PlayerState::Defeated {
                "row defeated".to_string()
            } else {
                "row".to_string()
            });
            let color = view.palette.get(id).copied().unwrap_or(0);
            row.color
                .style(format!("background: {}", css_color(color, BAND_ALPHA)));
            let economy = scene.economy.get(id).unwrap_or(&none);
            let phase = string(scene, economy.phase).unwrap_or_default();
            row.phase.style(if phase.is_empty() {
                String::new()
            } else {
                format!("background-image: var(--gui-phase-{phase})")
            });
            row.name.text(&player.name);
            row.counters.show(scene, economy);
        }
        Ok(())
    }

    fn update_heroes(&mut self, view: &View, now: f64) {
        let scene = view.scene;
        let ids: Vec<u32> = match view.perspective {
            Perspective::Observer => scene
                .economy
                .iter()
                .flat_map(|economy| economy.panel_entities.iter().copied())
                .collect(),
            Perspective::Player(id) => scene
                .economy
                .get(id)
                .map(|economy| economy.panel_entities.clone())
                .unwrap_or_default(),
        };
        self.heroes.retain(|hero| ids.contains(&hero.id));
        for &id in &ids {
            if self.heroes.iter().all(|hero| hero.id != id) {
                self.insert_if_relevant(view, id);
            }
        }
        for hero in &mut self.heroes {
            refresh(view, hero, now);
        }
        for (index, button) in self.buttons.iter_mut().enumerate() {
            match self.heroes.get(index) {
                Some(hero) => show_hero(view, hero, button),
                None => {
                    button.hit.hidden(true);
                    button.button.hidden(true);
                }
            }
        }
    }

    /// `PanelEntityManager.insertIfRelevant`. An entity out of the world
    /// has no template here yet, so it waits until it comes out.
    fn insert_if_relevant(&mut self, view: &View, id: u32) {
        let scene = view.scene;
        let Some(dot) = scene.dots.get(&id) else {
            return;
        };
        let template = template_details(scene, dot.template);
        let order_key = HERO_ORDER
            .iter()
            .position(|class| {
                template.is_some_and(|template| pick::has_class(template, &scene.strings, class))
            })
            .map_or(-1, |key| key as i32);
        let at = self
            .heroes
            .iter()
            .filter(|hero| hero.order_key <= order_key)
            .count();
        if at >= MAX_HEROES {
            return;
        }
        self.heroes.truncate(MAX_HEROES - 1);
        self.heroes.insert(
            at,
            Hero {
                id,
                order_key,
                icon: template
                    .and_then(|template| string(scene, template.icon))
                    .and_then(|icon| (view.portrait)(icon)),
                owner: dot.owner,
                hitpoints: dot.hitpoints,
                capture: scene.captures.get(&id).cloned(),
                fade: None,
            },
        );
    }
}

/// `PanelEntity.update`'s state: the bars and `onAttacked`.
fn refresh(view: &View, hero: &mut Hero, now: f64) {
    let scene = view.scene;
    let Some(dot) = scene.dots.get(&hero.id) else {
        return;
    };
    // The portrait index may load after the button was made.
    if hero.icon.is_none() {
        hero.icon = template_details(scene, dot.template)
            .and_then(|template| string(scene, template.icon))
            .and_then(|icon| (view.portrait)(icon));
    }
    let owner = usize::try_from(dot.owner).ok();
    let capture = scene.captures.get(&hero.id);
    let owned = |shares: Option<&Vec<u8>>| {
        owner
            .and_then(|owner| shares.and_then(|shares| shares.get(owner)))
            .copied()
            .unwrap_or(0)
    };
    let attacked = dot.hitpoints < hero.hitpoints
        || (hero.capture.is_some() && owned(capture) < owned(hero.capture.as_ref()));
    hero.owner = dot.owner;
    hero.hitpoints = dot.hitpoints;
    hero.capture = capture.cloned();
    if attacked {
        match &mut hero.fade {
            Some(fade) => fade.restart(),
            None => hero.fade = Some(Fade::new(now)),
        }
    }
}

fn show_hero(view: &View, hero: &Hero, button: &mut HeroButton) {
    button.icon.src(hero.icon.clone());
    let has_health = hero.hitpoints > 0;
    button.health.hidden(!has_health);
    if has_health {
        button.health_bar.style(format!(
            "width: {}%",
            100.0 * f64::from(hero.hitpoints) / 255.0
        ));
    }
    button.capture.hidden(hero.capture.is_none());
    if let Some(shares) = &hero.capture {
        // Without health the capture bar takes its place.
        button.capture.class(if has_health {
            "bar capture".to_string()
        } else {
            "bar capture top".to_string()
        });
        let owner = usize::try_from(hero.owner).ok();
        let mut parts: Vec<(usize, f64)> = owner
            .and_then(|owner| shares.get(owner).map(|share| (owner, f64::from(*share))))
            .into_iter()
            .collect();
        parts.extend(
            shares
                .iter()
                .enumerate()
                .filter(|(player, _)| Some(*player) != owner)
                .map(|(player, share)| (player, f64::from(*share))),
        );
        button.capture_bar.show(&parts, 255.0, view.palette, false);
    }
    if hero.fade.is_none() {
        button.hit.hidden(true);
    }
    button.button.hidden(false);
}

/// `abbreviateLargeNumbers`.
fn abbreviate(number: u32) -> String {
    if number >= 1_000_000 {
        format!("{}M", number / 1_000_000)
    } else if number >= 100_000 {
        format!("{}k", number / 1000)
    } else if number >= 10_000 {
        let text = format!("{:.1}", f64::from(number) / 1000.0);
        format!("{}k", text.strip_suffix(".0").unwrap_or(&text))
    } else {
        number.to_string()
    }
}
