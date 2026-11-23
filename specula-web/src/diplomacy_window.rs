//! The diplomacy window, a port of gui/session/diplomacy/
//! (`DiplomacyDialog`, `DiplomacyPlayerText`, `StanceButton`,
//! `AttackRequestButton`, `SpyRequestButton`,
//! `DiplomacyDialogCeasefireCounter`) into the page's `#diplomacy-window`,
//! in the engine's pixels.
//!
//! The engine does not open it for observers. Here the Observer
//! perspective shows every player's stance towards every other as a
//! matrix; a player perspective shows the engine's rows. The seat that
//! controls the viewed player can change the stance, tribute, request an
//! attack and bribe a unit; others see them disabled. Kick and Ban are for
//! the host (M29, D54).

use std::cell::Cell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::Document;
use web_sys::HtmlButtonElement;
use web_sys::HtmlElement;

use crate::TURN_MS;
use crate::barter::CODES;
use crate::detail::Control;
use crate::detail::ControlSeat;
use crate::detail::PlayerGui;
use crate::diplomacy;
use crate::diplomacy::Perspective;
use crate::lang;
use crate::panel;
use crate::panel::Out;
use crate::panel::create;
use crate::panel::css_color;
use crate::stream::PlayerState;
use crate::stream::Scene;
use crate::stream::Stance;
use crate::tips;
use crate::tips::DataFiles;
use crate::tooltip;
use specula_assets::command::Command;
use specula_assets::command::DIPLOMACY_ALLY;
use specula_assets::command::DIPLOMACY_ENEMY;
use specula_assets::command::DIPLOMACY_NEUTRAL;

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
/// diplomacyPlayer[n]_tribute at 430, one 20 px iconButton per tributable
/// resource.
const TRIBUTE_LEFT: i32 = 430;
const TRIBUTE_BUTTON: i32 = 20;
/// TributeButton.prototype.DefaultAmount and MassAmount.
const DEFAULT_AMOUNT: u32 = 100;
const MASS_AMOUNT: u32 = 500;
/// diplomacyPlayerAlly/Neutral/Enemy at 360, 380 and 400.
const STANCE_LEFT: i32 = 360;
const STANCE_BUTTON: i32 = 20;
/// diplomacyAttackRequest and diplomacySpyRequest at 100%-58 and 100%-30 of
/// the 576 px row of the 640 px dialog; REQUEST_BUTTON is their 20 px.
const ATTACK_LEFT: i32 = 518;
const SPY_LEFT: i32 = 546;
const REQUEST_BUTTON: i32 = 20;
/// Kick and Ban after the request column, which the engine has not.
const KICK_LEFT: i32 = 580;
const BAN_LEFT: i32 = 640;
/// Player-perspective widths: diplomacyDialogPanel widened by the tribute
/// and request columns, and room for Kick and Ban.
const TRIBUTE_WIDTH: i32 = 640;
const HOST_WIDTH: i32 = 760;
/// `SpyRequestButton.prototype.TemplateName`.
const SPY_TEMPLATE: &str = "special/spy";
/// The civ names `session-gui` writes.
const CIVS: &str = include_str!("../civs.txt");

/// What one update reads.
pub struct View<'a> {
    pub scene: &'a Scene,
    pub palette: &'a [u32],
    pub perspective: Perspective,
    /// The seats' control, and the visitor's own seat on `/play`.
    pub control: Option<&'a Control>,
    pub own: Option<u8>,
    /// The JSON the data table names.
    pub data: &'a DataFiles,
}

/// A spy request button of the current build, refreshed each update.
struct SpyButton {
    target: u8,
    button: HtmlButtonElement,
    icon: HtmlElement,
    tint: HtmlElement,
    tooltip: String,
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
    /// The player whose Kick was pressed, until `take_kick` takes it.
    kick_request: Rc<Cell<Option<u8>>>,
    /// The player whose Ban was pressed, until `take_ban` takes it.
    ban_request: Rc<Cell<Option<u8>>>,
    /// The Kick and Ban buttons' handlers, which live as long as their
    /// buttons.
    kick_handlers: Vec<Closure<dyn FnMut()>>,
    /// The (receiver, resource index into `CODES`, Shift held) of the last
    /// tribute button press, until `take_tributes` handles it.
    tribute_request: Rc<Cell<Option<(u8, usize, bool)>>>,
    /// `TributeButton.amount` per receiver and resource. Kept across
    /// rebuilds, as vanilla's buttons live all session.
    amounts: Vec<[u32; 4]>,
    /// The tribute buttons of the current build, to refresh a tooltip
    /// without a rebuild.
    tribute_buttons: Vec<(u8, usize, HtmlElement)>,
    tribute_handlers: Vec<Closure<dyn FnMut(web_sys::MouseEvent)>>,
    /// The row player and the `DIPLOMACY_*` code of the last stance press,
    /// until `take_requests` takes it.
    stance_request: Rc<Cell<Option<(u8, u8)>>>,
    /// The player of the last attack request press.
    attack_request: Rc<Cell<Option<u8>>>,
    /// The player of the last spy request press.
    spy_request: Rc<Cell<Option<u8>>>,
    /// The stance, attack and spy buttons' handlers, which live as long as
    /// their buttons.
    request_handlers: Vec<Closure<dyn FnMut()>>,
    /// `SpyRequestButton.spyRequests` of the seat's player: the targets
    /// whose bribe was sent and not yet answered.
    spy_pending: Vec<u8>,
    /// The `scene.match_id` `spy_pending` belongs to.
    spy_match: u64,
    /// The spy buttons of the current build, refreshed every update.
    spy_buttons: Vec<SpyButton>,
    /// `controlsPlayer(g_ViewedPlayer)` at the last build.
    controls: bool,
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
            kick_request: Rc::new(Cell::new(None)),
            ban_request: Rc::new(Cell::new(None)),
            kick_handlers: Vec::new(),
            tribute_request: Rc::new(Cell::new(None)),
            amounts: Vec::new(),
            tribute_buttons: Vec::new(),
            tribute_handlers: Vec::new(),
            stance_request: Rc::new(Cell::new(None)),
            attack_request: Rc::new(Cell::new(None)),
            spy_request: Rc::new(Cell::new(None)),
            request_handlers: Vec::new(),
            spy_pending: Vec::new(),
            spy_match: 0,
            spy_buttons: Vec::new(),
            controls: false,
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

    /// The player whose Kick was pressed since the last call.
    pub fn take_kick(&self) -> Option<u8> {
        self.kick_request.take()
    }

    /// The player whose Ban was pressed since the last call.
    pub fn take_ban(&self) -> Option<u8> {
        self.ban_request.take()
    }

    /// `TributeButton.onPress`: a Shift-click raises the amount, a plain
    /// click sends it and starts again at the default.
    pub fn take_tributes(&mut self, scene: &Scene, palette: &[u32]) -> Option<Command> {
        let (player, index, shift) = self.tribute_request.take()?;
        if !self.controls {
            return None;
        }
        let amount = self
            .amounts
            .get_mut(usize::from(player))
            .map(|amounts| &mut amounts[index])?;
        let command = if shift {
            *amount = next_amount(*amount);
            None
        } else {
            let sent = *amount;
            *amount = DEFAULT_AMOUNT;
            Some(Command::Tribute {
                player,
                resource: CODES[index].to_string(),
                amount: sent,
            })
        };
        self.refresh_tribute_tooltip(scene, palette, player, index);
        command
    }

    /// `onMassTributeRelease`: every amount raised to a mass amount is
    /// sent, whether or not the window is open, as vanilla's hotkey objects
    /// fire hidden.
    pub fn mass_release(&mut self, scene: &Scene, palette: &[u32]) -> Vec<Command> {
        let mut commands = Vec::new();
        let mut reset = Vec::new();
        for (player, amounts) in self.amounts.iter_mut().enumerate() {
            for (index, amount) in amounts.iter_mut().enumerate() {
                if *amount >= MASS_AMOUNT {
                    commands.push(Command::Tribute {
                        player: player as u8,
                        resource: CODES[index].to_string(),
                        amount: *amount,
                    });
                    *amount = DEFAULT_AMOUNT;
                    reset.push((player as u8, index));
                }
            }
        }
        for (player, index) in reset {
            self.refresh_tribute_tooltip(scene, palette, player, index);
        }
        commands
    }

    fn refresh_tribute_tooltip(&self, scene: &Scene, palette: &[u32], player: u8, index: usize) {
        let amount = self
            .amounts
            .get(usize::from(player))
            .map_or(DEFAULT_AMOUNT, |amounts| amounts[index]);
        for (id, resource, button) in &self.tribute_buttons {
            if *id == player && *resource == index {
                set_tooltip(
                    button,
                    &tribute_tooltip(scene, palette, usize::from(player), CODES[index], amount),
                    "sessionToolTipBold",
                );
            }
        }
    }

    /// `updatePanels` while it is open: the rows when the players, the
    /// palette or the perspective changed, the ceasefire counter each time.
    pub fn update(&mut self, view: &View) {
        if !self.is_open() {
            return;
        }
        if self.spy_match != view.scene.match_id {
            self.spy_match = view.scene.match_id;
            self.spy_pending.clear();
        }
        let key = build_key(view);
        if self.built.as_ref() != Some(&key) {
            if let Err(error) = self.build(view) {
                web_sys::console::error_1(&format!("diplomacy window: {error}").into());
            }
            self.built = Some(key);
        }
        self.refresh_spy(view);
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
            Some(_) if viewer_is_host(view) => HOST_WIDTH,
            Some(_) => TRIBUTE_WIDTH,
            None if viewer_is_host(view) => {
                HOST_WIDTH.max(2 * MARGIN + 300 + MATRIX_COLUMN * players)
            }
            None => WIDTH.max(2 * MARGIN + 300 + MATRIX_COLUMN * players),
        };
        self.controls = matches!(viewed, Some(v) if view.own.map(usize::from) == Some(v));
        if self.amounts.len() < scene.players.len() {
            self.amounts
                .resize(scene.players.len(), [DEFAULT_AMOUNT; 4]);
        }
        let height = HEIGHT + ROW_HEIGHT * (players - 1).max(0);
        self.panel.style(format!(
            "--dialog-width: {width}px; --dialog-height: {height}px; --rows-top: {ROWS_TOP}px"
        ));
        lang::set_text(&self.title, "Diplomacy");
        lang::set_text(&self.close, "Close");
        self.header.set_text_content(None);
        self.rows.set_text_content(None);
        self.kick_handlers.clear();
        self.tribute_handlers.clear();
        self.tribute_buttons.clear();
        self.request_handlers.clear();
        self.spy_buttons.clear();

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
                self.cell(
                    header,
                    TRIBUTE_LEFT,
                    TRIBUTE_BUTTON * CODES.len() as i32,
                    &lang::translate("Tribute"),
                    "center",
                )?;
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
                Some(viewed) => {
                    self.stance_columns(&row, scene, id, viewed)?;
                    self.tribute_buttons(&row, view, id, viewed)?;
                    self.attack_button(&row, view, id, viewed)?;
                    self.spy_button(&row, view, id, viewed)?;
                }
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
            if let Some(seat) = kick_row(view, id) {
                self.kick_button(&row, player.state, seat)?;
                self.ban_button(&row, seat)?;
            }
        }
        Ok(())
    }

    /// The host's Kick on the human row of `seat` (M29). A kicked seat and
    /// one out of the game cannot be kicked again.
    fn kick_button(
        &mut self,
        row: &HtmlElement,
        state: PlayerState,
        seat: ControlSeat,
    ) -> Result<(), String> {
        let button: HtmlButtonElement = create(&self.document, "button")?;
        let _ = button.set_attribute("class", "text center sans-bold-stroke-14 stone");
        let _ = button.set_attribute("style", &place(KICK_LEFT, 56));
        lang::set_text(&button, "Kick");
        let out = seat.kicked || state != PlayerState::Active;
        button.set_disabled(out);
        if seat.kicked {
            set_tooltip(&button, &lang::translate("Kicked"), "sessionToolTipBold");
        } else if out {
            set_tooltip(
                &button,
                &lang::translate("This player is out of the game."),
                "sessionToolTipBold",
            );
        }
        let request = Rc::clone(&self.kick_request);
        let player = seat.player;
        let handler = Closure::<dyn FnMut()>::new(move || request.set(Some(player)));
        button.set_onclick(Some(handler.as_ref().unchecked_ref()));
        self.kick_handlers.push(handler);
        row.append_child(&button)
            .map_err(|_| "cannot add a kick button")?;
        Ok(())
    }

    /// The host's Ban on the row of `seat` (D54): a kicked or defeated seat
    /// can still be banned, but a banned one cannot be banned again.
    fn ban_button(&mut self, row: &HtmlElement, seat: ControlSeat) -> Result<(), String> {
        let button: HtmlButtonElement = create(&self.document, "button")?;
        let _ = button.set_attribute("class", "text center sans-bold-stroke-14 stone");
        let _ = button.set_attribute("style", &place(BAN_LEFT, 56));
        lang::set_text(&button, "Ban");
        button.set_disabled(seat.banned);
        if seat.banned {
            set_tooltip(&button, &lang::translate("Banned"), "sessionToolTipBold");
        }
        let request = Rc::clone(&self.ban_request);
        let player = seat.player;
        let handler = Closure::<dyn FnMut()>::new(move || request.set(Some(player)));
        button.set_onclick(Some(handler.as_ref().unchecked_ref()));
        self.kick_handlers.push(handler);
        row.append_child(&button)
            .map_err(|_| "cannot add a ban button")?;
        Ok(())
    }

    /// `TributeButtonManager` and `TributeButton`: one button per
    /// tributable resource on the row of another active player, enabled
    /// only for the seat that controls the viewed player.
    fn tribute_buttons(
        &mut self,
        row: &HtmlElement,
        view: &View,
        id: usize,
        viewed: usize,
    ) -> Result<(), String> {
        let scene = view.scene;
        let Some(player) = scene.players.get(id) else {
            return Ok(());
        };
        // `isInactive`; the tribute ignores a ceasefire.
        let viewed_state = scene.players.get(viewed).map(|viewed| viewed.state);
        if id == viewed
            || viewed_state != Some(PlayerState::Active)
            || player.state != PlayerState::Active
        {
            return Ok(());
        }
        // In 0.28 every resource in simulation/data/resources/ is
        // "tributable", so this is `GetTributableCodes()`.
        for (index, code) in CODES.iter().enumerate() {
            let button: HtmlButtonElement = create(&self.document, "button")?;
            let _ = button.set_attribute("class", "tribute");
            let _ = button.set_attribute(
                "style",
                &place(TRIBUTE_LEFT + TRIBUTE_BUTTON * index as i32, TRIBUTE_BUTTON),
            );
            let icon: HtmlElement = create(&self.document, "div")?;
            let _ = icon.set_attribute("class", "icon");
            let _ = icon.set_attribute(
                "style",
                &format!("background-image: var(--gui-resource-{code})"),
            );
            button
                .append_child(&icon)
                .map_err(|_| "cannot add a tribute icon")?;
            button.set_disabled(!self.controls);
            let amount = self.amounts.get(id).map_or(DEFAULT_AMOUNT, |a| a[index]);
            set_tooltip(
                &button,
                &tribute_tooltip(scene, view.palette, id, code, amount),
                "sessionToolTipBold",
            );
            let request = Rc::clone(&self.tribute_request);
            let handler = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(
                move |event: web_sys::MouseEvent| {
                    request.set(Some((id as u8, index, event.shift_key())));
                },
            );
            button.set_onclick(Some(handler.as_ref().unchecked_ref()));
            self.tribute_handlers.push(handler);
            row.append_child(&button)
                .map_err(|_| "cannot add a tribute button")?;
            self.tribute_buttons.push((id as u8, index, button.into()));
        }
        Ok(())
    }

    /// `diplomacyPlayerTheirs[n]` and the three `StanceButton`s: one press
    /// per stance, enabled only for the seat that controls the viewed player
    /// (`StanceButtonManager.update`).
    fn stance_columns(
        &mut self,
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
        if inactive || ceasefire_active(scene) || scene.lock_teams {
            return Ok(());
        }
        let current = scene
            .players
            .get(viewed)
            .and_then(|viewed| viewed.stances.get(id))
            .copied()
            .unwrap_or(Stance::Enemy);
        for (i, (stance, code)) in [
            (Stance::Ally, DIPLOMACY_ALLY),
            (Stance::Neutral, DIPLOMACY_NEUTRAL),
            (Stance::Enemy, DIPLOMACY_ENEMY),
        ]
        .into_iter()
        .enumerate()
        {
            let button: HtmlButtonElement = create(&self.document, "button")?;
            let _ = button.set_attribute("class", "text center sans-bold-stroke-14 stone stance");
            let _ = button.set_attribute(
                "style",
                &place(STANCE_LEFT + STANCE_BUTTON * i as i32, STANCE_BUTTON),
            );
            if stance == current {
                let caption = lang::translate_context("diplomatic stance selection", "x");
                button.set_text_content(Some(&caption));
            }
            button.set_disabled(stance == current || !self.controls);
            let request = Rc::clone(&self.stance_request);
            let player = id as u8;
            let handler = Closure::<dyn FnMut()>::new(move || request.set(Some((player, code))));
            button.set_onclick(Some(handler.as_ref().unchecked_ref()));
            self.request_handlers.push(handler);
            row.append_child(&button)
                .map_err(|_| "cannot add a stance button")?;
        }
        Ok(())
    }

    /// `AttackRequestButton`: shown on an enemy row only while the viewed
    /// player has a mutual ally to ask.
    fn attack_button(
        &mut self,
        row: &HtmlElement,
        view: &View,
        id: usize,
        viewed: usize,
    ) -> Result<(), String> {
        let scene = view.scene;
        let Some(player) = scene.players.get(id) else {
            return Ok(());
        };
        let viewed_state = scene.players.get(viewed).map(|viewed| viewed.state);
        if id == viewed
            || viewed_state != Some(PlayerState::Active)
            || player.state != PlayerState::Active
            || ceasefire_active(scene)
        {
            return Ok(());
        }
        let enemy = scene
            .players
            .get(viewed)
            .and_then(|viewed| viewed.stances.get(id))
            == Some(&Stance::Enemy);
        if !enemy || !has_mutual_ally(scene, viewed) {
            return Ok(());
        }
        let button: HtmlButtonElement = create(&self.document, "button")?;
        let _ = button.set_attribute("class", "request");
        let _ = button.set_attribute("style", &place(ATTACK_LEFT, REQUEST_BUTTON));
        let icon: HtmlElement = create(&self.document, "div")?;
        let _ = icon.set_attribute("class", "icon");
        let _ = icon.set_attribute("style", "background-image: var(--gui-attack-request)");
        button
            .append_child(&icon)
            .map_err(|_| "cannot add an attack request icon")?;
        button.set_disabled(!self.controls);
        set_tooltip(
            &button,
            &lang::translate("Request your allies to attack this enemy"),
            "sessionToolTipBold",
        );
        let request = Rc::clone(&self.attack_request);
        let player = id as u8;
        let handler = Closure::<dyn FnMut()>::new(move || request.set(Some(player)));
        button.set_onclick(Some(handler.as_ref().unchecked_ref()));
        self.request_handlers.push(handler);
        row.append_child(&button)
            .map_err(|_| "cannot add an attack request button")?;
        Ok(())
    }

    /// `SpyRequestButton`: bribe a unit of the row's player; its tooltip and
    /// state are refreshed every update.
    fn spy_button(
        &mut self,
        row: &HtmlElement,
        view: &View,
        id: usize,
        viewed: usize,
    ) -> Result<(), String> {
        let scene = view.scene;
        let Some(player) = scene.players.get(id) else {
            return Ok(());
        };
        let viewed_state = scene.players.get(viewed).map(|viewed| viewed.state);
        if id == viewed
            || viewed_state != Some(PlayerState::Active)
            || player.state != PlayerState::Active
        {
            return Ok(());
        }
        let Some(name) = panel::string_index(scene, SPY_TEMPLATE) else {
            return Ok(());
        };
        if panel::template_data_of(scene, view.data, name, viewed as i32).is_none() {
            return Ok(());
        }
        // An old hook sends no spy data.
        let Some(gui) = scene.details.players.get(viewed) else {
            return Ok(());
        };
        if gui.spy_flags & PlayerGui::SPY_DISABLED != 0 {
            return Ok(());
        }
        if mutual_ally(scene, viewed, id)
            && !scene
                .players
                .get(viewed)
                .is_some_and(|viewed| viewed.shared_los)
        {
            return Ok(());
        }
        let button: HtmlButtonElement = create(&self.document, "button")?;
        let _ = button.set_attribute("class", "request spy");
        let _ = button.set_attribute("style", &place(SPY_LEFT, REQUEST_BUTTON));
        let icon: HtmlElement = create(&self.document, "div")?;
        let _ = icon.set_attribute("class", "icon");
        let _ = icon.set_attribute("style", "background-image: var(--gui-bribes)");
        let tint: HtmlElement = create(&self.document, "div")?;
        let _ = tint.set_attribute("class", "tint");
        icon.append_child(&tint)
            .map_err(|_| "cannot add a spy request tint")?;
        button
            .append_child(&icon)
            .map_err(|_| "cannot add a spy request icon")?;
        button.set_disabled(!self.controls);
        let request = Rc::clone(&self.spy_request);
        let player = id as u8;
        let handler = Closure::<dyn FnMut()>::new(move || request.set(Some(player)));
        button.set_onclick(Some(handler.as_ref().unchecked_ref()));
        self.request_handlers.push(handler);
        row.append_child(&button)
            .map_err(|_| "cannot add a spy request button")?;
        self.spy_buttons.push(SpyButton {
            target: id as u8,
            button,
            icon,
            tint,
            tooltip: String::new(),
        });
        Ok(())
    }

    /// `SpyRequestButton.update`, every update: the tooltip's requirements,
    /// modified cost and failed-bribe cost, and whether the bribe can be
    /// pressed.
    fn refresh_spy(&mut self, view: &View) {
        let Perspective::Player(viewed) = view.perspective else {
            return;
        };
        let scene = view.scene;
        let Some(name) = panel::string_index(scene, SPY_TEMPLATE) else {
            return;
        };
        let Some(template) = panel::template_data_of(scene, view.data, name, viewed as i32) else {
            return;
        };
        let Some(gui) = scene.details.players.get(viewed) else {
            return;
        };
        let controls = self.controls;
        for spy in &mut self.spy_buttons {
            let mut tooltip = lang::translate(
                "Bribe a random unit from this player and share its vision during a limited period.",
            );
            let mut grey = false;
            let mut tint: Option<String> = None;
            let mut disabled = false;
            let requirements = tips::field(&template, "requirements");
            let missing_requirements =
                requirements.is_truthy() && gui.spy_flags & PlayerGui::SPY_REQUIREMENTS_MET == 0;
            if missing_requirements {
                let technology = |technology: &str| {
                    panel::string_index(scene, technology).and_then(|name| {
                        panel::technology_data_of(scene, view.data, name, viewed as i32)
                    })
                };
                tooltip.push('\n');
                tooltip.push_str(&tips::requirements_tooltip(&template, &technology));
                grey = true;
                disabled = true;
            }
            let mut out_of_resources = false;
            if !missing_requirements && tips::field(&template, "cost").is_truthy() {
                let cost = tips::field(&template, "cost");
                let multiplier = scene
                    .details
                    .players
                    .get(usize::from(spy.target))
                    .map_or(1.0, |target| f64::from(target.spy_multiplier));
                let modified: Vec<(String, f64)> = tips::RESOURCES
                    .iter()
                    .map(|code| {
                        (
                            (*code).to_string(),
                            (multiplier * tips::field(&cost, code).as_f64().unwrap_or(0.0)).floor(),
                        )
                    })
                    .collect();
                tooltip.push('\n');
                tooltip.push_str(&tips::cost_tooltip(&template, 1.0, Some(&modified)));
                let mut needed = Vec::new();
                if let Some(economy) = scene.economy.get(viewed) {
                    for (code, amount) in &modified {
                        let Some(stock) = economy
                            .resources
                            .iter()
                            .find(|stock| panel::string(scene, stock.code) == Some(code.as_str()))
                        else {
                            continue;
                        };
                        let stock = f64::from(stock.amount);
                        if *amount > stock {
                            needed.push((code.clone(), *amount - stock));
                        }
                    }
                }
                if !needed.is_empty() {
                    tooltip.push('\n');
                    tooltip.push_str(&tips::needed_resources_tooltip(&needed));
                    if let Some(alpha) = panel::needed_alpha(&needed) {
                        tint = Some(format!("background: rgba(255, 0, 0, {alpha:.3})"));
                    }
                    out_of_resources = true;
                    disabled = true;
                } else if gui.spy_failure_ratio > 0.0 {
                    let ratio = f64::from(gui.spy_failure_ratio);
                    let failed: Vec<(String, f64)> = modified
                        .iter()
                        .map(|(code, amount)| (code.clone(), (ratio * amount).floor()))
                        .collect();
                    tooltip.push('\n');
                    tooltip.push_str(&lang::translate("A failed bribe will cost you:"));
                    tooltip.push('\n');
                    tooltip.push_str(&tips::cost_tooltip(&template, 1.0, Some(&failed)));
                }
            }
            if !missing_requirements && !out_of_resources && self.spy_pending.contains(&spy.target)
            {
                grey = true;
                disabled = true;
            }
            disabled = disabled || !controls;
            let class = if grey { "icon grey" } else { "icon" };
            if spy.icon.class_name() != class {
                spy.icon.set_class_name(class);
            }
            let tint = tint.unwrap_or_default();
            if spy.tint.get_attribute("style").unwrap_or_default() != tint {
                let _ = spy.tint.set_attribute("style", &tint);
            }
            if spy.button.disabled() != disabled {
                spy.button.set_disabled(disabled);
            }
            if spy.tooltip != tooltip {
                set_tooltip(&spy.button, &tooltip, "sessionToolTipBold");
                spy.tooltip = tooltip;
            }
        }
    }

    /// The presses of the stance, attack and spy buttons since the last
    /// call: a spy's target waits for its answer.
    pub fn take_requests(&mut self) -> Vec<Command> {
        let mut commands = Vec::new();
        if !self.controls {
            self.stance_request.take();
            self.attack_request.take();
            self.spy_request.take();
            return commands;
        }
        if let Some((player, stance)) = self.stance_request.take() {
            commands.push(Command::Diplomacy { player, stance });
        }
        if let Some(player) = self.attack_request.take() {
            commands.push(Command::AttackRequest { player });
        }
        if let Some(player) = self.spy_request.take() {
            if !self.spy_pending.contains(&player) {
                self.spy_pending.push(player);
            }
            commands.push(Command::SpyRequest { player });
        }
        commands
    }

    /// `onSpyResponse`: the answer arrived, so the row's button can be
    /// pressed again.
    pub fn spy_response(&mut self, target: u8) {
        self.spy_pending.retain(|pending| *pending != target);
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

/// `TributeButton.nextAmount`.
fn next_amount(amount: u32) -> u32 {
    MASS_AMOUNT * (amount / MASS_AMOUNT + 1)
}

/// `TributeButton.prototype.Tooltip`.
fn tribute_tooltip(scene: &Scene, palette: &[u32], id: usize, code: &str, amount: u32) -> String {
    let name = scene.players.get(id).map_or("", |player| &player.name);
    lang::format(
        &lang::translate(
            "Tribute %(resourceAmount)s %(resourceType)s to %(playerName)s. Shift-click to tribute %(greaterAmount)s.",
        ),
        &[
            ("resourceAmount", &amount.to_string()),
            ("greaterAmount", &next_amount(amount).to_string()),
            (
                "resourceType",
                &lang::translate_context(
                    "withinSentence",
                    tooltip::resource_data(code).map_or(code, |(name, _)| name),
                ),
            ),
            ("playerName", &tooltip::colored(name, &rgb(palette, id))),
        ],
    )
}

/// Whether the viewer is the host of the game.
fn viewer_is_host(view: &View) -> bool {
    let (Some(control), Some(own)) = (view.control, view.own) else {
        return false;
    };
    control
        .seats
        .iter()
        .any(|seat| seat.player == own && seat.host)
}

fn place(left: i32, width: i32) -> String {
    format!("left: {left}px; width: {width}px")
}

/// The control seat whose row shows Kick to the viewer: the viewer is the
/// host, and the row is another human seat.
fn kick_row(view: &View, id: usize) -> Option<ControlSeat> {
    let control = view.control?;
    let own = view.own?;
    if !viewer_is_host(view) || usize::from(own) == id {
        return None;
    }
    control
        .seats
        .iter()
        .find(|seat| usize::from(seat.player) == id)
        .copied()
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

pub(crate) fn scene_ms(scene: &Scene) -> f64 {
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

/// `Diplomacy.IsMutualAlly` between two players of the scene.
fn mutual_ally(scene: &Scene, a: usize, b: usize) -> bool {
    a != 0
        && b != 0
        && scene
            .players
            .get(a)
            .and_then(|player| player.stances.get(b))
            == Some(&Stance::Ally)
        && scene
            .players
            .get(b)
            .and_then(|player| player.stances.get(a))
            == Some(&Stance::Ally)
}

/// Whether the viewed player has a mutual ally other than itself to ask for
/// an attack.
fn has_mutual_ally(scene: &Scene, viewed: usize) -> bool {
    (1..scene.players.len()).any(|player| player != viewed && mutual_ally(scene, viewed, player))
}

/// Everything the rows show, so they are built again only when it changes;
/// the ceasefire is in it as the stance buttons hide during one.
fn build_key(view: &View) -> String {
    let mut key = format!(
        "{:?}|{}|{}|",
        view.palette,
        ceasefire_active(view.scene),
        u8::from(view.scene.lock_teams)
    );
    if let Perspective::Player(id) = view.perspective {
        key.push_str(&id.to_string());
    }
    key.push_str(&format!("|own{:?}", view.own));
    if let Some(control) = view.control {
        for seat in &control.seats {
            key.push_str(&format!(
                "|{}:{}{}{}{}",
                seat.player,
                u8::from(seat.host),
                u8::from(seat.kicked),
                u8::from(seat.absent),
                u8::from(seat.banned)
            ));
        }
    }
    for (index, player) in view.scene.players.iter().enumerate() {
        let stances: Vec<u8> = player.stances.iter().map(|&stance| stance as u8).collect();
        key.push_str(&format!(
            "|{:?}|{}|{}|{:?}|{}|{}|{}",
            player.name,
            player.team,
            player.state as u8,
            stances,
            player.civ,
            u8::from(player.shared_los),
            view.scene
                .details
                .players
                .get(index)
                .map_or(0, |gui| gui.spy_flags)
        ));
    }
    if let Perspective::Player(id) = view.perspective {
        let template = panel::string_index(view.scene, SPY_TEMPLATE)
            .and_then(|name| panel::template_data_of(view.scene, view.data, name, id as i32))
            .is_some();
        key.push_str(&format!("|spy{}", u8::from(template)));
    }
    key
}
