//! The session's Menu (`top_panel/MenuButton.xml`, `Menu.xml`,
//! `MenuButtons.js`) on `/play`, for a visitor with a seat: its Summary,
//! Pause, Resign, Claim victory, End game (the host's) and Exit; and the
//! requests they and the match over popup's Rematch make of the server.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::Document;
use web_sys::HtmlButtonElement;
use web_sys::HtmlElement;
use web_sys::Node;
use web_sys::Response;

use crate::detail::Control;
use crate::lang;

/// Long enough for an engine whose game the defeat finished to have quit,
/// so that the page stays on that match's end instead.
const DEFEAT_SETTLE_MS: i32 = 3000;
/// `localStorage` key of the nickname, which `lobby.html` keeps too.
const NICKNAME_KEY: &str = "specula.nickname";
/// The host's last settings, which `/new` takes (D34).
const SETTINGS_KEY: &str = "specula.matchsettings";
/// `/new` refuses a settings body longer than this, as `lobby.html` knows.
const SETTINGS_MAX: usize = 30000;

pub struct GameMenu {
    root: HtmlElement,
    open: HtmlButtonElement,
    panel: HtmlElement,
    summary: HtmlButtonElement,
    pause: HtmlButtonElement,
    resign: HtmlButtonElement,
    claim: HtmlButtonElement,
    end: HtmlButtonElement,
    exit: HtmlButtonElement,
}

/// The menu's buttons, which the page wires.
pub struct Buttons<'a> {
    pub open: &'a HtmlElement,
    pub summary: &'a HtmlElement,
    pub pause: &'a HtmlElement,
    pub resign: &'a HtmlElement,
    pub claim: &'a HtmlElement,
    pub end: &'a HtmlElement,
    pub exit: &'a HtmlElement,
}

impl GameMenu {
    pub fn new(document: &Document) -> Result<Self, String> {
        let menu = GameMenu {
            root: crate::element(document, "game-menu")?,
            open: crate::element(document, "game-menu-open")?,
            panel: crate::element(document, "game-menu-panel")?,
            summary: crate::element(document, "game-menu-summary")?,
            pause: crate::element(document, "game-menu-pause")?,
            resign: crate::element(document, "game-menu-resign")?,
            claim: crate::element(document, "game-menu-claim")?,
            end: crate::element(document, "game-menu-end")?,
            exit: crate::element(document, "game-menu-exit")?,
        };
        lang::set_text(&menu.open, "Menu");
        lang::set_text(&menu.summary, "Summary");
        lang::set_text(&menu.pause, "Pause");
        lang::set_text(&menu.resign, "Resign");
        lang::set_text(&menu.claim, "Claim victory");
        lang::set_text(&menu.end, "End game");
        lang::set_text(&menu.exit, "Exit");
        menu.root.set_hidden(false);
        Ok(menu)
    }

    pub fn buttons(&self) -> Buttons<'_> {
        Buttons {
            open: &self.open,
            summary: &self.summary,
            pause: &self.pause,
            resign: &self.resign,
            claim: &self.claim,
            end: &self.end,
            exit: &self.exit,
        }
    }

    /// `Menu.toggle`.
    pub fn toggle(&self) {
        self.set_open(self.panel.hidden());
    }

    /// `Menu.close`, which pressing one of its buttons does too.
    pub fn close(&self) {
        self.set_open(false);
    }

    /// A press anywhere but on the menu closes it.
    pub fn close_unless_on(&self, target: Option<&Node>) {
        if !self.panel.hidden() && !self.root.contains(target) {
            self.close();
        }
    }

    fn set_open(&self, open: bool) {
        self.panel.set_hidden(!open);
        let _ = self
            .open
            .set_attribute("aria-expanded", if open { "true" } else { "false" });
    }

    /// Resign is for a player still in the game, as `!g_IsObserver`;
    /// neither is left to do once the game has ended.
    /// `paused`: the visitor's seat has the game paused, which the button
    /// then resumes. Pausing, like Resign, is for a player, as
    /// `PauseControl.canPause` has it. `pause_left_ms`: the seat's pause
    /// time left, which the tooltip shows and which disables Pause at 0
    /// (D41). `control` and `own` are the server's control (kind 3) and the
    /// visitor's seat: Claim victory and End game follow it.
    pub fn update(
        &self,
        defeated: bool,
        ended: bool,
        paused: bool,
        pause_left_ms: Option<f64>,
        control: Option<&Control>,
        own: Option<u8>,
    ) {
        let resign = !defeated && !ended;
        lang::set_text(&self.pause, if paused { "Resume" } else { "Pause" });
        let no_time_left = pause_left_ms.is_some_and(|left| left < 1000.0) && !paused;
        let mut tooltip = lang::format(
            &lang::translate("Press %(hotkey)s to pause or resume the game."),
            &[("hotkey", &crate::command_row::hotkey_text("pause"))],
        );
        if no_time_left {
            tooltip.push('\n');
            tooltip.push_str(&lang::translate("You have no pause time left."));
        } else if let Some(left) = pause_left_ms {
            tooltip.push('\n');
            tooltip.push_str(&lang::format(
                &lang::translate("Pause time left: %(time)s"),
                &[(
                    "time",
                    &crate::format_minutes((left / 1000.0).ceil() as u64),
                )],
            ));
        }
        if self
            .pause
            .get_attribute(crate::tooltip::ATTRIBUTE)
            .as_deref()
            != Some(tooltip.as_str())
        {
            let _ = self
                .pause
                .set_attribute(crate::tooltip::ATTRIBUTE, &tooltip);
        }
        let disabled = !resign || no_time_left;
        if self.pause.disabled() != disabled {
            self.pause.set_disabled(disabled);
        }
        if self.resign.disabled() == resign {
            self.resign.set_disabled(!resign);
        }
        if self.exit.disabled() != ended {
            self.exit.set_disabled(ended);
        }
        let claim = !ended && control.is_some_and(|control| control.claim);
        let end = !ended && control.is_some_and(|control| control.end_allowed);
        let host = control
            .zip(own)
            .and_then(|(control, own)| control.seats.iter().find(|seat| seat.player == own))
            .is_some_and(|seat| seat.host);
        set_tooltip(
            &self.claim,
            if claim {
                ""
            } else {
                "Available once every player still connected is an ally, and the others have been away for 5 minutes with no pause time left."
            },
        );
        set_tooltip(
            &self.end,
            if end {
                ""
            } else if host {
                "The host can end the game only in its first minute, or once every other player has left or resigned."
            } else {
                "Only the host can end the game."
            },
        );
        if self.claim.disabled() == claim {
            self.claim.set_disabled(!claim);
        }
        if self.end.disabled() == end {
            self.end.set_disabled(!end);
        }
    }
}

/// The disabled button's reason, in the engine's tooltip markup; empty for
/// none.
fn set_tooltip(button: &HtmlElement, english: &str) {
    let text = if english.is_empty() {
        String::new()
    } else {
        lang::translate(english)
    };
    let current = button.get_attribute(crate::tooltip::ATTRIBUTE);
    if text.is_empty() {
        if current.is_some() {
            let _ = button.remove_attribute(crate::tooltip::ATTRIBUTE);
        }
    } else if current.as_deref() != Some(text.as_str()) {
        let _ = button.set_attribute(crate::tooltip::ATTRIBUTE, &text);
    }
}

/// What `play/status` says of the visitor's game.
pub struct Status {
    /// The visitor's seat resigned or was beaten while the game goes on.
    pub defeated: bool,
    /// The visitor's seat is defeated with no mutual ally left in the game:
    /// its page shows the observers' view (D47).
    pub watch: bool,
    /// The visitor's last game can be started again.
    pub rematch: bool,
    /// The visitor's seat, 0 when the game is not theirs.
    pub seat: usize,
    /// A running hosted match's wall time left before its time limit, in
    /// ms as of the answer, and its game time limit in ms of the game clock.
    pub wall_left_ms: Option<f64>,
    pub game_limit_ms: Option<f64>,
    /// The setup that this match's rematch opened for the visitor (M35),
    /// empty for none.
    pub next: String,
}

pub async fn status() -> Result<Status, String> {
    let window = web_sys::window().ok_or("no window")?;
    let response: Response =
        JsFuture::from(window.fetch_with_str(&format!("{}/play/status", crate::match_base())))
            .await
            .map_err(|_| "cannot ask for the game's status")?
            .dyn_into()
            .map_err(|_| "not a response")?;
    if !response.ok() {
        return Err(format!("the game's status: HTTP {}", response.status()));
    }
    let text = JsFuture::from(response.text().map_err(|_| "no body")?)
        .await
        .map_err(|_| "cannot read the game's status")?;
    let json = js_sys::JSON::parse(&text.as_string().unwrap_or_default())
        .map_err(|_| "the game's status is not JSON")?;
    let flag =
        |name: &str| js_sys::Reflect::get(&json, &name.into()).is_ok_and(|value| value.is_truthy());
    let number = |name: &str| {
        js_sys::Reflect::get(&json, &name.into())
            .ok()
            .and_then(|value| value.as_f64())
    };
    Ok(Status {
        defeated: flag("defeated"),
        watch: flag("watch"),
        rematch: flag("rematch"),
        seat: number("seat").unwrap_or(0.0) as usize,
        wall_left_ms: number("wall_left_ms"),
        game_limit_ms: number("game_limit_ms"),
        next: js_sys::Reflect::get(&json, &"next".into())
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default(),
    })
}

/// Starts a new game from the stored nickname and settings (M35) and opens
/// its setup; a refusal goes into the match over popup, through `error`.
pub fn new_match(error: Rc<RefCell<Option<String>>>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let storage = window.local_storage().ok().flatten();
    let stored = |key: &str| -> Option<String> {
        storage
            .as_ref()?
            .get_item(key)
            .ok()
            .flatten()
            .filter(|value| !value.is_empty())
    };
    let Some(nickname) = stored(NICKNAME_KEY) else {
        // `lobby.html` asks for a name; this page has no form for one.
        let _ = window.location().set_href("/");
        return;
    };
    let encoded = |text: &str| {
        js_sys::encode_uri_component(text)
            .as_string()
            .unwrap_or_default()
    };
    let mut body = format!("nickname={}", encoded(&nickname));
    if let Some(settings) = stored(SETTINGS_KEY)
        && encoded(&settings).len() < SETTINGS_MAX
    {
        body.push_str("&settings=");
        body.push_str(&encoded(&settings));
    }
    *error.borrow_mut() = None;
    wasm_bindgen_futures::spawn_local(async move {
        match post_form("/new", &body).await {
            Ok(id) => {
                if let Some(window) = web_sys::window() {
                    let _ = window.location().set_href(&format!("/g/{id}"));
                }
            }
            Err(reason) => {
                let text = lang::format(
                    &lang::translate("Cannot start the game: %(error)s"),
                    &[("error", &lang::translate(&reason))],
                );
                *error.borrow_mut() = Some(text);
            }
        }
    });
}

/// Posts a URL-encoded form to `path` (not under the match's base) and
/// answers its body; the error is the server's reason.
async fn post_form(path: &str, body: &str) -> Result<String, String> {
    let window = web_sys::window().ok_or("no window")?;
    // As `post_path`: RequestInit is not among web-sys's enabled features.
    let fetch = js_sys::Reflect::get(&window, &"fetch".into())
        .ok()
        .and_then(|fetch| fetch.dyn_into::<js_sys::Function>().ok())
        .ok_or("no fetch")?;
    let init = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&init, &"method".into(), &"POST".into());
    let headers = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &headers,
        &"Content-Type".into(),
        &"application/x-www-form-urlencoded".into(),
    );
    let _ = js_sys::Reflect::set(&init, &"headers".into(), &headers);
    let _ = js_sys::Reflect::set(&init, &"body".into(), &body.into());
    let promise: js_sys::Promise = fetch
        .call2(&window, &path.into(), &init)
        .map_err(|_| "cannot send the request")?
        .dyn_into()
        .map_err(|_| "fetch gave no promise")?;
    let response: Response = JsFuture::from(promise)
        .await
        .map_err(|_| "the request failed")?
        .dyn_into()
        .map_err(|_| "not a response")?;
    let body = match response.text() {
        Ok(text) => JsFuture::from(text)
            .await
            .ok()
            .and_then(|text| text.as_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    if response.ok() {
        return Ok(body);
    }
    Err(body)
}

/// Posts `play/<action>` of the match (resign, end or rematch); the answer
/// is the body, and the error the server's reason.
pub async fn post(action: &str) -> Result<String, String> {
    // The server takes a seat's action only from the page holding it (D64).
    let separator = if action.contains('?') { '&' } else { '?' };
    post_path(&format!("/play/{action}{separator}tab={}", crate::tab_id())).await
}

/// Posts to `path` under the match's base.
pub async fn post_path(path: &str) -> Result<String, String> {
    let window = web_sys::window().ok_or("no window")?;
    // Through Reflect: RequestInit is not among web-sys's enabled features,
    // and it is one call.
    let fetch = js_sys::Reflect::get(&window, &"fetch".into())
        .ok()
        .and_then(|fetch| fetch.dyn_into::<js_sys::Function>().ok())
        .ok_or("no fetch")?;
    let init = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&init, &"method".into(), &"POST".into());
    let url = JsValue::from_str(&format!("{}{path}", crate::match_base()));
    let promise: js_sys::Promise = fetch
        .call2(&window, &url, &init)
        .map_err(|_| "cannot send the request")?
        .dyn_into()
        .map_err(|_| "fetch gave no promise")?;
    let response: Response = JsFuture::from(promise)
        .await
        .map_err(|_| "the request failed")?
        .dyn_into()
        .map_err(|_| "not a response")?;
    let body = match response.text() {
        Ok(text) => JsFuture::from(text)
            .await
            .ok()
            .and_then(|text| text.as_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    if response.ok() {
        return Ok(body);
    }
    Err(format!("{path}: {body}"))
}

/// Opens the match's page again, which shows it as it now is for the
/// visitor.
pub fn reload() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href(&crate::match_base());
    }
}

/// Opens the match's observers' view, which a seat that is out of the game
/// (or has said it will return) watches from.
pub fn watch() {
    if let Some(window) = web_sys::window() {
        let _ = window
            .location()
            .set_href(&format!("{}?watch", crate::match_base()));
    }
}

/// The seat was just reported resigned or beaten: unless that ended the
/// game, opens the match's page again once the server says the seat has no
/// team vision left, which then shows the match as the observers see it.
pub fn after_defeat() {
    after_defeat_with_retry(|| {});
}

/// The same, for a page that asks by itself: `retry` runs when the server
/// has not said the seat is watching, so the caller may ask again.
pub fn after_defeat_with_retry(retry: impl FnOnce() + 'static) {
    let Some(window) = web_sys::window() else {
        retry();
        return;
    };
    let check = Closure::once_into_js(move || {
        wasm_bindgen_futures::spawn_local(async move {
            if status().await.is_ok_and(|status| status.watch) {
                reload();
            } else {
                retry();
            }
        });
    });
    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
        check.unchecked_ref(),
        DEFEAT_SETTLE_MS,
    );
}

/// Starts the visitor's last game again and opens it.
pub fn rematch() {
    wasm_bindgen_futures::spawn_local(async {
        match post("rematch").await {
            Ok(id) => {
                if let Some(window) = web_sys::window() {
                    let _ = window.location().set_href(&format!("/g/{id}"));
                }
            }
            Err(error) => web_sys::console::error_1(&error.into()),
        }
    });
}
