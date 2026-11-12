//! The diplomacy window, a port of gui/session/diplomacy/
//! (`DiplomacyDialog`, `DiplomacyPlayerText`, `StanceButton`,
//! `DiplomacyDialogCeasefireCounter`) into the page's `#diplomacy-window`,
//! in the engine's pixels.
//!
//! The engine does not open it for observers. Here the Observer
//! perspective shows every player's stance towards every other as a
//! matrix; a player perspective shows the engine's rows. Nothing in it
//! can be pressed but Close: tribute, attack and spy requests and the
//! stance buttons send commands, which a viewer cannot.

use web_sys::Document;
use web_sys::HtmlElement;

use crate::TURN_MS;
use crate::diplomacy;
use crate::diplomacy::Perspective;
use crate::lang;
use crate::panel::Out;
use crate::panel::create;
use crate::panel::css_color;
use crate::stream::PlayerState;
use crate::stream::Scene;
use crate::stream::Stance;
use crate::tooltip;

/// `diplomacyDialogPanel`'s size with one player row.
const WIDTH: i32 = 560;
const HEIGHT: i32 = 170;
/// `diplomacyPlayer[n]`.
const ROW_HEIGHT: i32 = 20;
/// The rows' frame, `32 64 100%-32 384`.
const MARGIN: i32 = 32;
const ROWS_TOP: i32 = 64;
/// The Observer matrix's column per player, which the engine has not.
const MATRIX_COLUMN: i32 = 24;
/// `getPlayerColor(id, 32)`.
const ROW_ALPHA: f64 = 32.0 / 255.0;
/// The civ names `session-gui` writes.
const CIVS: &str = include_str!("../civs.txt");

/// What one update reads.
pub struct View<'a> {
    pub scene: &'a Scene,
    pub palette: &'a [u32],
    pub perspective: Perspective,
}

/// `#diplomacy-window`.
pub struct DiplomacyWindow {
    document: Document,
    panel: Out,
    title: HtmlElement,
    header: HtmlElement,
    rows: HtmlElement,
    ceasefire: Out,
    close: HtmlElement,
    /// What the header and rows were built from; `None` to build again.
    built: Option<String>,
}

impl DiplomacyWindow {
    pub fn new(document: &Document) -> Result<Self, String> {
        let mut window = DiplomacyWindow {
            document: document.clone(),
            panel: Out::find(document, "diplomacy-window")?,
            title: crate::element(document, "diplomacy-title")?,
            header: crate::element(document, "diplomacy-header")?,
            rows: crate::element(document, "diplomacy-rows")?,
            ceasefire: Out::find(document, "diplomacy-ceasefire")?,
            close: crate::element(document, "diplomacy-close")?,
            built: None,
        };
        window.panel.hidden(true);
        Ok(window)
    }

    pub fn close_button(&self) -> &HtmlElement {
        &self.close
    }

    pub fn is_open(&self) -> bool {
        !self.panel.element.hidden()
    }

    pub fn set_open(&mut self, open: bool) {
        self.panel.hidden(!open);
    }

    /// The language changed, so every label is made again.
    pub fn invalidate(&mut self) {
        self.built = None;
    }

    /// `updatePanels` while it is open: the rows when the players, the
    /// palette or the perspective changed, the ceasefire counter each time.
    pub fn update(&mut self, view: &View) {
        if !self.is_open() {
            return;
        }
        let key = build_key(view);
        if self.built.as_ref() != Some(&key) {
            if let Err(error) = self.build(view) {
                web_sys::console::error_1(&format!("diplomacy window: {error}").into());
            }
            self.built = Some(key);
        }
        self.show_ceasefire(view.scene);
    }

    fn build(&mut self, view: &View) -> Result<(), String> {
        let scene = view.scene;
        let players = scene.players.len().saturating_sub(1) as i32;
        let viewed = match view.perspective {
            Perspective::Player(id) if id < scene.players.len() => Some(id),
            _ => None,
        };
        let width = match viewed {
            Some(_) => WIDTH,
            None => WIDTH.max(2 * MARGIN + 300 + MATRIX_COLUMN * players),
        };
        let height = HEIGHT + ROW_HEIGHT * (players - 1).max(0);
        self.panel.style(format!(
            "--dialog-width: {width}px; --dialog-height: {height}px; --rows-top: {ROWS_TOP}px"
        ));
        lang::set_text(&self.title, "Diplomacy");
        lang::set_text(&self.close, "Close");
        self.header.set_text_content(None);
        self.rows.set_text_content(None);

        let header = &self.header;
        self.cell(header, 0, 140, &lang::translate("Name"), "center")?;
        self.cell(header, 150, 100, &lang::translate("Civilization"), "")?;
        self.cell(header, 250, 50, &lang::translate("Team"), "")?;
        match viewed {
            Some(_) => {
                self.cell(header, 300, 60, &lang::translate("Theirs"), "")?;
                for (i, (caption, stance)) in [("A", "Ally"), ("N", "Neutral"), ("E", "Enemy")]
                    .into_iter()
                    .enumerate()
                {
                    let cell = self.cell(
                        header,
                        360 + 20 * i as i32,
                        20,
                        &lang::translate(caption),
                        "",
                    )?;
                    set_tooltip(&cell, &lang::translate(stance), "sessionToolTipBold");
                }
            }
            None => {
                // A swatch of each player's displayed colour, named in its
                // tooltip, heads the player's column.
                for (column, player) in scene.players.iter().enumerate().skip(1) {
                    let cell = self.cell(
                        header,
                        300 + MATRIX_COLUMN * (column as i32 - 1),
                        MATRIX_COLUMN,
                        "",
                        "center",
                    )?;
                    let swatch: HtmlElement = create(&self.document, "span")?;
                    let _ = swatch.set_attribute("class", "swatch");
                    let _ = swatch.set_attribute(
                        "style",
                        &format!(
                            "background: {}",
                            css_color(view.palette.get(column).copied().unwrap_or(0), 1.0)
                        ),
                    );
                    cell.append_child(&swatch)
                        .map_err(|_| "cannot build a column header")?;
                    set_tooltip(
                        &cell,
                        &tooltip::colored(&player.name, &rgb(view.palette, column)),
                        "sessionToolTipBold",
                    );
                }
            }
        }

        for (id, player) in scene.players.iter().enumerate().skip(1) {
            let row: HtmlElement = create(&self.document, "div")?;
            let _ = row.set_attribute("class", "row");
            let _ = row.set_attribute(
                "style",
                &format!(
                    "top: {}px; background: {}",
                    ROW_HEIGHT * (id as i32 - 1),
                    css_color(view.palette.get(id).copied().unwrap_or(0), ROW_ALPHA)
                ),
            );
            self.rows
                .append_child(&row)
                .map_err(|_| "cannot add a player row")?;
            // `setOutcomeIcon`.
            let outcome = match player.state {
                PlayerState::Won => Some(("victory", "Victorious")),
                PlayerState::Defeated => Some(("defeat", "Defeated")),
                PlayerState::Active => None,
            };
            if let Some((icon, label)) = outcome {
                let image: HtmlElement = create(&self.document, "div")?;
                let _ = image.set_attribute("class", "outcome");
                let _ =
                    image.set_attribute("style", &format!("background-image: var(--gui-{icon})"));
                set_tooltip(&image, &lang::translate(label), "sessionToolTipBold");
                row.append_child(&image)
                    .map_err(|_| "cannot build a player row")?;
            }
            // `colorizePlayernameByID`.
            let name = self.cell(&row, 25, 125, &player.name, "")?;
            let _ = name.set_attribute(
                "style",
                &format!(
                    "{}; color: {}",
                    place(25, 125),
                    css_color(view.palette.get(id).copied().unwrap_or(0), 1.0)
                ),
            );
            self.cell(&row, 150, 100, &civ_name(&player.civ), "")?;
            let team = if player.team >= 0 {
                (player.team + 1).to_string()
            } else {
                lang::translate_context("team", "None")
            };
            self.cell(&row, 250, 50, &team, "")?;
            match viewed {
                Some(viewed) => self.stance_columns(&row, scene, id, viewed)?,
                None => {
                    for (column, _) in scene.players.iter().enumerate().skip(1) {
                        if column == id {
                            continue;
                        }
                        let stance = player.stances.get(column).copied().unwrap_or(Stance::Enemy);
                        // A seat is not told the stances between others.
                        if stance == Stance::Unknown {
                            continue;
                        }
                        let (caption, label, color) = stance_text(stance);
                        let cell = self.cell(
                            &row,
                            300 + MATRIX_COLUMN * (column as i32 - 1),
                            MATRIX_COLUMN,
                            &lang::translate(caption),
                            "center",
                        )?;
                        let _ = cell.set_attribute(
                            "style",
                            &format!(
                                "{}; color: {}",
                                place(300 + MATRIX_COLUMN * (column as i32 - 1), MATRIX_COLUMN),
                                css_color(color, 1.0)
                            ),
                        );
                        set_tooltip(&cell, &lang::translate(label), "sessionToolTipBold");
                    }
                }
            }
        }
        Ok(())
    }

    /// `diplomacyPlayerTheirs[n]` and the three `StanceButton`s, shown but
    /// never enabled, as for a viewer who does not control the player.
    fn stance_columns(
        &self,
        row: &HtmlElement,
        scene: &Scene,
        id: usize,
        viewed: usize,
    ) -> Result<(), String> {
        let Some(player) = scene.players.get(id) else {
            return Ok(());
        };
        if id != viewed {
            let theirs = match player.stances.get(viewed) {
                Some(Stance::Ally) => "Ally",
                Some(Stance::Neutral) => "Neutral",
                _ => "Enemy",
            };
            self.cell(row, 300, 60, &lang::translate(theirs), "")?;
        }
        // `isInactive` and `StanceButtonManager.update`'s hidden.
        let viewed_state = scene.players.get(viewed).map(|viewed| viewed.state);
        let inactive = id == viewed
            || viewed_state != Some(PlayerState::Active)
            || player.state != PlayerState::Active;
        if inactive || ceasefire_active(scene) {
            return Ok(());
        }
        let current = scene
            .players
            .get(viewed)
            .and_then(|viewed| viewed.stances.get(id))
            .copied()
            .unwrap_or(Stance::Enemy);
        for (i, stance) in [Stance::Ally, Stance::Neutral, Stance::Enemy]
            .into_iter()
            .enumerate()
        {
            let caption = if stance == current {
                lang::translate_context("diplomatic stance selection", "x")
            } else {
                String::new()
            };
            let button = self.cell(row, 360 + 20 * i as i32, 20, &caption, "center")?;
            let _ = button.set_attribute("class", "text center sans-bold-stroke-14 stone disabled");
        }
        Ok(())
    }

    /// `DiplomacyDialogCeasefireCounter.update`.
    fn show_ceasefire(&mut self, scene: &Scene) {
        let remaining = ceasefire_remaining(scene);
        self.ceasefire.hidden(remaining.is_none());
        if let Some(remaining) = remaining {
            self.ceasefire.text(&lang::format(
                &lang::translate_context("ceasefire", "Remaining ceasefire time: %(time)s."),
                &[("time", &time_to_string(remaining))],
            ));
        }
    }

    /// A `DiplomacyText` cell of `parent` at `left`, `width` px wide.
    fn cell(
        &self,
        parent: &HtmlElement,
        left: i32,
        width: i32,
        text: &str,
        align: &str,
    ) -> Result<HtmlElement, String> {
        let cell: HtmlElement = create(&self.document, "div")?;
        let _ = cell.set_attribute("class", &format!("text sans-bold-stroke-14 {align}"));
        let _ = cell.set_attribute("style", &place(left, width));
        cell.set_text_content(Some(text));
        parent
            .append_child(&cell)
            .map_err(|_| "cannot build the diplomacy window")?;
        Ok(cell)
    }
}

fn place(left: i32, width: i32) -> String {
    format!("left: {left}px; width: {width}px")
}

fn set_tooltip(element: &HtmlElement, text: &str, style: &str) {
    let _ = element.set_attribute(tooltip::ATTRIBUTE, text);
    let _ = element.set_attribute(tooltip::STYLE_ATTRIBUTE, style);
}

/// The engine markup colour of player `id`'s displayed colour.
fn rgb(palette: &[u32], id: usize) -> String {
    let [_, r, g, b] = palette.get(id).copied().unwrap_or(0).to_be_bytes();
    format!("{r} {g} {b}")
}

/// The matrix cell: the stance header's letter, its tooltip, and the
/// default diplomacy colour of that stance, so the matrix reads at a
/// glance.
fn stance_text(stance: Stance) -> (&'static str, &'static str, u32) {
    match stance {
        Stance::Ally => ("A", "Ally", diplomacy::ALLY),
        Stance::Neutral => ("N", "Neutral", diplomacy::NEUTRAL),
        Stance::Enemy | Stance::Unknown => ("E", "Enemy", diplomacy::ENEMY),
    }
}

/// `g_CivData[civ].Name`, translated as the engine's `loadCivData` does.
pub(crate) fn civ_name(civ: &str) -> String {
    CIVS.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('\t'))
        .find(|(code, _)| *code == civ)
        .map_or_else(|| civ.to_string(), |(_, name)| lang::translate(name))
}

fn scene_ms(scene: &Scene) -> f64 {
    f64::from(scene.turn) * TURN_MS
}

/// The ceasefire's time left in ms, `None` without one.
fn ceasefire_remaining(scene: &Scene) -> Option<f64> {
    let remaining = f64::from(scene.ceasefire_end) - scene_ms(scene);
    (scene.ceasefire_end != 0 && remaining > 0.0).then_some(remaining)
}

/// `GetSimState().ceasefireActive`.
fn ceasefire_active(scene: &Scene) -> bool {
    ceasefire_remaining(scene).is_some()
}

/// `timeToString`: `mm:ss` under an hour, else `HH:mm:ss`.
pub(crate) fn time_to_string(ms: f64) -> String {
    let seconds = (ms / 1000.0) as u64;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Everything the rows show, so they are built again only when it changes;
/// the ceasefire is in it as the stance buttons hide during one.
fn build_key(view: &View) -> String {
    let mut key = format!("{:?}|{}|", view.palette, ceasefire_active(view.scene));
    if let Perspective::Player(id) = view.perspective {
        key.push_str(&id.to_string());
    }
    for player in &view.scene.players {
        let stances: Vec<u8> = player.stances.iter().map(|&stance| stance as u8).collect();
        key.push_str(&format!(
            "|{:?}|{}|{}|{:?}|{}",
            player.name, player.team, player.state as u8, stances, player.civ
        ));
    }
    key
}
