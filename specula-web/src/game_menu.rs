//! The session's Menu (`top_panel/MenuButton.xml`, `Menu.xml`,
//! `MenuButtons.js`) on `/play`, for the visitor who started the game, with
//! only its Resign and Exit; and the requests they and the match over
//! popup's Rematch make of the server.

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::Document;
use web_sys::HtmlButtonElement;
use web_sys::HtmlElement;
use web_sys::Response;

use crate::lang;

/// Long enough for an engine whose game the defeat finished to have quit,
/// so that the page stays on that match's end instead.
const DEFEAT_SETTLE_MS: i32 = 3000;

pub struct GameMenu {
    open: HtmlButtonElement,
    panel: HtmlElement,
    resign: HtmlButtonElement,
    exit: HtmlButtonElement,
}

impl GameMenu {
    pub fn new(document: &Document) -> Result<Self, String> {
        let menu = GameMenu {
            open: crate::element(document, "game-menu-open")?,
            panel: crate::element(document, "game-menu-panel")?,
            resign: crate::element(document, "game-menu-resign")?,
            exit: crate::element(document, "game-menu-exit")?,
        };
        lang::set_text(&menu.open, "Menu");
        lang::set_text(&menu.resign, "Resign");
        lang::set_text(&menu.exit, "Exit");
        crate::element::<HtmlElement>(document, "game-menu")?.set_hidden(false);
        Ok(menu)
    }

    /// Menu, Resign and Exit.
    pub fn buttons(&self) -> (&HtmlElement, &HtmlElement, &HtmlElement) {
        (&self.open, &self.resign, &self.exit)
    }

    /// `Menu.toggle`.
    pub fn toggle(&self) {
        self.set_open(self.panel.hidden());
    }

    /// `Menu.close`, which pressing one of its buttons does too.
    pub fn close(&self) {
        self.set_open(false);
    }

    fn set_open(&self, open: bool) {
        self.panel.set_hidden(!open);
        let _ = self
            .open
            .set_attribute("aria-expanded", if open { "true" } else { "false" });
    }

    /// Resign is for a player still in the game, as `!g_IsObserver`;
    /// neither is left to do once the game has ended.
    pub fn update(&self, defeated: bool, ended: bool) {
        let resign = !defeated && !ended;
        if self.resign.disabled() == resign {
            self.resign.set_disabled(!resign);
        }
        if self.exit.disabled() != ended {
            self.exit.set_disabled(ended);
        }
    }
}

/// What `/play/setup` says of the visitor's game.
pub struct Status {
    /// The visitor's seat resigned or was beaten while the game goes on.
    pub defeated: bool,
    /// The visitor's last game can be started again.
    pub rematch: bool,
}

pub async fn status() -> Result<Status, String> {
    let window = web_sys::window().ok_or("no window")?;
    let response: Response = JsFuture::from(window.fetch_with_str("/play/setup"))
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
    Ok(Status {
        defeated: flag("defeated"),
        rematch: flag("rematch"),
    })
}

/// Posts `/play/<action>` (resign, end or rematch); the error is the
/// server's reason.
pub async fn post(action: &str) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    // Through Reflect: RequestInit is not among web-sys's enabled features,
    // and it is one call.
    let fetch = js_sys::Reflect::get(&window, &"fetch".into())
        .ok()
        .and_then(|fetch| fetch.dyn_into::<js_sys::Function>().ok())
        .ok_or("no fetch")?;
    let init = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&init, &"method".into(), &"POST".into());
    let url = JsValue::from_str(&format!("/play/{action}"));
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
    if response.ok() {
        return Ok(());
    }
    let reason = match response.text() {
        Ok(text) => JsFuture::from(text)
            .await
            .ok()
            .and_then(|text| text.as_string())
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    Err(format!("{action}: {reason}"))
}

/// Opens `/play` again, which shows the visitor's game as it now is.
pub fn reload() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href("/play");
    }
}

/// The seat was just reported resigned or beaten: unless that ended the
/// game, opens `/play` again, which then shows the match as the observers
/// see it.
pub fn after_defeat() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let check = Closure::once_into_js(|| {
        wasm_bindgen_futures::spawn_local(async {
            if status().await.is_ok_and(|status| status.defeated) {
                reload();
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
            Ok(()) => reload(),
            Err(error) => web_sys::console::error_1(&error.into()),
        }
    });
}
