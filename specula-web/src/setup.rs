//! The setup page of a match (`/g/<uuid>` before the launch): the settings
//! with vanilla's controls, the player slots, the spectators, Ready and
//! Start, over the setup socket (`specula-server/src/setup.rs`). The server
//! sends the whole state on every change; each control is built once and
//! only has its value, state and options updated, so a list a visitor has
//! open is not closed by someone else's change.
//!
//! The settings are read as the engine's `initAttributes`
//! (`state.attributes`), what is offered from `state.offer`. Only the host's
//! controls are enabled.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use js_sys::Array;
use js_sys::Reflect;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::Document;
use web_sys::Element;
use web_sys::Event;
use web_sys::HtmlButtonElement;
use web_sys::HtmlElement;
use web_sys::HtmlInputElement;
use web_sys::HtmlOptionElement;
use web_sys::HtmlSelectElement;
use web_sys::MessageEvent;
use web_sys::WebSocket;

use crate::chat;
use crate::lang;
use crate::lang::translate;
use crate::lang::translate_context as tc;

/// `localStorage` key of the nickname, which `lobby.html` keeps too.
const NICKNAME_KEY: &str = "specula.nickname";
/// The host's last settings, which `lobby.html` sends with `POST /new`; it is
/// vanilla's `config/matchsettings.mp.json` (D34).
const SETTINGS_KEY: &str = "specula.matchsettings";
/// Reconnecting: first retry after this, doubling up to `RETRY_MAX_MS`.
const RETRY_MS: i32 = 1000;
const RETRY_MAX_MS: i32 = 5000;
const COPIED_MS: i32 = 2000;
/// The civ names `session-gui` writes.
const CIVS: &str = include_str!("../civs.txt");
/// Population caps from here on read "Unlimited" (`PopulationCap.js`).
const UNLIMITED_POPULATION: f64 = 10000.0;
/// The sliders' lowest value for the durations (`Sliders/*Duration.js`).
const RELIC_DURATION_MIN: f64 = 0.0;

mod map_browser;

type Shared = Rc<RefCell<Page>>;
type Message = Rc<dyn Fn(Input) -> JsValue>;

/// What a control's event gives.
enum Input {
    Text(String),
    Bool(bool),
    Num(f64),
}

impl Input {
    fn text(&self) -> String {
        match self {
            Input::Text(text) => text.clone(),
            Input::Bool(flag) => flag.to_string(),
            Input::Num(number) => number.to_string(),
        }
    }

    fn number(&self) -> f64 {
        match self {
            Input::Text(text) => text.parse().unwrap_or(f64::NAN),
            Input::Bool(flag) => f64::from(u8::from(*flag)),
            Input::Num(number) => *number,
        }
    }

    fn flag(&self) -> bool {
        match self {
            Input::Text(text) => text == "true",
            Input::Bool(flag) => *flag,
            Input::Num(number) => *number != 0.0,
        }
    }
}

// ---- JSON access ----

fn get(value: &JsValue, key: &str) -> JsValue {
    Reflect::get(value, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

fn text(value: &JsValue, key: &str) -> String {
    get(value, key).as_string().unwrap_or_default()
}

fn number(value: &JsValue, key: &str) -> f64 {
    get(value, key).as_f64().unwrap_or(0.0)
}

fn flag(value: &JsValue, key: &str) -> bool {
    get(value, key).as_bool().unwrap_or(false)
}

fn list(value: &JsValue, key: &str) -> Vec<JsValue> {
    let found = get(value, key);
    if Array::is_array(&found) {
        found.unchecked_into::<Array>().iter().collect()
    } else {
        Vec::new()
    }
}

fn strings(value: &JsValue, key: &str) -> Vec<String> {
    list(value, key)
        .iter()
        .filter_map(JsValue::as_string)
        .collect()
}

fn present(value: &JsValue, key: &str) -> bool {
    !get(value, key).is_null() && !get(value, key).is_undefined()
}

fn object(pairs: &[(&str, JsValue)]) -> JsValue {
    let object = js_sys::Object::new();
    for (key, value) in pairs {
        let _ = Reflect::set(&object, &(*key).into(), value);
    }
    object.into()
}

fn change(name: &str, value: JsValue) -> JsValue {
    object(&[
        ("type", "change".into()),
        ("name", name.into()),
        ("value", value),
    ])
}

fn message_text(name: &'static str) -> Message {
    Rc::new(move |input| change(name, input.text().into()))
}

fn message_number(name: &'static str) -> Message {
    Rc::new(move |input| change(name, input.number().into()))
}

fn message_flag(name: &'static str) -> Message {
    Rc::new(move |input| change(name, input.flag().into()))
}

// ---- DOM helpers ----

fn create<T: JsCast>(document: &Document, tag: &str) -> T {
    document
        .create_element(tag)
        .expect("a known tag")
        .unchecked_into()
}

fn by_id<T: JsCast>(document: &Document, id: &str) -> Result<T, String> {
    document
        .get_element_by_id(id)
        .map(|element| element.unchecked_into())
        .ok_or_else(|| format!("setup.html has no #{id}"))
}

fn show(element: &Element, visible: bool) {
    if visible {
        let _ = element.remove_attribute("hidden");
    } else {
        let _ = element.set_attribute("hidden", "");
    }
}

fn enable(element: &Element, enabled: bool) {
    if enabled {
        let _ = element.remove_attribute("disabled");
    } else {
        let _ = element.set_attribute("disabled", "");
    }
}

/// Gives `element` the text vanilla's `onscreenToolTip` shows in the
/// bottom-left box while it is hovered, in place of a browser tooltip that
/// would show it twice.
fn tip(element: &Element, text: &str) {
    let _ = element.set_attribute("data-tip", text);
    if element.has_attribute("data-tip-shown")
        && let Some(board) = element
            .owner_document()
            .and_then(|document| document.get_element_by_id("tip"))
    {
        board.set_text_content(Some(text));
        show(&board, !text.is_empty());
    }
}

/// Shows in `#tip` the text of what the pointer or, with no hover on a
/// touch screen or keyboard, the focus last went to, as vanilla's
/// `hide_object` tooltip does.
fn install_tips(document: &Document) -> Result<(), String> {
    let board: HtmlElement = by_id(document, "tip")?;
    let root: Element = by_id(document, "window")?;
    let shown: Rc<RefCell<Option<Element>>> = Rc::new(RefCell::new(None));
    let point = Rc::new(move |target: Option<Element>| {
        let owner = target.and_then(|target| target.closest("[data-tip]").ok().flatten());
        let mut shown = shown.borrow_mut();
        if let Some(old) = shown.as_ref() {
            let _ = old.remove_attribute("data-tip-shown");
        }
        let text = owner
            .as_ref()
            .and_then(|owner| owner.get_attribute("data-tip"))
            .unwrap_or_default();
        if let Some(owner) = &owner {
            let _ = owner.set_attribute("data-tip-shown", "");
        }
        *shown = owner;
        board.set_text_content(Some(&text));
        show(&board, !text.is_empty());
    });
    let target = |event: &Event| {
        event
            .target()
            .and_then(|target| target.dyn_into::<Element>().ok())
    };
    let over = Rc::clone(&point);
    listen(&root, "mouseover", move |event| over(target(&event)));
    let leave = Rc::clone(&point);
    listen(&root, "mouseleave", move |_| leave(None));
    listen(&root, "focusin", move |event| {
        let focused = target(&event);
        if let Some(focused) = &focused {
            // The box is also what a screen reader reads as its description.
            let _ = focused.set_attribute("aria-describedby", "tip");
        }
        point(focused);
    });
    listen(&root, "focusout", move |event| {
        if let Some(focused) = target(&event) {
            let _ = focused.remove_attribute("aria-describedby");
        }
    });
    Ok(())
}

fn clear(element: &Element) {
    while let Some(child) = element.first_child() {
        let _ = element.remove_child(&child);
    }
}

fn listen(target: &Element, event: &str, handler: impl FnMut(Event) + 'static) {
    let closure = Closure::<dyn FnMut(Event)>::new(handler);
    let _ = target.add_event_listener_with_callback(event, closure.as_ref().unchecked_ref());
    closure.forget();
}

/// Makes `select`'s options `options` (value, text) unless they already are,
/// then picks `value`.
fn fill(
    document: &Document,
    select: &HtmlSelectElement,
    options: &[(String, String)],
    value: &str,
) {
    let key = options
        .iter()
        .map(|(value, text)| format!("{value}\u{1}{text}"))
        .collect::<Vec<_>>()
        .join("\u{2}");
    if select.get_attribute("data-options").as_deref() != Some(key.as_str()) {
        clear(select);
        for (value, text) in options {
            let option: HtmlOptionElement = create(document, "option");
            option.set_value(value);
            option.set_text(text);
            // A color list shows each color rather than its name.
            if select.class_name() == "color" {
                let _ = option.set_attribute("style", &format!("background: rgb({value})"));
            }
            let _ = select.append_child(&option);
        }
        let _ = select.set_attribute("data-options", &key);
    }
    select.set_value(value);
}

fn option(value: impl ToString, text: impl ToString) -> (String, String) {
    (value.to_string(), text.to_string())
}

fn title_case(name: &str) -> String {
    name.split(' ')
        .map(|word| {
            let mut letters = word.chars();
            letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn civ_name(code: &str) -> String {
    CIVS.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('\t'))
        .find(|(known, _)| *known == code)
        .map_or_else(|| code.to_string(), |(_, name)| translate(name))
}

fn plural(context: &str, singular: &str, plural: &str, name: &str, count: f64) -> String {
    lang::format(
        &lang::translate_plural_context(context, singular, plural, count),
        &[(name, &count.to_string())],
    )
}

fn minutes_caption(context: &'static str) -> Caption {
    Rc::new(move |value| {
        plural(
            context,
            "%(minutes)s minute",
            "%(minutes)s minutes",
            "minutes",
            value,
        )
    })
}

type Caption = Rc<dyn Fn(f64) -> String>;

/// `GameSettingControl.prototype.TitleCaptionFormat`.
fn titled(setting: &str) -> String {
    lang::format(
        &tc("Title for specific setting", "%(setting)s:"),
        &[("setting", setting)],
    )
}

// ---- the page ----

struct Dom {
    document: Document,
    tabs: HtmlElement,
    settings: HtmlElement,
    players: HtmlElement,
    spectators: HtmlElement,
    ready: HtmlButtonElement,
    start: HtmlButtonElement,
    step_down: HtmlButtonElement,
    join: HtmlElement,
    room: HtmlElement,
    closed: HtmlElement,
    closed_reason: HtmlElement,
    kicked: HtmlElement,
    kicked_text: HtmlElement,
    kicked_rejoin: HtmlButtonElement,
    connection: HtmlElement,
    countdown: HtmlElement,
    error: HtmlElement,
    preview: HtmlElement,
    preview_title: HtmlElement,
    description: HtmlElement,
    copy_link: HtmlElement,
    back: HtmlElement,
    reset_civs: HtmlElement,
    reset_teams: HtmlElement,
    ai_config: HtmlElement,
    ai_bot: HtmlSelectElement,
    ai_difficulty: HtmlSelectElement,
    ai_behavior: HtmlSelectElement,
    ai_description: HtmlElement,
    ai_ok: HtmlElement,
    chat_lines: HtmlElement,
    chat_form: HtmlElement,
    chat_input: HtmlInputElement,
    chat_send: HtmlButtonElement,
}

struct Control {
    wrap: HtmlElement,
    input: HtmlElement,
    name: HtmlElement,
    /// A slider's value caption, or a checkbox's "Yes" or "No", which
    /// vanilla shows in place of a control the viewer cannot change.
    caption: Option<HtmlElement>,
    make_caption: Option<Caption>,
}

/// One slot of `PlayersPanel.xml`: the name, the color, the assignment
/// (or its text for whoever is not the host), the AI's gear, the civ and
/// the team.
struct Row {
    line: HtmlElement,
    name: HtmlElement,
    color: HtmlSelectElement,
    who: HtmlSelectElement,
    who_text: HtmlElement,
    take: HtmlButtonElement,
    gear: HtmlButtonElement,
    civ: HtmlSelectElement,
    team: HtmlSelectElement,
    kick: HtmlButtonElement,
    ban: HtmlButtonElement,
    /// The member the row's Kick and Ban act on, updated each render.
    member: Rc<Cell<Option<usize>>>,
}

struct Page {
    dom: Dom,
    chat: Option<chat::Chat>,
    /// The seats' colours the chat lines were drawn with; the lines are
    /// drawn again when one changes.
    chat_colors: String,
    socket: Option<WebSocket>,
    catalog: JsValue,
    state: JsValue,
    /// The name kept for the next game, sent with every join.
    nickname: Option<String>,
    built: bool,
    controls: HashMap<String, Control>,
    tab_buttons: Vec<HtmlElement>,
    tab_panels: Vec<HtmlElement>,
    rows: Vec<Row>,
    /// The slot whose AI configuration dialog is open.
    ai_slot: Option<usize>,
    /// `Date.now()` the setup closes at, by the server's count.
    closes_at: f64,
    retry_ms: i32,
    /// `launched` or `closed` came: the socket is not reopened.
    done: bool,
    /// Each spectator's Kick and Ban by member index, made once: `listen`
    /// never frees a handler, and the list is redrawn on every state. A
    /// member index always names the same member.
    spectator_buttons: HashMap<usize, (HtmlButtonElement, HtmlButtonElement)>,
    /// The map browser dialog (`MapBrowser.js`).
    map_browser: map_browser::MapBrowser,
}

pub async fn run() -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let dom = Dom {
        tabs: by_id(&document, "tabs")?,
        settings: by_id(&document, "settings")?,
        players: by_id(&document, "players")?,
        spectators: by_id(&document, "spectators")?,
        ready: by_id(&document, "ready")?,
        start: by_id(&document, "start")?,
        step_down: by_id(&document, "step-down")?,
        join: by_id(&document, "join")?,
        room: by_id(&document, "page")?,
        closed: by_id(&document, "closed")?,
        closed_reason: by_id(&document, "closed-reason")?,
        kicked: by_id(&document, "kicked")?,
        kicked_text: by_id(&document, "kicked-text")?,
        kicked_rejoin: by_id(&document, "kicked-rejoin")?,
        connection: by_id(&document, "connection")?,
        countdown: by_id(&document, "countdown")?,
        error: by_id(&document, "error")?,
        preview: by_id(&document, "preview")?,
        preview_title: by_id(&document, "preview-title")?,
        description: by_id(&document, "description")?,
        copy_link: by_id(&document, "copy-link")?,
        back: by_id(&document, "back")?,
        reset_civs: by_id(&document, "reset-civs")?,
        reset_teams: by_id(&document, "reset-teams")?,
        ai_config: by_id(&document, "ai-config")?,
        ai_bot: by_id(&document, "ai-bot")?,
        ai_difficulty: by_id(&document, "ai-difficulty")?,
        ai_behavior: by_id(&document, "ai-behavior")?,
        ai_description: by_id(&document, "ai-description")?,
        ai_ok: by_id(&document, "ai-ok")?,
        chat_lines: by_id(&document, "chat-lines")?,
        chat_form: by_id(&document, "chat-form")?,
        chat_input: by_id(&document, "chat-input")?,
        chat_send: by_id(&document, "chat-send")?,
        document: document.clone(),
    };
    install_tips(&document)?;
    let storage = window.local_storage().ok().flatten();
    let saved = storage
        .as_ref()
        .and_then(|storage| storage.get_item(NICKNAME_KEY).ok().flatten())
        .filter(|name| !name.is_empty());
    let page: Shared = Rc::new(RefCell::new(Page {
        dom,
        chat: None,
        chat_colors: String::new(),
        socket: None,
        catalog: JsValue::UNDEFINED,
        state: JsValue::UNDEFINED,
        nickname: saved,
        built: false,
        controls: HashMap::new(),
        tab_buttons: Vec::new(),
        tab_panels: Vec::new(),
        rows: Vec::new(),
        ai_slot: None,
        closes_at: 0.0,
        retry_ms: RETRY_MS,
        done: false,
        spectator_buttons: HashMap::new(),
        map_browser: map_browser::MapBrowser::new(&document)?,
    }));

    // The language first, so what the page builds is translated.
    let language_setting = storage
        .as_ref()
        .and_then(|storage| storage.get_item(lang::SETTING_KEY).ok().flatten())
        .unwrap_or_default();
    let languages = lang::languages().await;
    lang::load(lang::choose(&languages, &language_setting)).await;

    wire_static(&page);
    map_browser::wire(&page);
    connect(&page);
    wire_chat(&page);
    Ok(())
}

/// The chat of the setup (M32): Everyone only, which the server enforces
/// too (M31). The socket's messages redraw it, as this page has no frame
/// loop.
fn wire_chat(page: &Shared) {
    let weak = Rc::downgrade(page);
    let on_change: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(page) = weak.upgrade() {
            render_chat(&page);
        }
    });
    let chat = chat::Chat::open(false, Some(on_change));
    let form = page.borrow().dom.chat_form.clone();
    page.borrow_mut().chat = Some(chat);
    let shared = Rc::clone(page);
    listen(&form, "submit", move |event| {
        event.prevent_default();
        let page = shared.borrow();
        let Some(chat) = &page.chat else {
            return;
        };
        show(&page.dom.error, false);
        let value = page.dom.chat_input.value();
        if value.is_empty() || chat.you().is_none() {
            return;
        }
        chat.send(chat::Channel::Everyone, None, &value);
        page.dom.chat_input.set_value("");
        // ChatInputPanel.onPress.
        let _ = page.dom.chat_input.focus();
    });
}

/// The name colour of `player`'s seat, from the state's settings, lightened
/// as `ClientChat.colorizePlayernameByGUID` does.
fn chat_color(page: &Page, player: u8) -> Option<String> {
    let attributes = get(&page.state, "attributes");
    let settings = get(&attributes, "settings");
    let data = list(&settings, "PlayerData")
        .into_iter()
        .nth(usize::from(player).checked_sub(1)?)?;
    let color = get(&data, "Color");
    if color.is_undefined() || color.is_null() {
        return None;
    }
    Some(chat::lightened(
        number(&color, "r"),
        number(&color, "g"),
        number(&color, "b"),
    ))
}

/// Every seat's colour as the state names it, to notice a change.
fn chat_colors(page: &Page) -> String {
    let attributes = get(&page.state, "attributes");
    let settings = get(&attributes, "settings");
    js_sys::JSON::stringify(&get(&settings, "PlayerData"))
        .ok()
        .and_then(|json| json.as_string())
        .unwrap_or_default()
}

fn render_chat(shared: &Shared) {
    let colors = chat_colors(&shared.borrow());
    let redraw = {
        let mut page = shared.borrow_mut();
        let changed = page.chat_colors != colors;
        page.chat_colors = colors;
        changed
    };
    let page = shared.borrow();
    let Some(chat) = &page.chat else {
        return;
    };
    let usable = chat.you().is_some() && !chat.closed();
    enable(&page.dom.chat_input, usable);
    enable(&page.dom.chat_send, usable);
    if usable {
        page.dom.chat_input.set_max_length(chat.max_length() as i32);
    }
    if let Some(reason) = chat.take_error() {
        page.dom.error.set_text_content(Some(&translate(&reason)));
        show(&page.dom.error, true);
    }
    let mut fresh = chat.take_fresh();
    let lines = &page.dom.chat_lines;
    let at_bottom = lines.scroll_height() - lines.scroll_top() - lines.client_height() < 8;
    if redraw {
        // The fresh entries are among the kept ones.
        lines.set_text_content(None);
        fresh = chat.entries();
    }
    if fresh.is_empty() {
        return;
    }
    let stamps = chat::setting(chat::TIMESTAMP_KEY, true);
    for entry in fresh {
        let line = chat::setup_line(&entry, stamps);
        let color = entry
            .from_player
            .and_then(|player| chat_color(&page, player))
            .or_else(|| Some("white".to_string()));
        let _ = lines.append_child(&chat::render(&page.dom.document, &line, color));
    }
    if at_bottom {
        lines.set_scroll_top(lines.scroll_height());
    }
}

/// The buttons and the form the page has from the start.
fn wire_static(page: &Shared) {
    let dom_clone = |page: &Shared| {
        let page = page.borrow();
        (
            page.dom.join.clone(),
            page.dom.ready.clone(),
            page.dom.start.clone(),
            page.dom.step_down.clone(),
            page.dom.copy_link.clone(),
        )
    };
    let (join, ready, start, step_down, copy_link) = dom_clone(page);
    let shared = Rc::clone(page);
    listen(&join, "submit", move |event| {
        event.prevent_default();
        let Some(input) = shared
            .borrow()
            .dom
            .document
            .get_element_by_id("nickname")
            .map(|input| input.unchecked_into::<HtmlInputElement>())
        else {
            return;
        };
        let name = input.value();
        if name.is_empty() || !input.check_validity() {
            return;
        }
        if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = storage.set_item(NICKNAME_KEY, &name);
        }
        shared.borrow_mut().nickname = Some(name);
        send_join(&shared);
    });

    let shared = Rc::clone(page);
    listen(&ready, "click", move |_| {
        let state = my_ready(&shared);
        send(
            &shared,
            &object(&[
                ("type", "ready".into()),
                ("state", f64::from((state + 1) % 3).into()),
            ]),
        );
    });
    let shared = Rc::clone(page);
    listen(&ready, "contextmenu", move |event| {
        event.prevent_default();
        send(
            &shared,
            &object(&[("type", "ready".into()), ("state", 0.0.into())]),
        );
    });
    let shared = Rc::clone(page);
    listen(&start, "click", move |_| {
        show(&shared.borrow().dom.error, false);
        send(&shared, &object(&[("type", "launch".into())]));
    });
    let shared = Rc::clone(page);
    listen(&step_down, "click", move |_| {
        send(&shared, &object(&[("type", "step_down".into())]));
    });
    let button = copy_link.clone();
    listen(&copy_link, "click", move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        let link = format!(
            "{}{}",
            window.location().origin().unwrap_or_default(),
            crate::match_base()
        );
        if !crate::write_clipboard(&link) {
            return;
        }
        lang::set_text(&button, "Link copied");
        let restore = {
            let button = button.clone();
            Closure::once_into_js(move || lang::set_text(&button, "Copy link"))
        };
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            restore.unchecked_ref(),
            COPIED_MS,
        );
    });

    // `CancelButton`: back to where a new game starts; leaving frees the
    // slot after the grace (U2).
    let back = page.borrow().dom.back.clone();
    listen(&back, "click", move |_| {
        if let Some(window) = web_sys::window() {
            let _ = window.location().set_href("/");
        }
    });
    for (button, name) in [
        (page.borrow().dom.reset_civs.clone(), "reset_civs"),
        (page.borrow().dom.reset_teams.clone(), "reset_teams"),
    ] {
        let shared = Rc::clone(page);
        listen(&button, "click", move |_| {
            send(&shared, &change(name, JsValue::NULL));
        });
    }
    let rejoin = page.borrow().dom.kicked_rejoin.clone();
    listen(&rejoin, "click", move |_| {
        if let Some(window) = web_sys::window() {
            let _ = window.location().reload();
        }
    });
    wire_ai_config(page);

    // The countdown, from the server's count at the last state.
    let shared = Rc::clone(page);
    let tick = Closure::<dyn FnMut()>::new(move || {
        let page = shared.borrow();
        if page.closes_at == 0.0 {
            return;
        }
        let left = ((page.closes_at - js_sys::Date::now()) / 1000.0).max(0.0) as u64;
        let time = format!("{}:{:02}", left / 60, left % 60);
        page.dom.countdown.set_text_content(Some(&lang::format(
            &translate("This setup closes in %(time)s unless it starts."),
            &[("time", &time)],
        )));
    });
    if let Some(window) = web_sys::window() {
        let _ = window.set_interval_with_callback_and_timeout_and_arguments_0(
            tick.as_ref().unchecked_ref(),
            1000,
        );
    }
    tick.forget();
}

/// `AIConfigPage`: the bot, its difficulty and behavior for the slot the
/// gear was pressed on; OK or a click beside it closes it.
fn wire_ai_config(page: &Shared) {
    let (bot, difficulty, behavior, ok, backdrop) = {
        let page = page.borrow();
        let dom = &page.dom;
        (
            dom.ai_bot.clone(),
            dom.ai_difficulty.clone(),
            dom.ai_behavior.clone(),
            dom.ai_ok.clone(),
            dom.ai_config.clone(),
        )
    };
    let slot = |page: &Shared| page.borrow().ai_slot;
    {
        let shared = Rc::clone(page);
        let target = bot.clone();
        listen(&bot, "change", move |_| {
            let Some(index) = slot(&shared) else {
                return;
            };
            let value = target.value();
            let bot = if value.is_empty() {
                JsValue::NULL
            } else {
                value.into()
            };
            send(&shared, &change_at("ai_bot", index, bot));
        });
    }
    {
        let shared = Rc::clone(page);
        let target = difficulty.clone();
        listen(&difficulty, "change", move |_| {
            if let Some(index) = slot(&shared) {
                let level = target.value().parse::<f64>().unwrap_or(f64::NAN);
                send(&shared, &change_at("ai_difficulty", index, level.into()));
            }
        });
    }
    {
        let shared = Rc::clone(page);
        let target = behavior.clone();
        listen(&behavior, "change", move |_| {
            if let Some(index) = slot(&shared) {
                send(
                    &shared,
                    &change_at("ai_behavior", index, target.value().into()),
                );
            }
        });
    }
    let close = |page: &Shared| {
        let mut page = page.borrow_mut();
        page.ai_slot = None;
        show(&page.dom.ai_config, false);
    };
    let shared = Rc::clone(page);
    listen(&ok, "click", move |_| close(&shared));
    let shared = Rc::clone(page);
    let target = backdrop.clone();
    listen(&backdrop, "click", move |event| {
        // Only a click on the fade itself, not inside the dialog.
        let outside = event
            .target()
            .and_then(|clicked| clicked.dyn_into::<Element>().ok())
            .is_some_and(|clicked| clicked == *target.unchecked_ref::<Element>());
        if outside {
            close(&shared);
        }
    });
}

fn my_ready(page: &Shared) -> u8 {
    let page = page.borrow();
    let you = get(&page.state, "you").as_f64();
    list(&page.state, "members")
        .iter()
        .find(|member| Some(number(member, "index")) == you)
        .map_or(0, |member| number(member, "ready") as u8)
}

fn send(page: &Shared, message: &JsValue) {
    let page = page.borrow();
    let Some(socket) = &page.socket else {
        return;
    };
    if let Ok(json) = js_sys::JSON::stringify(message)
        && let Some(json) = json.as_string()
    {
        let _ = socket.send_with_str(&json);
    }
}

fn send_join(page: &Shared) {
    let Some(name) = page.borrow().nickname.clone() else {
        return;
    };
    send(
        page,
        &object(&[("type", "join".into()), ("nickname", name.into())]),
    );
}

fn connect(page: &Shared) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let location = window.location();
    let scheme = match location.protocol().as_deref() {
        Ok("https:") => "wss",
        _ => "ws",
    };
    let url = format!(
        "{scheme}://{}{}/setup",
        location.host().unwrap_or_default(),
        crate::match_base()
    );
    let Ok(socket) = WebSocket::new(&url) else {
        web_sys::console::error_1(&format!("cannot open {url}").into());
        return;
    };
    page.borrow_mut().socket = Some(socket.clone());

    let shared = Rc::clone(page);
    let on_open = Closure::<dyn FnMut()>::new(move || {
        show(&shared.borrow().dom.connection, false);
        send_join(&shared);
    });
    socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    on_open.forget();

    let shared = Rc::clone(page);
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Some(body) = event.data().as_string() else {
            return;
        };
        let Ok(message) = js_sys::JSON::parse(&body) else {
            return;
        };
        on_message_value(&shared, message);
    });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    let shared = Rc::clone(page);
    let on_close = Closure::<dyn FnMut()>::new(move || {
        let delay = {
            let mut page = shared.borrow_mut();
            if page.done {
                return;
            }
            show(&page.dom.connection, true);
            let delay = page.retry_ms;
            page.retry_ms = (delay * 2).min(RETRY_MAX_MS);
            delay
        };
        let again = Rc::clone(&shared);
        let retry = Closure::once_into_js(move || connect(&again));
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                retry.unchecked_ref(),
                delay,
            );
        }
    });
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();
}

/// Stores the host's settings for the next setup, on every state while the
/// viewer hosts (D34).
fn save_settings(state: &JsValue) {
    let you = get(state, "you").as_f64();
    if you.is_none() || you != get(state, "host").as_f64() {
        return;
    }
    let Some(json) = js_sys::JSON::stringify(&get(state, "attributes"))
        .ok()
        .and_then(|json| json.as_string())
    else {
        return;
    };
    let Ok(copy) = js_sys::JSON::parse(&json) else {
        return;
    };
    let gui_data = js_sys::Object::new();
    let filter = get(&get(state, "gui"), "map_filter");
    if Reflect::set(&gui_data, &"mapFilter".into(), &filter).is_err() {
        return;
    }
    // `Settings::open` reads the filter from `guiData`, as vanilla's
    // `GameSettingsGUI.getSettings` writes it.
    if Reflect::set(&copy, &"guiData".into(), &gui_data).is_err() {
        return;
    }
    let Some(serialized) = js_sys::JSON::stringify(&copy)
        .ok()
        .and_then(|json| json.as_string())
    else {
        return;
    };
    let storage = web_sys::window().and_then(|window| window.local_storage().ok().flatten());
    if let Some(storage) = storage {
        let _ = storage.set_item(SETTINGS_KEY, &serialized);
    }
}

fn on_message_value(page: &Shared, message: JsValue) {
    match text(&message, "type").as_str() {
        "catalog" => {
            page.borrow_mut().catalog = message;
        }
        "state" => {
            save_settings(&message);
            {
                let mut page = page.borrow_mut();
                page.retry_ms = RETRY_MS;
                page.closes_at = js_sys::Date::now() + number(&message, "closes_in_ms");
                page.state = message;
            }
            render(page);
            render_chat(page);
        }
        "launched" => {
            page.borrow_mut().done = true;
            // The server now serves the game page at this address.
            if let Some(window) = web_sys::window() {
                let _ = window.location().reload();
            }
        }
        "closed" => {
            let mut page = page.borrow_mut();
            page.done = true;
            page.ai_slot = None;
            show(&page.dom.ai_config, false);
            map_browser::close(&mut page);
            show(&page.dom.room, false);
            show(&page.dom.join, false);
            show(&page.dom.connection, false);
            show(&page.dom.countdown, false);
            show(&page.dom.closed, true);
            let reason = match text(&message, "reason").as_str() {
                "timeout" => "It was not started within 10 minutes.",
                "empty" => "Everyone left it.",
                "stopping" => "The server is shutting down.",
                _ => "Its host opened another game.",
            };
            lang::set_text(&page.dom.closed_reason, reason);
        }
        "kicked" => {
            let mut page = page.borrow_mut();
            page.done = true;
            page.ai_slot = None;
            show(&page.dom.ai_config, false);
            map_browser::close(&mut page);
            show(&page.dom.room, false);
            show(&page.dom.join, false);
            show(&page.dom.connection, false);
            show(&page.dom.countdown, false);
            show(&page.dom.closed, false);
            show(&page.dom.kicked, true);
            let ban = flag(&message, "ban");
            lang::set_text(
                &page.dom.kicked_text,
                if ban {
                    "You are banned from this match."
                } else {
                    "You were kicked from this setup."
                },
            );
            show(&page.dom.kicked_rejoin, !ban);
        }
        "error" => {
            let page = page.borrow();
            let reason = translate(&text(&message, "text"));
            page.dom.error.set_text_content(Some(&lang::format(
                &translate("Cannot start the game: %(error)s"),
                &[("error", &reason)],
            )));
            show(&page.dom.error, true);
        }
        _ => {}
    }
}

// ---- building ----

struct Spec {
    key: &'static str,
    label: &'static str,
    tooltip: &'static str,
    kind: Kind,
}

enum Kind {
    Select(Message),
    Check(Message),
    Range(Message, Caption),
    Button(Rc<dyn Fn(&Shared)>),
}

fn select_spec(
    key: &'static str,
    label: &'static str,
    tooltip: &'static str,
    name: &'static str,
) -> Spec {
    Spec {
        key,
        label,
        tooltip,
        kind: Kind::Select(message_text(name)),
    }
}

fn number_spec(
    key: &'static str,
    label: &'static str,
    tooltip: &'static str,
    name: &'static str,
) -> Spec {
    Spec {
        key,
        label,
        tooltip,
        kind: Kind::Select(message_number(name)),
    }
}

fn check_spec(
    key: &'static str,
    label: &'static str,
    tooltip: &'static str,
    name: &'static str,
) -> Spec {
    Spec {
        key,
        label,
        tooltip,
        kind: Kind::Check(message_flag(name)),
    }
}

fn range_spec(
    key: &'static str,
    label: &'static str,
    tooltip: &'static str,
    name: &'static str,
    caption: Caption,
) -> Spec {
    Spec {
        key,
        label,
        tooltip,
        kind: Kind::Range(message_number(name), caption),
    }
}

/// `GameSettingsLayout.js`'s tabs and their controls, in its order.
fn layout() -> Vec<(&'static str, Vec<Spec>)> {
    let victory: Caption = Rc::new(|value| {
        if value == 0.0 {
            tc("victory duration", "Immediate Victory.")
        } else {
            plural(
                "victory duration",
                "%(min)s minute",
                "%(min)s minutes",
                "min",
                value,
            )
        }
    });
    vec![
        (
            "Map",
            vec![
                select_spec("map_type", "Map Type", "Select a map type.", "map_type"),
                select_spec(
                    "map_filter",
                    "Map Filter",
                    "Select a map filter.",
                    "map_filter",
                ),
                select_spec("map", "Select Map", "Select a map to play on.", "map"),
                Spec {
                    key: "map_browser",
                    label: "",
                    tooltip: "Click to view the list of available maps.",
                    kind: Kind::Button(Rc::new(map_browser::open)),
                },
                number_spec(
                    "map_size",
                    "Map Size",
                    "Select map size. (Larger sizes may reduce performance.)",
                    "map_size",
                ),
                select_spec(
                    "placement",
                    "Player Placement",
                    "Select one of the starting position patterns of this map.",
                    "placement",
                ),
                Spec {
                    key: "landscape",
                    label: "Landscape",
                    tooltip: "Select one of the landscapes of this map.",
                    kind: Kind::Select(Rc::new(|input| {
                        let value = input.text();
                        match value.strip_prefix("group:") {
                            Some(id) => object(&[
                                ("type", "change".into()),
                                ("name", "landscape".into()),
                                ("value", id.into()),
                                ("group", true.into()),
                            ]),
                            None => change("landscape", value.into()),
                        }
                    })),
                },
                select_spec("biome", "Biome", "Select the flora and fauna.", "biome"),
                select_spec(
                    "water_level",
                    "Water level",
                    "Select the water level of this map.",
                    "water_level",
                ),
                range_spec(
                    "sea_level_rise",
                    "Sea Level Rise Time",
                    "Set the time when the water will start to rise.",
                    "sea_level_rise",
                    minutes_caption("sea level rise time"),
                ),
                select_spec(
                    "daytime",
                    "Daytime",
                    "Select whether the match takes place at daylight or night.",
                    "daytime",
                ),
                number_spec(
                    "trigger_difficulty",
                    "Difficulty",
                    "Select the difficulty of this scenario.",
                    "trigger_difficulty",
                ),
                check_spec(
                    "nomad",
                    "Nomad",
                    "In Nomad mode, players start with only few units and have to find a suitable place to build their city. Ceasefire is recommended.",
                    "nomad",
                ),
                check_spec(
                    "disable_treasures",
                    "Disable Treasures",
                    "Do not add treasures to the map.",
                    "disable_treasures",
                ),
                check_spec(
                    "explored",
                    "Explored Map",
                    "Toggle explored map (see initial map).",
                    "explored",
                ),
                check_spec(
                    "revealed",
                    "Revealed Map",
                    "Toggle revealed map (see everything).",
                    "revealed",
                ),
                check_spec(
                    "allied",
                    "Allied View",
                    "Toggle allied view (see what your allies see).",
                    "allied",
                ),
            ],
        ),
        (
            "Player",
            vec![
                number_spec(
                    "player_count",
                    "Number of Players",
                    "Select number of players.",
                    "player_count",
                ),
                select_spec(
                    "population_cap_type",
                    "Population Cap Type",
                    "Select a population capacity type.",
                    "population_cap_type",
                ),
                number_spec(
                    "population_cap",
                    // Both come from the capacity type (`PopulationCap.js`).
                    "",
                    "",
                    "population_cap",
                ),
                number_spec(
                    "starting_resources",
                    "Starting Resources",
                    "Select the game's starting resources.",
                    "starting_resources",
                ),
                check_spec(
                    "disable_spies",
                    "Disable Spies",
                    "Disable spies during the game.",
                    "disable_spies",
                ),
                check_spec(
                    "cheats",
                    "Cheats",
                    "Toggle the usability of cheats.",
                    "cheats",
                ),
            ],
        ),
        (
            "Game Type",
            vec![
                // The victory conditions come from the catalog, in `build`.
                range_spec(
                    "relic_count",
                    "Relic Count",
                    "Total number of relics spawned on the map. Relic victory is most realistic with only one or two relics. With greater numbers, the relics are important to capture to receive aura bonuses.",
                    "relic_count",
                    Rc::new(|value| {
                        plural("", "%(number)s relic", "%(number)s relics", "number", value)
                    }),
                ),
                range_spec(
                    "relic_duration",
                    "Relic Duration",
                    "Minutes until the player has achieved Relic Victory.",
                    "relic_duration",
                    Rc::clone(&victory),
                ),
                check_spec(
                    "regicide_garrison",
                    "Hero Garrison",
                    "Toggle whether heroes can be garrisoned.",
                    "regicide_garrison",
                ),
                range_spec(
                    "wonder_duration",
                    "Wonder Duration",
                    "Minutes until the player has achieved Wonder Victory",
                    "wonder_duration",
                    victory,
                ),
                Spec {
                    key: "game_speed",
                    label: "Game Speed",
                    tooltip: "Select game speed.",
                    kind: Kind::Select(message_number("game_speed")),
                },
                range_spec(
                    "ceasefire",
                    "Ceasefire",
                    "Set time where no attacks are possible.",
                    "ceasefire",
                    Rc::new(|value| {
                        if value == 0.0 {
                            tc("ceasefire", "No ceasefire")
                        } else {
                            plural(
                                "ceasefire",
                                "%(minutes)s minute",
                                "%(minutes)s minutes",
                                "minutes",
                                value,
                            )
                        }
                    }),
                ),
                check_spec(
                    "locked_teams",
                    "Teams Locked",
                    "Toggle locked teams.",
                    "locked_teams",
                ),
                check_spec(
                    "last_man_standing",
                    "Last Man Standing",
                    "Toggle whether the last remaining player or the last remaining set of allies wins.",
                    "last_man_standing",
                ),
            ],
        ),
    ]
}

fn build_control(page: &Shared, dom: &Dom, panel: &HtmlElement, spec: Spec) -> Control {
    let document = &dom.document;
    let wrap: HtmlElement = create(document, "div");
    wrap.set_class_name("setting");
    tip(&wrap, &translate(spec.tooltip));
    let name: HtmlElement = create(document, "label");
    name.set_class_name("name");
    if !spec.label.is_empty() {
        name.set_text_content(Some(&titled(&translate(spec.label))));
    }
    let value: HtmlElement = create(document, "div");
    value.set_class_name("value");
    let mut caption = None;
    let mut make_caption = None;
    let input: HtmlElement = match spec.kind {
        Kind::Select(message) => {
            let select: HtmlSelectElement = create(document, "select");
            let shared = Rc::clone(page);
            let target = select.clone();
            listen(&select, "change", move |_| {
                send(&shared, &message(Input::Text(target.value())));
            });
            select.unchecked_into()
        }
        Kind::Check(message) => {
            let check: HtmlInputElement = create(document, "input");
            check.set_type("checkbox");
            let shared = Rc::clone(page);
            let target = check.clone();
            listen(&check, "change", move |_| {
                send(&shared, &message(Input::Bool(target.checked())));
            });
            caption = Some(create(document, "span"));
            check.unchecked_into()
        }
        Kind::Button(action) => {
            let button: HtmlButtonElement = create(document, "button");
            button.set_type("button");
            button.set_class_name("red");
            button.set_text_content(Some(&translate("Browse Maps")));
            let shared = Rc::clone(page);
            listen(&button, "click", move |_| action(&shared));
            button.unchecked_into()
        }
        Kind::Range(message, make) => {
            let range: HtmlInputElement = create(document, "input");
            range.set_type("range");
            range.set_step("1");
            let label: HtmlElement = create(document, "span");
            // The caption follows the thumb; only a release sends.
            let (shown, update) = (label.clone(), Rc::clone(&make));
            let target = range.clone();
            listen(&range, "input", move |_| {
                shown.set_text_content(Some(&update(target.value_as_number())));
            });
            let shared = Rc::clone(page);
            let target = range.clone();
            listen(&range, "change", move |_| {
                send(&shared, &message(Input::Num(target.value_as_number())));
            });
            caption = Some(label);
            make_caption = Some(make);
            range.unchecked_into()
        }
    };
    let _ = value.append_child(&input);
    if let Some(caption) = &caption {
        let _ = value.append_child(caption);
    }
    let _ = wrap.append_child(&name);
    let _ = wrap.append_child(&value);
    let _ = panel.append_child(&wrap);
    Control {
        wrap,
        input,
        name,
        caption,
        make_caption,
    }
}

fn build(page: &Shared, state: &mut Page) {
    let catalog = state.catalog.clone();
    let mut tab_buttons = Vec::new();
    let mut tab_panels = Vec::new();
    let mut controls = HashMap::new();
    let layout = layout();
    for (index, (title, specs)) in layout.into_iter().enumerate() {
        let document = state.dom.document.clone();
        let button: HtmlElement = create(&document, "button");
        let _ = button.set_attribute("type", "button");
        let _ = button.set_attribute("role", "tab");
        button.set_text_content(Some(&tc("Match settings tab name", title)));
        let shared = Rc::clone(page);
        listen(&button, "click", move |_| {
            select_tab(&mut shared.borrow_mut(), index)
        });
        let _ = state.dom.tabs.append_child(&button);
        let panel: HtmlElement = create(&document, "div");
        panel.set_class_name("tab");
        let _ = state.dom.settings.append_child(&panel);
        for spec in specs {
            let key = spec.key;
            if key == "relic_count" {
                // The victory conditions come first on the tab, as in
                // vanilla, one checkbox for each.
                for condition in list(&catalog, "victory_conditions") {
                    let name = text(&condition, "name");
                    let spec_name = name.clone();
                    let message: Message = Rc::new(move |input| {
                        object(&[
                            ("type", "change".into()),
                            ("name", "victory_condition".into()),
                            ("value", spec_name.as_str().into()),
                            ("enabled", input.flag().into()),
                        ])
                    });
                    let control = build_control(
                        page,
                        &state.dom,
                        &panel,
                        Spec {
                            key: "victory",
                            label: "",
                            tooltip: "",
                            kind: Kind::Check(message),
                        },
                    );
                    control
                        .name
                        .set_text_content(Some(&titled(&translate(&text(&condition, "title")))));
                    tip(&control.wrap, &translate(&text(&condition, "description")));
                    controls.insert(format!("victory:{name}"), control);
                }
            }
            let control = build_control(page, &state.dom, &panel, spec);
            controls.insert(key.to_string(), control);
        }
        tab_buttons.push(button);
        tab_panels.push(panel);
    }
    state.controls = controls;
    state.tab_buttons = tab_buttons;
    state.tab_panels = tab_panels;
    select_tab(state, 0);
}

fn select_tab(page: &mut Page, selected: usize) {
    for (index, (button, panel)) in page.tab_buttons.iter().zip(&page.tab_panels).enumerate() {
        let _ = button.set_attribute(
            "aria-selected",
            if index == selected { "true" } else { "false" },
        );
        show(panel, index == selected);
    }
}

/// The member a row's or a spectator's buttons act on, read when pressed so
/// a redraw does not need new handlers.
fn member_name(page: &Page, member: usize) -> String {
    list(&page.state, "members")
        .iter()
        .find(|entry| number(entry, "index") as usize == member)
        .map(|entry| text(entry, "name"))
        .unwrap_or_default()
}

/// The host's Kick and Ban buttons for one member (D54).
fn member_buttons(
    page: &Shared,
    document: &Document,
    member: Rc<Cell<Option<usize>>>,
) -> (HtmlButtonElement, HtmlButtonElement) {
    let kick: HtmlButtonElement = create(document, "button");
    kick.set_class_name("red take");
    kick.set_text_content(Some(&translate("Kick")));
    let _ = kick.set_attribute("type", "button");
    let ban: HtmlButtonElement = create(document, "button");
    ban.set_class_name("red take");
    ban.set_text_content(Some(&translate("Ban")));
    let _ = ban.set_attribute("type", "button");
    {
        let shared = Rc::clone(page);
        let member = Rc::clone(&member);
        listen(&kick, "click", move |_| {
            let Some(member) = member.get() else {
                return;
            };
            send(
                &shared,
                &object(&[
                    ("type", "kick".into()),
                    ("member", (member as f64).into()),
                    ("ban", false.into()),
                ]),
            );
        });
    }
    {
        let shared = Rc::clone(page);
        let member = Rc::clone(&member);
        listen(&ban, "click", move |_| {
            let Some(index) = member.get() else {
                return;
            };
            let text = {
                let page = shared.borrow();
                let name = member_name(&page, index);
                lang::format(
                    &translate("Ban %(name)s from this match? They cannot join it again."),
                    &[("name", &name)],
                )
            };
            let confirmed = web_sys::window()
                .and_then(|window| window.confirm_with_message(&text).ok())
                .unwrap_or(false);
            if !confirmed {
                return;
            }
            send(
                &shared,
                &object(&[
                    ("type", "kick".into()),
                    ("member", (index as f64).into()),
                    ("ban", true.into()),
                ]),
            );
        });
    }
    (kick, ban)
}

fn build_row(page: &Shared, state: &Page, index: usize) -> Row {
    let document = &state.dom.document;
    let line: HtmlElement = create(document, "div");
    line.set_class_name("slot slot-grid");
    let _ = state.dom.players.append_child(&line);
    let cell = || -> HtmlElement {
        let cell: HtmlElement = create(document, "div");
        let _ = line.append_child(&cell);
        cell
    };
    let name_cell = cell();
    name_cell.set_class_name("name");
    // The name in its own span: the cell also holds the Kick and Ban
    // buttons, which a render must not clear with the text.
    let name: HtmlElement = create(document, "span");
    let _ = name_cell.append_child(&name);
    let member = Rc::new(Cell::new(None));
    let (kick, ban) = member_buttons(page, document, Rc::clone(&member));
    let _ = name_cell.append_child(&kick);
    let _ = name_cell.append_child(&ban);
    let color_cell = cell();
    let who_cell = cell();
    let gear_cell = cell();
    let civ_cell = cell();
    let team_cell = cell();

    let color: HtmlSelectElement = create(document, "select");
    color.set_class_name("color");
    tip(&color, &translate("Pick a color."));
    let who: HtmlSelectElement = create(document, "select");
    tip(&who, &translate("Select player."));
    let who_text: HtmlElement = create(document, "div");
    who_text.set_class_name("who-text");
    let take: HtmlButtonElement = create(document, "button");
    take.set_class_name("red take");
    take.set_text_content(Some(&translate("Take slot")));
    let _ = take.set_attribute("type", "button");
    let gear: HtmlButtonElement = create(document, "button");
    gear.set_class_name("gear");
    let _ = gear.set_attribute("type", "button");
    let civ: HtmlSelectElement = create(document, "select");
    tip(&civ, &translate("Choose the civilization for this player."));
    let team: HtmlSelectElement = create(document, "select");
    tip(&team, &translate("Select player's team."));
    let _ = color_cell.append_child(&color);
    let _ = who_cell.append_child(&who);
    let _ = who_cell.append_child(&who_text);
    let _ = who_cell.append_child(&take);
    let _ = gear_cell.append_child(&gear);
    let _ = civ_cell.append_child(&civ);
    let _ = team_cell.append_child(&team);

    let slot_message = |name: &'static str, target: &HtmlSelectElement, numeric: bool| {
        let shared = Rc::clone(page);
        let target = target.clone();
        listen(&target.clone(), "change", move |_| {
            let value = target.value();
            let json = if numeric {
                change_at(name, index, value.parse::<f64>().unwrap_or(f64::NAN).into())
            } else {
                change_at(name, index, value.as_str().into())
            };
            send(&shared, &json);
        });
    };
    slot_message("civ", &civ, false);
    slot_message("team", &team, true);
    {
        let shared = Rc::clone(page);
        let target = color.clone();
        listen(&color, "change", move |_| {
            let channels: Array = target
                .value()
                .split(',')
                .map(|channel| JsValue::from_f64(channel.parse().unwrap_or(0.0)))
                .collect();
            send(&shared, &change_at("color", index, channels.into()));
        });
    }
    {
        let shared = Rc::clone(page);
        let target = who.clone();
        listen(&who, "change", move |_| {
            let value = target.value();
            let json = if let Some(member) = value.strip_prefix("member:") {
                object(&[
                    ("type", "move".into()),
                    ("member", member.parse::<f64>().unwrap_or(f64::NAN).into()),
                    ("slot", (index as f64).into()),
                ])
            } else if let Some(bot) = value.strip_prefix("ai:") {
                change_at("ai", index, bot.into())
            } else if value == "reserved" {
                return;
            } else if value == "removed" {
                object(&[
                    ("type", "change".into()),
                    ("name", "remove".into()),
                    ("index", (index as f64).into()),
                ])
            } else {
                object(&[
                    ("type", "change".into()),
                    ("name", "unassign".into()),
                    ("index", (index as f64).into()),
                ])
            };
            send(&shared, &json);
        });
    }
    {
        let shared = Rc::clone(page);
        listen(&take, "click", move |_| {
            send(
                &shared,
                &object(&[("type", "take".into()), ("slot", (index as f64).into())]),
            );
        });
    }
    {
        let shared = Rc::clone(page);
        listen(&gear, "click", move |_| {
            shared.borrow_mut().ai_slot = Some(index);
            render(&shared);
        });
    }
    Row {
        line,
        name,
        color,
        who,
        who_text,
        take,
        gear,
        civ,
        team,
        kick,
        ban,
        member,
    }
}

fn change_at(name: &str, index: usize, value: JsValue) -> JsValue {
    object(&[
        ("type", "change".into()),
        ("name", name.into()),
        ("index", (index as f64).into()),
        ("value", value),
    ])
}

// ---- rendering ----

fn render(shared: &Shared) {
    let mut guard = shared.borrow_mut();
    let page = &mut *guard;
    if page.catalog.is_undefined() || page.state.is_undefined() {
        return;
    }
    if !page.built {
        page.built = true;
        build(shared, page);
    }
    let view = View::new(page);
    render_chrome(page, &view);
    render_controls(page, &view);
    render_players(shared, page, &view);
    render_spectators(shared, page, &view);
    map_browser::sync_host(page, &view);
}

/// What one update reads, resolved once.
struct View {
    catalog: JsValue,
    state: JsValue,
    attributes: JsValue,
    settings: JsValue,
    offer: JsValue,
    members: Vec<JsValue>,
    reserved: Vec<JsValue>,
    you: Option<usize>,
    host: bool,
    seated: Option<usize>,
    scenario: bool,
    count: usize,
}

impl View {
    fn new(page: &Page) -> View {
        let state = page.state.clone();
        let attributes = get(&state, "attributes");
        let offer = get(&state, "offer");
        let members = list(&state, "members");
        let you = get(&state, "you").as_f64().map(|index| index as usize);
        let host_index = get(&state, "host").as_f64().map(|index| index as usize);
        let seated = you.and_then(|you| {
            members
                .iter()
                .find(|member| number(member, "index") as usize == you)
                .and_then(|member| get(member, "player").as_f64())
                .map(|player| player as usize)
        });
        View {
            catalog: page.catalog.clone(),
            settings: get(&attributes, "settings"),
            scenario: flag(&offer, "scenario"),
            count: number(&state, "player_count") as usize,
            host: you.is_some() && you == host_index,
            attributes,
            offer,
            members,
            reserved: list(&state, "reserved"),
            you,
            seated,
            state,
        }
    }

    fn player(&self, index: usize) -> JsValue {
        list(&self.settings, "PlayerData")
            .into_iter()
            .nth(index)
            .unwrap_or(JsValue::UNDEFINED)
    }
}

fn render_chrome(page: &Page, view: &View) {
    let dom = &page.dom;
    show(&dom.join, view.you.is_none());
    show(&dom.countdown, true);
    // `MapPreview`'s name and the stock image `session-gui` wrote for it.
    dom.preview_title
        .set_text_content(Some(&map_name(view, &text(&view.attributes, "map"))));
    let key = preview_key(&view.attributes);
    let _ = dom.preview.style().set_property(
        "background-image",
        &format!("var(--gui-mappreview-{key}, var(--gui-mappreview-nopreview))"),
    );
    render_description(dom, view);

    show(&dom.step_down, view.seated.is_some());
    let my_ready = view
        .you
        .and_then(|you| {
            view.members
                .iter()
                .find(|member| number(member, "index") as usize == you)
        })
        .map_or(0, |member| number(member, "ready") as usize);
    show(&dom.ready, view.seated.is_some() && !view.host);
    const CAPTIONS: [&str; 3] = ["I'm ready", "Stay ready", "I'm not ready!"];
    const TOOLTIPS: [&str; 3] = [
        "State that you are ready to play.",
        "Stay ready even when the game settings change.",
        "State that you are not ready to play.",
    ];
    dom.ready
        .set_text_content(Some(&translate(CAPTIONS[my_ready.min(2)])));
    tip(&dom.ready, &translate(TOOLTIPS[my_ready.min(2)]));
    show(&dom.start, view.host);
    let blocker = get(&view.state, "launch_blocker").as_string();
    enable(&dom.start, blocker.is_none());
    tip(
        &dom.start,
        &translate(if blocker.is_none() {
            "Start a new game with the current settings."
        } else {
            "Start a new game with the current settings (disabled until all players are ready)."
        }),
    );
    if let Some(reason) = blocker.as_deref().filter(|_| view.host) {
        // Also what the button's wait is for.
        let _ = dom
            .start
            .set_attribute("aria-description", &translate(reason));
    }
}

/// The key `session-gui` writes for the map's stock preview, `nopreview`
/// for none. A landscape preview is not existence-checked upstream and the
/// value goes into CSS.
fn preview_key(attributes: &JsValue) -> String {
    preview_stem(&text(&get(attributes, "settings"), "mapPreview"))
}

/// `preview_key` of a preview file name or path.
fn preview_stem(preview: &str) -> String {
    let name = preview.rsplit('/').next().unwrap_or_default();
    let stem = name.strip_suffix(".png").unwrap_or(name);
    if stem.is_empty()
        || !stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return "nopreview".to_string();
    }
    stem.to_string()
}

/// `MapCache.translateMapName` of `path`'s map.
fn map_name(view: &View, path: &str) -> String {
    if path == "random" {
        return tc("map selection", "Random");
    }
    list(&view.catalog, "maps")
        .into_iter()
        .find(|map| text(map, "path") == path)
        .map(|map| translate(&text(&map, "name")))
        .unwrap_or_default()
}

/// A line's details in `getGameDescription`: a flag reads enabled or
/// disabled, as does an empty text.
enum Details {
    Text(String),
    Flag(bool),
}

/// `getGameDescription` (`gui/common/gamedescription.js`), from the
/// settings as they stand; the rated game line is left out, since no game
/// here is rated (V58).
fn game_description(view: &View) -> Vec<(String, Details)> {
    let settings = &view.settings;
    let catalog = &view.catalog;
    let mut lines = Vec::new();
    let text_line = |label: String, details: String| (label, Details::Text(details));
    let active = strings(settings, "VictoryConditions");
    if active.is_empty() {
        lines.push(text_line(
            tc("victory condition", "Endless Game"),
            translate("No winner will be determined, even if everyone is defeated."),
        ));
    }
    let mut conditions = list(catalog, "victory_conditions");
    conditions.sort_by(|a, b| number(a, "gui_order").total_cmp(&number(b, "gui_order")));
    for condition in conditions {
        let name = text(&condition, "name");
        if !active.contains(&name) {
            continue;
        }
        let minutes = |key: &str, singular: &str, plural_text: &str| {
            let value = number(settings, key).round();
            lang::format(
                &lang::translate_plural_context("victory condition", singular, plural_text, value),
                &[("min", &value.to_string())],
            )
        };
        let title = match name.as_str() {
            "wonder" => minutes(
                "WonderDuration",
                "Wonder (%(min)s minute)",
                "Wonder (%(min)s minutes)",
            ),
            "capture_the_relic" => minutes(
                "RelicDuration",
                "Capture the Relic (%(min)s minute)",
                "Capture the Relic (%(min)s minutes)",
            ),
            _ => translate(&text(&condition, "title")),
        };
        lines.push(text_line(
            title,
            translate(&text(&condition, "description")),
        ));
        if name == "capture_the_relic" {
            lines.push(text_line(
                translate("Relic Count"),
                number(settings, "RelicCount").round().to_string(),
            ));
        }
        if name == "regicide" {
            lines.push(if flag(settings, "RegicideGarrison") {
                text_line(
                    translate("Hero Garrison"),
                    translate("Heroes can be garrisoned."),
                )
            } else {
                text_line(
                    translate("Exposed Heroes"),
                    translate("Heroes cannot be garrisoned and they are vulnerable to raids."),
                )
            });
        }
    }
    lines.push(if flag(settings, "LockTeams") {
        text_line(
            translate("Locked Teams"),
            translate("Players can't change the initial teams."),
        )
    } else {
        text_line(
            translate("Diplomacy"),
            translate("Players can make alliances and declare war on allies."),
        )
    });
    lines.push(if flag(settings, "LastManStanding") {
        text_line(
            translate("Last Man Standing"),
            translate(
                "Only one player can win the game. If the remaining players are allies, the game continues until only one remains.",
            ),
        )
    } else {
        text_line(
            translate("Allied Victory"),
            translate(
                "If one player wins, his or her allies win too. If one group of allies remains, they win.",
            ),
        )
    });
    let ceasefire = number(settings, "Ceasefire").round();
    lines.push(text_line(
        translate("Ceasefire"),
        if ceasefire == 0.0 {
            translate("disabled")
        } else if ceasefire == 1.0 {
            translate("For the first minute, other players will stay neutral.")
        } else {
            lang::format(
                &lang::translate_plural(
                    "For the first %(min)s minute, other players will stay neutral.",
                    "For the first %(min)s minutes, other players will stay neutral.",
                    ceasefire,
                ),
                &[("min", &ceasefire.to_string())],
            )
        },
    ));
    let path = text(&view.attributes, "map");
    if path == "random" {
        lines.push(text_line(
            tc("Map Selection", "Random Map"),
            translate("Randomly select a map from the list."),
        ));
    } else {
        lines.push(text_line(translate("Map Name"), map_name(view, &path)));
        let description = list(catalog, "maps")
            .into_iter()
            .find(|map| text(map, "path") == path)
            .map(|map| translate(&text(&map, "description")))
            .unwrap_or_default();
        lines.push(text_line(translate("Map Description"), description));
    }
    let kind = text(&view.attributes, "mapType");
    lines.push(text_line(
        translate("Map Type"),
        tc("map", &title_case(&kind)),
    ));
    if kind == "random" {
        let size = number(settings, "Size");
        if let Some(entry) = list(catalog, "map_sizes")
            .into_iter()
            .find(|entry| number(entry, "tiles") == size)
        {
            lines.push(text_line(
                translate("Map Size"),
                translate(&text(&entry, "name")),
            ));
        }
    }
    if present(settings, "PlayerPlacement") {
        let id = text(settings, "PlayerPlacement");
        lines.push(
            match list(catalog, "player_placements")
                .into_iter()
                .find(|entry| text(entry, "id") == id)
            {
                Some(entry) => text_line(
                    tc("player placement", &text(&entry, "name")),
                    tc("player placement", &text(&entry, "description")),
                ),
                None => text_line(
                    tc("player placement", "Random Player Placement"),
                    translate("Randomly select a player placement from the list."),
                ),
            },
        );
    }
    if present(settings, "Biome") {
        let id = text(settings, "Biome");
        lines.push(
            match list(catalog, "biomes")
                .into_iter()
                .find(|entry| text(entry, "id") == id)
            {
                Some(entry) => text_line(
                    tc("biome definition", &text(&entry, "title")),
                    tc("biome definition", &text(&entry, "description")),
                ),
                None => text_line(
                    tc("biome", "Random Biome"),
                    translate("Randomly select a biome from the list."),
                ),
            },
        );
    }
    if present(settings, "TriggerDifficulty") {
        let difficulty = number(settings, "TriggerDifficulty");
        if let Some(entry) = list(catalog, "trigger_difficulties")
            .into_iter()
            .find(|entry| number(entry, "difficulty") == difficulty)
        {
            lines.push(text_line(
                translate(&text(&entry, "title")),
                translate(&text(&entry, "tooltip")),
            ));
        }
    }
    if present(settings, "Nomad") {
        lines.push(if flag(settings, "Nomad") {
            text_line(
                translate("Nomad Mode"),
                translate(
                    "Players start with only few units and have to find a suitable place to build their city.",
                ),
            )
        } else {
            text_line(
                translate("Civic Centers"),
                translate("Players start with a Civic Center."),
            )
        });
    }
    let players = list(settings, "PlayerData");
    if present(settings, "StartingResources") {
        let amount = number(settings, "StartingResources");
        let details = if players.iter().any(|player| present(player, "Resources")) {
            tc("starting resources", "Per Player")
        } else {
            let title = list(catalog, "starting_resources")
                .into_iter()
                .find(|entry| number(entry, "resources") == amount)
                .map(|entry| tc("startingResources", &text(&entry, "title")))
                .unwrap_or_default();
            lang::format(
                &translate("%(startingResourcesTitle)s (%(amount)s)"),
                &[
                    ("startingResourcesTitle", &title),
                    ("amount", &amount.to_string()),
                ],
            )
        };
        lines.push(text_line(translate("Starting Resources"), details));
    }
    let cap_type = text(settings, "PopulationCapType");
    let capacity = list(catalog, "population_capacities")
        .into_iter()
        .find(|entry| text(entry, "name") == cap_type);
    if present(settings, "PopulationCapType") {
        lines.push(text_line(
            translate("Population Cap Type"),
            capacity
                .as_ref()
                .map(|entry| translate(&text(entry, "title")))
                .unwrap_or_default(),
        ));
    }
    if present(settings, "PopulationCap") {
        let cap = number(settings, "PopulationCap");
        let details = if players
            .iter()
            .any(|player| present(player, "PopulationLimit"))
        {
            tc("population capacity", "Per Player")
        } else if cap < UNLIMITED_POPULATION {
            cap.to_string()
        } else {
            tc("population capacity", "Unlimited")
        };
        lines.push(text_line(
            capacity
                .as_ref()
                .map(|entry| translate(&text(entry, "cap_title")))
                .unwrap_or_default(),
            details,
        ));
    }
    lines.push(text_line(
        translate("Treasures"),
        if flag(settings, "DisableTreasures") {
            tc("treasures", "Disabled")
        } else {
            tc("treasures", "As defined by the map.")
        },
    ));
    for (label, key) in [
        ("Explored Map", "ExploreMap"),
        ("Revealed Map", "RevealMap"),
        ("Allied View", "AllyView"),
        ("Cheats", "CheatsEnabled"),
    ] {
        lines.push((translate(label), Details::Flag(flag(settings, key))));
    }
    lines
}

/// Each line as `"%(label)s %(details)s"` with the label highlighted.
fn render_description(dom: &Dom, view: &View) {
    let document = &dom.document;
    let element = &dom.description;
    clear(element);
    let template = translate("%(label)s %(details)s");
    for (label, details) in game_description(view) {
        let details = match details {
            Details::Flag(true) => tc("game setup option", "enabled"),
            Details::Flag(false) => tc("game setup option", "disabled"),
            Details::Text(text) if text.is_empty() => tc("game setup option", "disabled"),
            Details::Text(text) => text,
        };
        let line = template.replace("%(details)s", &details);
        let (before, after) = line.split_once("%(label)s").unwrap_or(("", &line));
        let _ = element.append_with_str_1(before);
        let span: HtmlElement = create(document, "span");
        span.set_class_name("label");
        span.set_text_content(Some(&label));
        let _ = element.append_child(&span);
        let _ = element.append_with_str_1(&format!("{after}\n"));
    }
}

fn render_controls(page: &Page, view: &View) {
    let document = &page.dom.document;
    let catalog = &view.catalog;
    let offer = &view.offer;
    let host = view.host;
    let scenario = view.scenario;
    let free = host && !scenario;

    let select =
        |key: &str, visible: bool, enabled: bool, options: &[(String, String)], value: &str| {
            if let Some(control) = page.controls.get(key) {
                show(&control.wrap, visible);
                enable(&control.input, enabled);
                fill(document, control.input.unchecked_ref(), options, value);
            }
        };
    let check = |key: &str, visible: bool, enabled: bool, checked: bool| {
        if let Some(control) = page.controls.get(key) {
            show(&control.wrap, visible);
            enable(&control.input, enabled);
            show(&control.input, enabled);
            control
                .input
                .unchecked_ref::<HtmlInputElement>()
                .set_checked(checked);
            if let Some(caption) = &control.caption {
                show(caption, !enabled);
                let shown = if checked { "Yes" } else { "No" };
                caption.set_text_content(Some(&translate(shown)));
            }
        }
    };
    let range = |key: &str, visible: bool, enabled: bool, min: f64, max: f64, value: f64| {
        if let Some(control) = page.controls.get(key) {
            show(&control.wrap, visible);
            enable(&control.input, enabled);
            show(&control.input, enabled);
            let input: &HtmlInputElement = control.input.unchecked_ref();
            input.set_min(&min.to_string());
            input.set_max(&max.to_string());
            input.set_value(&value.to_string());
            if let (Some(caption), Some(make)) = (&control.caption, &control.make_caption) {
                caption.set_text_content(Some(&make(value)));
            }
        }
    };

    // Map tab
    let kind = text(&view.attributes, "mapType");
    select(
        "map_type",
        true,
        host,
        &strings(catalog, "map_types")
            .iter()
            .map(|name| option(name, tc("map", &title_case(name))))
            .collect::<Vec<_>>(),
        &kind,
    );
    let titles = list(catalog, "map_filters");
    let filters = strings(offer, "map_filters");
    select(
        "map_filter",
        !filters.is_empty(),
        host,
        &filters
            .iter()
            .map(|name| {
                let title = titles
                    .iter()
                    .find(|filter| text(filter, "name") == *name)
                    .map(|filter| text(filter, "title"))
                    .unwrap_or_default();
                option(name, translate(&title))
            })
            .collect::<Vec<_>>(),
        &text(&get(&view.state, "gui"), "map_filter"),
    );
    let maps = list(catalog, "maps");
    select(
        "map",
        true,
        host,
        &strings(offer, "maps")
            .iter()
            .map(|path| {
                if path == "random" {
                    return option(path, tc("map selection", "Random"));
                }
                let name = maps
                    .iter()
                    .find(|map| text(map, "path") == *path)
                    .map(|map| text(map, "name"))
                    .unwrap_or_else(|| path.clone());
                option(path, translate(&name))
            })
            .collect::<Vec<_>>(),
        &text(&view.attributes, "map"),
    );
    select(
        "map_size",
        flag(offer, "map_size"),
        host,
        &list(catalog, "map_sizes")
            .iter()
            .map(|size| option(number(size, "tiles"), translate(&text(size, "name"))))
            .collect::<Vec<_>>(),
        &number(&view.settings, "Size").to_string(),
    );
    let placements = list(offer, "placements");
    let placement_titles = list(catalog, "player_placements");
    let mut options = vec![option("random", tc("player placement", "Random"))];
    options.extend(placements.iter().filter_map(JsValue::as_string).map(|id| {
        let title = placement_titles
            .iter()
            .find(|entry| text(entry, "id") == id)
            .map(|entry| text(entry, "name"))
            .unwrap_or_else(|| id.clone());
        option(id, tc("player placement", &title))
    }));
    select(
        "placement",
        present(offer, "placements"),
        host,
        &options,
        &text(&view.settings, "PlayerPlacement"),
    );
    let groups = list(offer, "landscape");
    let mut options = vec![option("random", tc("landscape selection", "Random"))];
    for group in &groups {
        options.push(option(
            format!("group:{}", text(group, "Id")),
            translate(&text(group, "Name")),
        ));
        for item in list(group, "Items") {
            options.push(option(
                text(&item, "Id"),
                format!("  {}", translate(&text(&item, "Name"))),
            ));
        }
    }
    let landscape = text(&view.settings, "Landscape");
    let landscape = if groups.iter().any(|group| text(group, "Id") == landscape) {
        format!("group:{landscape}")
    } else {
        landscape
    };
    select(
        "landscape",
        present(offer, "landscape"),
        host,
        &options,
        &landscape,
    );
    let biome_titles = list(catalog, "biomes");
    let mut options = vec![option("random", tc("biome", "Random"))];
    options.extend(strings(offer, "biomes").into_iter().map(|id| {
        let title = biome_titles
            .iter()
            .find(|entry| text(entry, "id") == id)
            .map(|entry| text(entry, "title"))
            .unwrap_or_else(|| id.clone());
        option(id, tc("biome definition", &title))
    }));
    select(
        "biome",
        !strings(offer, "biomes").is_empty(),
        host,
        &options,
        &text(&view.settings, "Biome"),
    );
    let water = list(offer, "water_levels");
    select(
        "water_level",
        present(offer, "water_levels"),
        host,
        &water
            .iter()
            .map(|level| option(text(level, "Name"), translate(&text(level, "Name"))))
            .collect::<Vec<_>>(),
        &text(&view.settings, "waterLevel"),
    );
    let rise = get(offer, "sea_level_rise");
    let rise_max = if present(&rise, "max") {
        number(&rise, "max")
    } else {
        60.0
    };
    range(
        "sea_level_rise",
        !rise.is_null(),
        free,
        number(&rise, "min"),
        rise_max,
        number(&view.settings, "SeaLevelRiseTime"),
    );
    let daytime = list(offer, "daytime");
    let mut options = vec![option("random", tc("daytime selection", "Random"))];
    options.extend(
        daytime
            .iter()
            .map(|item| option(text(item, "Id"), translate(&text(item, "Name")))),
    );
    select(
        "daytime",
        present(offer, "daytime"),
        host,
        &options,
        &text(&view.settings, "Daytime"),
    );
    let allowed = strings(offer, "trigger_difficulties");
    select(
        "trigger_difficulty",
        present(offer, "trigger_difficulties"),
        host,
        &list(catalog, "trigger_difficulties")
            .iter()
            .filter(|entry| allowed.contains(&text(entry, "name")))
            .map(|entry| {
                option(
                    number(entry, "difficulty"),
                    translate(&text(entry, "title")),
                )
            })
            .collect::<Vec<_>>(),
        &number(&view.settings, "TriggerDifficulty").to_string(),
    );
    check(
        "nomad",
        flag(offer, "nomad"),
        host,
        flag(&view.settings, "Nomad"),
    );
    check(
        "disable_treasures",
        true,
        free,
        flag(&view.settings, "DisableTreasures"),
    );
    check("explored", true, free, flag(&view.settings, "ExploreMap"));
    check("revealed", true, free, flag(&view.settings, "RevealMap"));
    check("allied", true, free, flag(&view.settings, "AllyView"));

    // Player tab
    let max_players = (number(offer, "player_count_max") as usize).max(1);
    select(
        "player_count",
        true,
        host && kind == "random",
        &(1..=max_players)
            .map(|count| option(count, count))
            .collect::<Vec<_>>(),
        &view.count.to_string(),
    );
    let capacities = list(catalog, "population_capacities");
    let cap_type = text(&view.settings, "PopulationCapType");
    select(
        "population_cap_type",
        true,
        free,
        &capacities
            .iter()
            .map(|entry| option(text(entry, "name"), translate(&text(entry, "title"))))
            .collect::<Vec<_>>(),
        &cap_type,
    );
    let capacity = capacities
        .iter()
        .find(|entry| text(entry, "name") == cap_type);
    let per_player = flag(offer, "pop_per_player");
    let options = if per_player {
        vec![option("0", tc("population limit", "Per Player"))]
    } else {
        capacity
            .map(|entry| {
                list(entry, "options")
                    .iter()
                    .filter_map(JsValue::as_f64)
                    .map(|cap| {
                        let label = if cap < UNLIMITED_POPULATION {
                            cap.to_string()
                        } else {
                            translate("Unlimited")
                        };
                        option(cap, label)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    select(
        "population_cap",
        true,
        free && !per_player,
        &options,
        &if per_player {
            "0".to_string()
        } else {
            number(&view.settings, "PopulationCap").to_string()
        },
    );
    if let (Some(control), Some(entry)) = (page.controls.get("population_cap"), capacity) {
        control
            .name
            .set_text_content(Some(&titled(&translate(&text(entry, "cap_title")))));
        tip(&control.wrap, &translate(&text(entry, "cap_tooltip")));
    }
    let resources_per_player = flag(offer, "resources_per_player");
    let options = if resources_per_player {
        vec![option("0", tc("starting resources", "Per Player"))]
    } else {
        list(catalog, "starting_resources")
            .iter()
            .map(|entry| {
                option(
                    number(entry, "resources"),
                    tc("startingResources", &text(entry, "title")),
                )
            })
            .collect()
    };
    select(
        "starting_resources",
        true,
        free && !resources_per_player,
        &options,
        &if resources_per_player {
            "0".to_string()
        } else {
            number(&view.settings, "StartingResources").to_string()
        },
    );
    check(
        "disable_spies",
        true,
        free,
        flag(&view.settings, "DisableSpies"),
    );
    check("cheats", true, host, flag(&view.settings, "CheatsEnabled"));

    // Game Type tab
    let active = strings(&view.settings, "VictoryConditions");
    let disabled = strings(offer, "victory_disabled");
    for condition in list(catalog, "victory_conditions") {
        let name = text(&condition, "name");
        check(
            &format!("victory:{name}"),
            true,
            free && !disabled.contains(&name),
            active.contains(&name),
        );
    }
    let relic = flag(offer, "relic");
    let civs = list(catalog, "civs").len() as f64;
    range(
        "relic_count",
        relic,
        free,
        1.0,
        civs,
        number(&view.settings, "RelicCount"),
    );
    range(
        "relic_duration",
        relic,
        free,
        RELIC_DURATION_MIN,
        number(catalog, "relic_duration_max"),
        number(&view.settings, "RelicDuration"),
    );
    check(
        "regicide_garrison",
        flag(offer, "regicide_garrison") && !scenario,
        free,
        flag(&view.settings, "RegicideGarrison"),
    );
    range(
        "wonder_duration",
        flag(offer, "wonder"),
        free,
        RELIC_DURATION_MIN,
        number(catalog, "wonder_duration_max"),
        number(&view.settings, "WonderDuration"),
    );
    let fast = flag(offer, "fast_forward");
    select(
        "game_speed",
        true,
        host,
        &list(catalog, "game_speeds")
            .iter()
            .filter(|speed| fast || !flag(speed, "fast_forward"))
            .map(|speed| option(number(speed, "speed"), translate(&text(speed, "title"))))
            .collect::<Vec<_>>(),
        &number(&view.attributes, "gameSpeed").to_string(),
    );
    range(
        "ceasefire",
        true,
        free,
        0.0,
        number(catalog, "ceasefire_max"),
        number(&view.settings, "Ceasefire"),
    );
    check(
        "locked_teams",
        true,
        host && flag(offer, "locked_teams"),
        flag(&view.settings, "LockTeams"),
    );
    check(
        "last_man_standing",
        true,
        free && flag(offer, "last_man_standing"),
        flag(&view.settings, "LastManStanding"),
    );
}

/// `PlayerName.ReadyTags`: a seat's name in white until its player is
/// ready, green when ready, and blue when they stay ready or nobody holds
/// the seat.
const READY_COLORS: [&str; 3] = ["white", "green", "rgb(150, 150, 250)"];
/// `PlayerColor`'s row background: the player's color at alpha 100.
const ROW_ALPHA: f64 = 100.0 / 255.0;

fn render_players(shared: &Shared, page: &mut Page, view: &View) {
    while page.rows.len() < view.count {
        let row = build_row(shared, page, page.rows.len());
        page.rows.push(row);
    }
    let document = page.dom.document.clone();
    let catalog = &view.catalog;
    let offer = &view.offer;
    let ais = list(catalog, "ais");
    let ai_choices: Vec<(String, String)> = ais
        .iter()
        .filter(|ai| !flag(ai, "hidden"))
        .map(|ai| (text(ai, "id"), translate(&text(ai, "name"))))
        .collect();
    let ai_name = |id: &str| {
        ais.iter()
            .find(|ai| text(ai, "id") == id)
            .map(|ai| translate(&text(ai, "name")))
            .unwrap_or_else(|| id.to_string())
    };
    let ai_label = |id: &str| lang::format(&translate("AI: %(ai)s"), &[("ai", &ai_name(id))]);
    let removed = list(offer, "removed");
    let ai_list = list(offer, "ai");
    let civ_locked = list(offer, "civ_locked");
    let team_locked = list(offer, "team_locked");
    let color_locked = list(offer, "color_locked");
    let colors = list(offer, "colors");
    let max_teams = number(catalog, "max_teams") as usize;
    let host_index = get(&view.state, "host")
        .as_f64()
        .map(|index| index as usize);
    let unassigned = translate("Unassigned");
    show(&page.dom.reset_civs, view.host && !view.scenario);
    show(&page.dom.reset_teams, view.host && !view.scenario);
    for (index, row) in page.rows.iter().enumerate() {
        let used = index < view.count;
        show(&row.line, used);
        if !used {
            continue;
        }
        let data = view.player(index);
        let human = view
            .members
            .iter()
            .find(|member| get(member, "player").as_f64() == Some((index + 1) as f64));
        let ai = ai_list.get(index).filter(|ai| !ai.is_null());
        let is_removed = removed.get(index).and_then(JsValue::as_bool) == Some(true);
        let color = get(&data, "Color");
        let rgb = (
            number(&color, "r"),
            number(&color, "g"),
            number(&color, "b"),
        );
        let _ = row.line.set_attribute(
            "style",
            &format!(
                "background: rgba({},{},{},{ROW_ALPHA:.3})",
                rgb.0, rgb.1, rgb.2
            ),
        );

        // The name, colored by its player's Ready.
        let name = text(&data, "Name");
        // `PlayerName` falls back on `player_defaults.json`'s names, which
        // the server always sets; the number is only a last resort.
        let name = if name.is_empty() {
            (index + 1).to_string()
        } else {
            name
        };
        row.name.set_text_content(Some(&name));
        let ready = human.map_or(READY_COLORS.len() - 1, |human| {
            (number(human, "ready") as usize).min(READY_COLORS.len() - 1)
        });
        let _ = row
            .name
            .set_attribute("style", &format!("color: {}", READY_COLORS[ready]));

        // Color: a swatch, which the host opens to pick another.
        let color_locked = color_locked.get(index).and_then(JsValue::as_bool) == Some(true);
        enable(&row.color, view.host && !color_locked && !view.scenario);
        let options: Vec<(String, String)> = colors
            .iter()
            .map(|color| {
                let channels = Array::from(color);
                let at = |i: u32| channels.get(i).as_f64().unwrap_or(0.0);
                option(format!("{},{},{}", at(0), at(1), at(2)), "\u{a0}")
            })
            .collect();
        fill(
            &document,
            &row.color,
            &options,
            &format!("{},{},{}", rgb.0, rgb.1, rgb.2),
        );
        let _ = row.color.set_attribute(
            "style",
            &format!(
                "background: rgb({},{},{}); box-shadow: inset 0 0 0 1px rgba(0,0,0,0.6)",
                rgb.0, rgb.1, rgb.2
            ),
        );

        // Who holds the slot: the host's list, everyone else's text. A
        // rematch reservation (M35) holds it until its player joins or it
        // expires.
        let reserved_for = if human.is_none() {
            view.reserved
                .iter()
                .find(|hold| number(hold, "slot") as usize == index)
                .map(|hold| text(hold, "name"))
        } else {
            None
        };
        let who_value = if let Some(human) = human {
            format!("member:{}", number(human, "index"))
        } else if reserved_for.is_some() {
            "reserved".to_string()
        } else if is_removed {
            "removed".to_string()
        } else if let Some(ai) = ai {
            format!("ai:{}", text(ai, "bot"))
        } else {
            "unassigned".to_string()
        };
        let mut options = Vec::new();
        if let Some(name) = &reserved_for {
            options.push(option(
                "reserved",
                lang::format(&translate("Reserved for %(name)s"), &[("name", name)]),
            ));
        }
        options.push(option("unassigned", &unassigned));
        options.push(option("removed", translate("Removed")));
        for (id, name) in &ai_choices {
            options.push(option(
                format!("ai:{id}"),
                lang::format(&translate("AI: %(ai)s"), &[("ai", name)]),
            ));
        }
        for member in &view.members {
            if flag(member, "present") {
                options.push(option(
                    format!("member:{}", number(member, "index")),
                    text(member, "name"),
                ));
            }
        }
        // A seated human or an AI outside the list stays shown.
        if !options.iter().any(|(value, _)| *value == who_value) {
            let shown = ai.map(|ai| ai_label(&text(ai, "bot"))).unwrap_or_default();
            options.push(option(&who_value, shown));
        }
        show(&row.who, view.host);
        fill(&document, &row.who, &options, &who_value);
        let who_text = if let Some(human) = human {
            let name = text(human, "name");
            if Some(number(human, "index") as usize) == host_index {
                format!("{name} ({})", translate("Host"))
            } else {
                name
            }
        } else if let Some(name) = &reserved_for {
            lang::format(&translate("Reserved for %(name)s"), &[("name", name)])
        } else if is_removed {
            translate("Removed")
        } else if let Some(ai) = ai {
            ai_label(&text(ai, "bot"))
        } else {
            unassigned.clone()
        };
        let open = human.is_none() && ai.is_none() && !is_removed && reserved_for.is_none();
        let can_take = open && view.you.is_some() && view.seated != Some(index + 1);
        show(&row.who_text, !view.host && !can_take);
        row.who_text.set_text_content(Some(&who_text));
        let _ = row.who_text.set_attribute(
            "style",
            if ai.is_some() {
                "color: rgb(100, 200, 100)"
            } else {
                ""
            },
        );
        show(&row.take, can_take && !view.host);
        let member_index = human.map(|human| number(human, "index") as usize);
        row.member.set(member_index);
        let can_kick = view.host && member_index.is_some_and(|index| Some(index) != host_index);
        show(&row.kick, can_kick);
        show(&row.ban, can_kick);

        // `AIConfigButton`: shown for an AI's slot, which only the host changes.
        show(&row.gear, ai.is_some());
        enable(&row.gear, ai.is_some());
        if let Some(ai) = ai {
            let difficulty = strings(catalog, "ai_difficulties")
                .get(number(ai, "difficulty") as usize)
                .map(|name| tc("aiDiff", &title_case(name)))
                .unwrap_or_default();
            let behavior = tc("aiBehavior", &title_case(&text(ai, "behavior")));
            let description = lang::format(
                &translate("%(AIdifficulty)s %(AIbehavior)s %(AIname)s"),
                &[
                    ("AIdifficulty", &difficulty),
                    ("AIbehavior", &behavior),
                    ("AIname", &ai_name(&text(ai, "bot"))),
                ],
            );
            tip(
                &row.gear,
                &lang::format(
                    &translate("Configure AI: %(description)s."),
                    &[("description", &description)],
                ),
            );
        }

        // Civ and team.
        let civ_enabled = (view.host || view.seated == Some(index + 1))
            && civ_locked.get(index).and_then(JsValue::as_bool) != Some(true);
        let mut options = vec![option("random", tc("civilization", "Random"))];
        for civ in list(catalog, "civs") {
            if flag(&civ, "selectable") {
                let code = text(&civ, "code");
                options.push(option(&code, civ_name(&code)));
            }
        }
        enable(&row.civ, civ_enabled);
        fill(&document, &row.civ, &options, &text(&data, "Civ"));
        let mut options = vec![option(-1, tc("team", "None"))];
        options.extend((0..max_teams).map(|team| option(team, team + 1)));
        enable(
            &row.team,
            view.host
                && !view.scenario
                && team_locked.get(index).and_then(JsValue::as_bool) != Some(true),
        );
        fill(
            &document,
            &row.team,
            &options,
            &get(&data, "Team").as_f64().unwrap_or(-1.0).to_string(),
        );
    }
    render_ai_config(page, view);
}

/// The open AI configuration dialog follows its slot, and closes once the
/// slot has no AI.
fn render_ai_config(page: &mut Page, view: &View) {
    let ai = page.ai_slot.and_then(|index| {
        list(&view.offer, "ai")
            .get(index)
            .filter(|ai| !ai.is_null())
            .cloned()
    });
    let Some(ai) = ai else {
        page.ai_slot = None;
        show(&page.dom.ai_config, false);
        return;
    };
    let dom = &page.dom;
    let catalog = &view.catalog;
    show(&dom.ai_config, true);
    let ais = list(catalog, "ais");
    let mut options = vec![option("", tc("ai", "None"))];
    options.extend(
        ais.iter()
            .filter(|entry| !flag(entry, "hidden"))
            .map(|entry| option(text(entry, "id"), translate(&text(entry, "name")))),
    );
    let bot = text(&ai, "bot");
    fill(&dom.document, &dom.ai_bot, &options, &bot);
    let options: Vec<(String, String)> = strings(catalog, "ai_difficulties")
        .iter()
        .enumerate()
        .map(|(level, name)| option(level, tc("aiDiff", &title_case(name))))
        .collect();
    fill(
        &dom.document,
        &dom.ai_difficulty,
        &options,
        &number(&ai, "difficulty").to_string(),
    );
    let options: Vec<(String, String)> = strings(catalog, "ai_behaviors")
        .iter()
        .map(|name| option(name, tc("aiBehavior", &title_case(name))))
        .collect();
    fill(
        &dom.document,
        &dom.ai_behavior,
        &options,
        &text(&ai, "behavior"),
    );
    for select in [&dom.ai_bot, &dom.ai_difficulty, &dom.ai_behavior] {
        enable(select, view.host);
    }
    let description = ais
        .iter()
        .find(|entry| text(entry, "id") == bot)
        .map(|entry| translate(&text(entry, "description")))
        .unwrap_or_else(|| translate("AI will be disabled for this player."));
    dom.ai_description.set_text_content(Some(&description));
}

fn render_spectators(shared: &Shared, page: &mut Page, view: &View) {
    let document = &page.dom.document;
    let list_element = &page.dom.spectators;
    clear(list_element);
    let host_index = get(&view.state, "host")
        .as_f64()
        .map(|index| index as usize);
    for member in &view.members {
        if get(member, "player").as_f64().is_some() || !flag(member, "present") {
            continue;
        }
        let index = number(member, "index") as usize;
        let item: HtmlElement = create(document, "li");
        if !flag(member, "connected") {
            let _ = item.set_attribute("style", "opacity: 0.6");
        }
        let name: HtmlElement = create(document, "span");
        name.set_text_content(Some(&text(member, "name")));
        let _ = item.append_child(&name);
        if view.host && Some(index) != host_index {
            let (kick, ban) = page.spectator_buttons.entry(index).or_insert_with(|| {
                member_buttons(shared, document, Rc::new(Cell::new(Some(index))))
            });
            let _ = item.append_child(kick);
            let _ = item.append_child(ban);
        }
        if Some(index) == host_index {
            let tag: HtmlElement = create(document, "span");
            tag.set_class_name("host");
            tag.set_text_content(Some(&translate("Host")));
            let _ = item.append_child(&tag);
        }
        if view.host {
            let seat: HtmlSelectElement = create(document, "select");
            let mut options = vec![option("", translate("Seat"))];
            options.extend((0..view.count).map(|slot| option(slot, slot + 1)));
            fill(document, &seat, &options, "");
            let target = seat.clone();
            let shared = Rc::clone(shared);
            listen(&seat, "change", move |_| {
                let Ok(slot) = target.value().parse::<f64>() else {
                    return;
                };
                send(
                    &shared,
                    &object(&[
                        ("type", "move".into()),
                        ("member", (index as f64).into()),
                        ("slot", slot.into()),
                    ]),
                );
            });
            let _ = item.append_child(&seat);
        }
        let _ = list_element.append_child(&item);
    }
}
