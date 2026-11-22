//! The match's chat over `/g/<uuid>/chat` (M31), shared by the setup page and
//! the game page: the socket with reconnects, the entries, the members, and
//! the vanilla line formats of ChatMessageFormatPlayer.js and the gamesetup
//! ClientChat.js.
//!
//! The module owns the socket and the data, not the DOM of either page. The
//! setup page has no frame loop, so it passes a callback that runs after each
//! message; the game page polls with `take_fresh` each frame.

use std::cell::RefCell;
use std::rc::Rc;
use std::rc::Weak;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::Document;
use web_sys::HtmlElement;
use web_sys::MessageEvent;
use web_sys::WebSocket;

use crate::lang;

/// `g_SoundNotifications.nick`'s threshold.
pub const NICK_THRESHOLD_MS: f64 = 3000.0;
/// `chat.timestamp`, on unless turned off.
pub const TIMESTAMP_KEY: &str = "specula.chat.timestamp";
/// `sound.notify.nick`, on unless turned off.
pub const NICK_KEY: &str = "specula.chat.nick";
/// Hide what observers write while the visitor plays (D49), off by default.
pub const HIDE_OBSERVERS_KEY: &str = "specula.chat.hideobservers";
/// `chat.session.extended`, on unless turned off.
pub const EXTENDED_KEY: &str = "specula.chat.extended";
/// The key `play.html` and the setup page keep the nickname under.
const NICKNAME_KEY: &str = "specula.nickname";

/// The pause before reconnecting, doubled after each failure, as the server
/// may be restarting.
const RECONNECT_MS: i32 = 1000;
const RECONNECT_MAX_MS: i32 = 30_000;
/// Entries kept; the server keeps 500 for a backlog and chat is rate
/// limited, so this only bounds a very long match.
pub const KEEP: usize = 2000;

/// Stand-ins for the parts of a line that get their own element, which no
/// translation of the engine's templates contains.
const USER_MARK: char = '\u{1}';
const MESSAGE_MARK: char = '\u{2}';
const TIME_MARK: char = '\u{3}';
const SENDER_MARK: char = '\u{4}';

/// A boolean setting of this module kept in `localStorage`; `default` when
/// it has never been stored or storage is unavailable.
pub fn setting(key: &str, default: bool) -> bool {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(key).ok().flatten())
        .map_or(default, |text| text == "1")
}

pub fn store_setting(key: &str, on: bool) {
    // Best effort: without storage the choice lasts only this visit.
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let _ = storage.set_item(key, if on { "1" } else { "0" });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Everyone,
    Allies,
    Enemies,
    Observers,
    Private,
}

impl Channel {
    fn parse(name: &str) -> Channel {
        match name {
            "allies" => Channel::Allies,
            "enemies" => Channel::Enemies,
            "observers" => Channel::Observers,
            "private" => Channel::Private,
            _ => Channel::Everyone,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Channel::Everyone => "everyone",
            Channel::Allies => "allies",
            Channel::Enemies => "enemies",
            Channel::Observers => "observers",
            Channel::Private => "private",
        }
    }
}

#[derive(Clone)]
pub struct Entry {
    pub seq: u64,
    pub channel: Channel,
    pub from_member: usize,
    pub from_name: String,
    pub from_player: Option<u8>,
    pub from_observer: bool,
    pub text: String,
    pub sent_ms: f64,
    /// Arrived after `live`: shown in the overlay, may ring the nick sound.
    pub live: bool,
}

#[derive(Clone)]
pub struct Member {
    pub member: usize,
    pub name: String,
    pub player: Option<u8>,
    pub observer: bool,
    pub online: bool,
    /// Banned from the match and its chat (D54).
    pub banned: bool,
}

pub struct Chat {
    inner: Rc<RefCell<Inner>>,
}

struct Inner {
    socket: Option<WebSocket>,
    reconnect_ms: i32,
    closed: bool,
    you: Option<usize>,
    /// The host's member index, which only the chat ban needs.
    host: Option<usize>,
    max_length: usize,
    members: Vec<Member>,
    live: bool,
    entries: Vec<Entry>,
    /// New since the page last took them (overlay, sound, rendering).
    fresh: Vec<Entry>,
    error: Option<String>,
    members_changed: bool,
    /// An observer's page joins with the stored nickname once per
    /// connection (D5); the setup page's members join over the setup socket.
    auto_join: bool,
    join_sent: bool,
    on_change: Option<Rc<dyn Fn()>>,
    /// Kept as long as their socket.
    _handlers: Option<Handlers>,
}

struct Handlers {
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut()>,
}

impl Chat {
    pub fn open(auto_join: bool, on_change: Option<Rc<dyn Fn()>>) -> Chat {
        let inner = Rc::new(RefCell::new(Inner {
            socket: None,
            reconnect_ms: RECONNECT_MS,
            closed: false,
            you: None,
            host: None,
            max_length: 1024,
            members: Vec::new(),
            live: false,
            entries: Vec::new(),
            fresh: Vec::new(),
            error: None,
            members_changed: false,
            auto_join,
            join_sent: false,
            on_change,
            _handlers: None,
        }));
        connect(&Rc::downgrade(&inner));
        Chat { inner }
    }

    /// Dropped while the socket is not open: a message is worth sending only
    /// now.
    pub fn send(&self, channel: Channel, to: Option<usize>, text: &str) {
        let message = object(&[
            ("type", "send".into()),
            ("channel", channel.name().into()),
            ("text", text.into()),
        ]);
        if let Some(to) = to {
            let _ = js_sys::Reflect::set(&message, &"to".into(), &(to as f64).into());
        }
        self.transmit(&message);
    }

    pub fn join(&self, nickname: &str) {
        self.transmit(&object(&[
            ("type", "join".into()),
            ("nickname", nickname.into()),
        ]));
    }

    fn transmit(&self, message: &JsValue) {
        let inner = self.inner.borrow();
        let Some(socket) = &inner.socket else {
            return;
        };
        if socket.ready_state() == WebSocket::OPEN
            && let Some(json) = js_sys::JSON::stringify(message)
                .ok()
                .and_then(|json| json.as_string())
        {
            let _ = socket.send_with_str(&json);
        }
    }

    /// The server said `closed`: there is no chat here (a replay, an ended
    /// match).
    pub fn closed(&self) -> bool {
        self.inner.borrow().closed
    }

    pub fn you(&self) -> Option<usize> {
        self.inner.borrow().you
    }

    pub fn host(&self) -> Option<usize> {
        self.inner.borrow().host
    }

    pub fn max_length(&self) -> usize {
        self.inner.borrow().max_length
    }

    pub fn members(&self) -> Vec<Member> {
        self.inner.borrow().members.clone()
    }

    /// The visitor's own name in the match.
    pub fn my_name(&self) -> Option<String> {
        let inner = self.inner.borrow();
        let you = inner.you?;
        inner
            .members
            .iter()
            .find(|member| member.member == you)
            .map(|member| member.name.clone())
    }

    /// Every kept entry, oldest first, for a page that draws them again.
    pub fn entries(&self) -> Vec<Entry> {
        self.inner.borrow().entries.clone()
    }

    pub fn take_fresh(&self) -> Vec<Entry> {
        std::mem::take(&mut self.inner.borrow_mut().fresh)
    }

    pub fn take_members_changed(&self) -> bool {
        std::mem::take(&mut self.inner.borrow_mut().members_changed)
    }

    pub fn take_error(&self) -> Option<String> {
        self.inner.borrow_mut().error.take()
    }
}

fn object(pairs: &[(&str, JsValue)]) -> JsValue {
    let object = js_sys::Object::new();
    for (key, value) in pairs {
        let _ = js_sys::Reflect::set(&object, &(*key).into(), value);
    }
    object.into()
}

fn get(value: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(value, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

fn text(value: &JsValue, key: &str) -> String {
    get(value, key).as_string().unwrap_or_default()
}

fn number(value: &JsValue, key: &str) -> Option<f64> {
    get(value, key).as_f64()
}

fn connect(inner: &Weak<RefCell<Inner>>) {
    let Some(strong) = inner.upgrade() else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let location = window.location();
    let scheme = match location.protocol().as_deref() {
        Ok("https:") => "wss",
        _ => "ws",
    };
    let url = format!(
        "{scheme}://{}{}/chat",
        location.host().unwrap_or_default(),
        crate::match_base()
    );
    let Ok(socket) = WebSocket::new(&url) else {
        web_sys::console::error_1(&format!("cannot open {url}").into());
        return;
    };
    let weak = inner.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Some(body) = event.data().as_string() else {
            return;
        };
        let Ok(message) = js_sys::JSON::parse(&body) else {
            return;
        };
        let Some(strong) = weak.upgrade() else {
            return;
        };
        let join = handle(&mut strong.borrow_mut(), &message);
        if let Some(name) = join {
            Chat {
                inner: Rc::clone(&strong),
            }
            .join(&name);
        }
        let on_change = strong.borrow().on_change.clone();
        if let Some(on_change) = on_change {
            on_change();
        }
    });
    let weak = inner.clone();
    let on_close = Closure::<dyn FnMut()>::new(move || {
        let Some(strong) = weak.upgrade() else {
            return;
        };
        let wait = {
            let mut inner = strong.borrow_mut();
            inner.socket = None;
            if inner.closed {
                return;
            }
            let wait = inner.reconnect_ms;
            inner.reconnect_ms = (wait * 2).min(RECONNECT_MAX_MS);
            wait
        };
        let again = weak.clone();
        let retry = Closure::once_into_js(move || connect(&again));
        if let Some(window) = web_sys::window() {
            let _ = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(retry.unchecked_ref(), wait);
        }
    });
    let weak = inner.clone();
    let on_open = Closure::once_into_js(move || {
        if let Some(strong) = weak.upgrade() {
            strong.borrow_mut().reconnect_ms = RECONNECT_MS;
        }
    });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    socket.set_onopen(Some(on_open.unchecked_ref()));
    let mut inner = strong.borrow_mut();
    // The server resends the backlog on a new connection: the entries are
    // deduplicated by `seq`, and the overlay waits for the next `live`.
    inner.live = false;
    inner.join_sent = false;
    inner.socket = Some(socket);
    inner._handlers = Some(Handlers {
        _on_message: on_message,
        _on_close: on_close,
    });
}

/// Applies one server message; answers the nickname to join with, if this
/// is the moment.
fn handle(inner: &mut Inner, message: &JsValue) -> Option<String> {
    match text(message, "type").as_str() {
        "you" => {
            inner.you = number(message, "member").map(|member| member as usize);
            if let Some(length) = number(message, "max_length") {
                inner.max_length = length as usize;
            }
        }
        "members" => {
            inner.you = number(message, "you").map(|member| member as usize);
            inner.host = number(message, "host").map(|member| member as usize);
            let list = get(message, "members");
            let members: Vec<Member> = if js_sys::Array::is_array(&list) {
                list.unchecked_into::<js_sys::Array>()
                    .iter()
                    .map(|member| Member {
                        member: number(&member, "member").unwrap_or(0.0) as usize,
                        name: text(&member, "name"),
                        player: number(&member, "player").map(|player| player as u8),
                        observer: get(&member, "observer").as_bool().unwrap_or(false),
                        online: get(&member, "online").as_bool().unwrap_or(false),
                        banned: get(&member, "banned").as_bool().unwrap_or(false),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let changed = members.len() != inner.members.len()
                || members.iter().zip(&inner.members).any(|(new, old)| {
                    new.member != old.member
                        || new.name != old.name
                        || new.player != old.player
                        || new.observer != old.observer
                        || new.online != old.online
                        || new.banned != old.banned
                });
            if changed {
                inner.members = members;
                inner.members_changed = true;
            }
            if inner.you.is_none() && inner.auto_join && !inner.join_sent {
                let name = web_sys::window()
                    .and_then(|window| window.local_storage().ok().flatten())
                    .and_then(|storage| storage.get_item(NICKNAME_KEY).ok().flatten())
                    .filter(|name| !name.is_empty());
                if name.is_some() {
                    inner.join_sent = true;
                }
                return name;
            }
        }
        "live" => inner.live = true,
        "chat" => {
            let seq = number(message, "seq").unwrap_or(0.0) as u64;
            if inner.entries.iter().any(|entry| entry.seq == seq) {
                return None;
            }
            let from = get(message, "from");
            let entry = Entry {
                seq,
                channel: Channel::parse(&text(message, "channel")),
                from_member: number(&from, "member").unwrap_or(0.0) as usize,
                from_name: text(&from, "name"),
                from_player: number(&from, "player").map(|player| player as u8),
                from_observer: get(&from, "observer").as_bool().unwrap_or(false),
                text: text(message, "text"),
                sent_ms: number(message, "sent_ms").unwrap_or(0.0),
                live: inner.live,
            };
            inner.fresh.push(entry.clone());
            inner.entries.push(entry);
            if inner.entries.len() > KEEP {
                inner.entries.remove(0);
            }
        }
        "error" => inner.error = Some(text(message, "text")),
        "closed" => inner.closed = true,
        _ => {}
    }
    None
}

/// What a line shows, before the page turns it into elements.
pub struct Line {
    /// The `[HH:mm]` stamp, if the setting is on.
    pub time: Option<String>,
    pub context: Option<String>,
    pub me: bool,
    pub name: String,
    pub text: String,
    /// The gamesetup format: a bold `<name>`, no context and no `/me`.
    pub setup: bool,
}

/// The line of `ChatMessageFormatPlayer`: `/me` and the channel's context.
/// None for what vanilla shows nothing for (an empty `/me`).
pub fn game_line(entry: &Entry, timestamps: bool) -> Option<Line> {
    let (me, message) = match entry.text.strip_prefix("/me ") {
        Some(rest) => (true, rest),
        None => (false, entry.text.as_str()),
    };
    if message.is_empty() {
        return None;
    }
    let context = match entry.channel {
        Channel::Everyone => None,
        Channel::Allies => Some(lang::translate_context("chat message context", "Ally")),
        Channel::Enemies => Some(lang::translate_context("chat message context", "Enemy")),
        Channel::Observers => Some(lang::translate_context("chat message context", "Observer")),
        Channel::Private => Some(lang::translate_context("chat message context", "Private")),
    };
    Some(Line {
        time: timestamps.then(|| stamp(entry.sent_ms)),
        context,
        me,
        name: entry.from_name.clone(),
        text: message.to_string(),
        setup: false,
    })
}

/// The gamesetup `ClientChat` line.
pub fn setup_line(entry: &Entry, timestamps: bool) -> Line {
    Line {
        time: timestamps.then(|| stamp(entry.sent_ms)),
        context: None,
        me: false,
        name: entry.from_name.clone(),
        text: entry.text.clone(),
        setup: true,
    }
}

/// Local `HH:mm`; the engine's `translate("HH:mm")` is an ICU pattern, which
/// a browser has no use for.
pub fn time_of(sent_ms: f64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(sent_ms));
    format!("{:02}:{:02}", date.get_hours(), date.get_minutes())
}

/// `TimestampFormat`; the backslash is the engine GUI's markup escape.
fn stamp(sent_ms: f64) -> String {
    lang::format(
        &lang::translate("\\[%(time)s]"),
        &[("time", &time_of(sent_ms))],
    )
    .replace("\\[", "[")
}

/// True if `entry` names the visitor, as `ChatMessageFormatPlayer`'s nick
/// test (`splitRatingFromNick` is moot: there are no ratings).
pub fn is_nick_mention(entry: &Entry, you: Option<usize>, my_name: &str) -> bool {
    !my_name.is_empty()
        && Some(entry.from_member) != you
        && entry.text.to_lowercase().contains(&my_name.to_lowercase())
}

/// A line as a `<div>` of text and spans, so the name can have its colour and
/// no user text is ever parsed as markup (vanilla's `escapeText`).
pub fn render(document: &Document, line: &Line, color: Option<String>) -> HtmlElement {
    let root: HtmlElement = element(document, "div");
    let mut shown = if line.setup {
        lang::format(
            &lang::translate("%(username)s %(message)s"),
            &[
                ("username", &SENDER_MARK.to_string()),
                ("message", &MESSAGE_MARK.to_string()),
            ],
        )
    } else {
        let user = USER_MARK.to_string();
        let message = MESSAGE_MARK.to_string();
        let tag = lang::format(&lang::translate("<%(user)s>"), &[("user", &user)]);
        let template = match (line.me, line.context.is_some()) {
            (false, true) => "(%(context)s) %(userTag)s %(message)s",
            (false, false) => "%(userTag)s %(message)s",
            (true, true) => "(%(context)s) * %(user)s %(message)s",
            (true, false) => "* %(user)s %(message)s",
        };
        lang::format(
            &lang::translate(template),
            &[
                ("context", line.context.as_deref().unwrap_or("")),
                ("userTag", &tag),
                ("user", &user),
                ("message", &message),
            ],
        )
    };
    if line.time.is_some() {
        shown = lang::format(
            &lang::translate("%(time)s %(message)s"),
            &[("time", &TIME_MARK.to_string()), ("message", &shown)],
        );
    }
    let name_span = |color: &Option<String>| {
        let span: HtmlElement = element(document, "span");
        span.set_text_content(Some(&line.name));
        if let Some(color) = color {
            let _ = span.style().set_property("color", color);
        }
        span
    };
    // Text goes in spans: a text node needs a web-sys feature the crate
    // does not enable.
    let text_span = |text: &str| {
        let span: HtmlElement = element(document, "span");
        span.set_text_content(Some(text));
        span
    };
    let mut pending = String::new();
    let flush = |pending: &mut String| {
        if !pending.is_empty() {
            let _ = root.append_child(&text_span(pending));
            pending.clear();
        }
    };
    for c in shown.chars() {
        match c {
            USER_MARK => {
                flush(&mut pending);
                let _ = root.append_child(&name_span(&color));
            }
            SENDER_MARK => {
                flush(&mut pending);
                // `SenderFormat` in the bold font, the name inside it.
                let bold: HtmlElement = element(document, "span");
                let _ = bold.style().set_property("font-weight", "700");
                let tag = lang::format(
                    &lang::translate("<%(username)s>"),
                    &[("username", &USER_MARK.to_string())],
                );
                let mut part = String::new();
                for c in tag.chars() {
                    if c == USER_MARK {
                        if !part.is_empty() {
                            let _ = bold.append_child(&text_span(&part));
                            part.clear();
                        }
                        let _ = bold.append_child(&name_span(&color));
                    } else {
                        part.push(c);
                    }
                }
                if !part.is_empty() {
                    let _ = bold.append_child(&text_span(&part));
                }
                let _ = root.append_child(&bold);
            }
            MESSAGE_MARK => {
                flush(&mut pending);
                let _ = root.append_child(&text_span(&line.text));
            }
            TIME_MARK => {
                flush(&mut pending);
                let time = line.time.as_deref().unwrap_or("");
                let _ = root.append_child(&text_span(time));
            }
            c => pending.push(c),
        }
    }
    flush(&mut pending);
    root
}

fn element<T: JsCast>(document: &Document, tag: &str) -> T {
    document
        .create_element(tag)
        .expect("a known tag")
        .unchecked_into()
}

/// `gui/common/color.js`'s `rgbToHsl`, in 0..=1 parts.
fn rgb_to_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

/// `hslToRgb`, in 0..=255.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    if s == 0.0 {
        let v = l * 255.0;
        return (v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        v * 255.0
    };
    (channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
}

/// The setup chat's name colour: the player's, lightened to `l >= 0.6` so it
/// reads on the dark box (`ClientChat.colorizePlayernameByGUID`).
pub fn lightened(r: f64, g: f64, b: f64) -> String {
    let (h, s, l) = rgb_to_hsl(r, g, b);
    let (r, g, b) = hsl_to_rgb(h, s, l.max(0.6));
    format!("rgb({}, {}, {})", r.round(), g.round(), b.round())
}
