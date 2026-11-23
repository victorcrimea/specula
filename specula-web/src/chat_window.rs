//! The game page's chat, a port of gui/session/chat/ (`ChatWindow`,
//! `ChatOverlay`, `ChatHistory`, `ChatAddressees`) onto the DOM: the dialog
//! of `chat_window.xml`, and the last lines on the screen for a while.
//!
//! What differs from the engine: the addressee list is the server's members
//! (`chat::Chat::members`), the lines carry the server's send time, and of
//! the engine's notifications the lines are those of `notices.rs`, live
//! only (CONTROL-IMPL.md V95, V105).

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::Document;
use web_sys::HtmlButtonElement;
use web_sys::HtmlElement;
use web_sys::HtmlInputElement;
use web_sys::HtmlOptionElement;
use web_sys::HtmlSelectElement;

use crate::chat;
use crate::chat::Channel;
use crate::chat::Entry;
use crate::lang;
use crate::notices::Filter;
use crate::notices::Notice;
use crate::notices::Piece;
use crate::panel::create;
use crate::panel::css_color;

/// `g_SoundNotifications.nick`, as the synthetic group `bake-assets` makes
/// for the bare file.
pub const NICK_SOUND: &str = "interface/ui/chat_alert.ogg";
/// `ChatOverlay.chatLinesNumber`.
const OVERLAY_LINES: usize = 20;
/// `ChatOverlay.chatTimeout`.
const OVERLAY_MS: f64 = 30_000.0;
/// `ChatHistory.Filters` that the server has messages for, in their order.
const FILTERS: [(&str, &str); 9] = [
    ("all", "Chat and notifications"),
    ("chat", "Chat messages"),
    ("player", "Players chat"),
    ("ally", "Ally chat"),
    ("enemy", "Enemy chat"),
    ("observer", "Observer chat"),
    ("private", "Private chat"),
    ("gamenotifications", "Game notifications"),
    ("chatnotifications", "Network notifications"),
];

/// Where `ChatMessageFormatSimulation.attack`'s line and the
/// camera.lastattackfocus hotkey take the camera (`focusAttack`).
#[derive(Clone, Copy)]
pub struct Attack {
    pub target: u32,
    /// The target is a unit, so it is followed rather than looked at.
    pub unit: bool,
    pub x: f32,
    pub z: f32,
}

/// The attack line's attribute that holds its `Attack`, read on a click.
const FOCUS_ATTRIBUTE: &str = "data-focus";
/// Stand-ins for the parts of a translated line that are not text.
const ICON_MARK: char = '\u{E000}';
const ATTACKER_MARK: char = '\u{E001}';
const TEXT_MARK: char = '\u{E002}';

/// What the page wires listeners to.
pub struct Controls {
    pub input: HtmlInputElement,
    pub send: HtmlButtonElement,
    pub cancel: HtmlButtonElement,
    pub extended: HtmlInputElement,
    pub filter: HtmlSelectElement,
    pub to: HtmlSelectElement,
    pub ban: HtmlButtonElement,
}

enum Item {
    Message {
        entry: Entry,
        /// Decided on arrival, as the engine does "before diplomacies
        /// change": a seated sender who was not an observer then.
        player: bool,
    },
    /// An error of the server, as the engine's plain system lines.
    System(String),
    /// `ChatMessageFormatSimulation.attack`, its text fixed on arrival as
    /// the engine's history keeps it.
    Attack {
        domestic: bool,
        attacker: String,
        color: String,
        received_ms: f64,
    },
    /// A line of `notices.rs`, as the engine's history keeps it.
    Notice { notice: Notice, received_ms: f64 },
}

/// `#chat-window` and `#chat-overlay`.
pub struct ChatWindow {
    chat: chat::Chat,
    document: Document,
    panel: HtmlElement,
    history_page: HtmlElement,
    history: HtmlElement,
    filter: HtmlSelectElement,
    to: HtmlSelectElement,
    ban: HtmlButtonElement,
    input: HtmlInputElement,
    send: HtmlButtonElement,
    cancel: HtmlButtonElement,
    extended: HtmlInputElement,
    overlay: HtmlElement,
    captions: Captions,
    /// (expires at page ms, line element), oldest first; at most
    /// `OVERLAY_LINES`.
    overlay_lines: VecDeque<(f64, HtmlElement)>,
    items: Vec<Item>,
    /// The last private partner, for L (vanilla rebinds privatechat on
    /// send).
    last_private: Option<usize>,
    last_nick_ms: f64,
    /// The captions and the filter list were written in the language
    /// loaded then; false to write them again.
    labelled: bool,
    members_dirty: bool,
    history_dirty: bool,
    /// What the history was last drawn for.
    drawn: String,
    can_send: Option<bool>,
    /// An attack line clicked since the last `take_focus`.
    focus: Rc<Cell<Option<Attack>>>,
}

struct Captions {
    filter: HtmlElement,
    to: HtmlElement,
    text: HtmlElement,
    extended: HtmlElement,
}

impl ChatWindow {
    pub fn new(document: &Document) -> Result<Self, String> {
        let find = |id: &str| crate::element::<HtmlElement>(document, id);
        let window = ChatWindow {
            chat: chat::Chat::open(true, None),
            document: document.clone(),
            panel: find("chat-window")?,
            history_page: find("chat-history-page")?,
            history: find("chat-history")?,
            filter: crate::element(document, "chat-filter")?,
            to: crate::element(document, "chat-to")?,
            ban: crate::element(document, "chat-ban")?,
            input: crate::element(document, "chat-input")?,
            send: crate::element(document, "chat-send")?,
            cancel: crate::element(document, "chat-cancel")?,
            extended: crate::element(document, "chat-extended")?,
            overlay: find("chat-overlay")?,
            captions: Captions {
                filter: find("chat-filter-caption")?,
                to: find("chat-to-caption")?,
                text: find("chat-text-caption")?,
                extended: find("chat-extended-text")?,
            },
            overlay_lines: VecDeque::new(),
            items: Vec::new(),
            last_private: None,
            last_nick_ms: f64::NEG_INFINITY,
            labelled: false,
            members_dirty: true,
            history_dirty: true,
            drawn: String::new(),
            can_send: None,
            focus: Rc::new(Cell::new(None)),
        };
        window.wire_focus();
        window.panel.set_hidden(true);
        window
            .extended
            .set_checked(chat::setting(chat::EXTENDED_KEY, true));
        window.apply_extended();
        Ok(window)
    }

    /// One listener on the overlay for every attack line, as the lines come
    /// and go.
    fn wire_focus(&self) {
        let focus = Rc::clone(&self.focus);
        let on_click = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            let attack = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                .and_then(|element| {
                    element
                        .closest(&format!("[{FOCUS_ATTRIBUTE}]"))
                        .ok()
                        .flatten()
                })
                .and_then(|line| line.get_attribute(FOCUS_ATTRIBUTE))
                .and_then(|value| parse_attack(&value));
            if attack.is_some() {
                focus.set(attack);
            }
        });
        let _ = self
            .overlay
            .add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref());
        // The overlay lives as long as the page, so its handler does too.
        on_click.forget();
    }

    /// The attack line clicked since the last call.
    pub fn take_focus(&self) -> Option<Attack> {
        self.focus.take()
    }

    /// `ChatMessageFormatSimulation.attack`: a line in the overlay that
    /// focuses the attack when clicked, and one in the history.
    pub fn push_attack(
        &mut self,
        now: f64,
        domestic: bool,
        attacker: &str,
        color: &str,
        attack: Attack,
    ) {
        let attacker = if attacker.is_empty() {
            lang::translate("Unknown Player")
        } else {
            attacker.to_string()
        };
        let received_ms = js_sys::Date::now();
        let line = self.attack_line(domestic, &attacker, color, None);
        let _ = line.set_attribute(
            FOCUS_ATTRIBUTE,
            &format!(
                "{} {} {} {}",
                attack.target,
                u8::from(attack.unit),
                attack.x,
                attack.z
            ),
        );
        let _ = line.set_attribute(
            crate::tooltip::ATTRIBUTE,
            &lang::translate("Click to focus on the attacked unit."),
        );
        let _ = line.set_attribute("class", "focus");
        self.push_overlay(now, line);
        self.items.push(Item::Attack {
            domestic,
            attacker,
            color: color.to_string(),
            received_ms,
        });
        self.trim_items();
        self.history_dirty = true;
    }

    /// A notification line (`notices.rs`) in the overlay and the history; a
    /// focus place makes a click on the overlay line move the camera.
    pub fn push_notice(&mut self, now: f64, notice: Notice) {
        let line = self.notice_line(&notice, None);
        if let Some(focus) = notice.focus {
            let _ = line.set_attribute(
                FOCUS_ATTRIBUTE,
                &format!(
                    "{} {} {} {}",
                    focus.target,
                    u8::from(focus.unit),
                    focus.x,
                    focus.z
                ),
            );
            let _ = line.set_attribute(
                crate::tooltip::ATTRIBUTE,
                &lang::translate("Click to focus on the flare's location."),
            );
            let _ = line.set_attribute("class", "focus");
        }
        self.push_overlay(now, line);
        self.items.push(Item::Notice {
            notice,
            received_ms: js_sys::Date::now(),
        });
        self.trim_items();
        self.history_dirty = true;
    }

    /// Whether a line is showing in the overlay, which a closed chat hides
    /// otherwise.
    fn overlay_busy(&self) -> bool {
        !self.overlay_lines.is_empty()
    }

    fn notice_line(&self, notice: &Notice, time: Option<f64>) -> HtmlElement {
        let root: HtmlElement = create(&self.document, "div").expect("a div");
        let span = |text: &str| {
            let span: HtmlElement = create(&self.document, "span").expect("a span");
            span.set_text_content(Some(text));
            span
        };
        let mut suffix = String::new();
        if let Some(time) = time {
            // The engine's one template for the stamp, with the line itself
            // standing in as a mark so that its pieces stay pieces.
            let stamped = lang::format(
                &lang::translate("%(time)s %(message)s"),
                &[
                    ("time", &chat::stamp(time)),
                    ("message", &TEXT_MARK.to_string()),
                ],
            );
            let (before, after) = stamped.split_once(TEXT_MARK).unwrap_or((&stamped, ""));
            if !before.is_empty() {
                let _ = root.append_child(&span(before));
            }
            suffix = after.to_string();
        }
        for piece in &notice.pieces {
            match piece {
                Piece::Text(text) => {
                    let _ = root.append_child(&span(text));
                }
                Piece::Name { text, color } => {
                    let name = span(text);
                    let _ = name.style().set_property("color", color);
                    let _ = root.append_child(&name);
                }
                Piece::Icon(variable) => {
                    let icon = span("");
                    let _ = icon.set_attribute("class", "icon");
                    let _ = icon
                        .style()
                        .set_property("background-image", &format!("var({variable})"));
                    let _ = root.append_child(&icon);
                }
            }
        }
        if !suffix.is_empty() {
            let _ = root.append_child(&span(&suffix));
        }
        root
    }

    /// The attack text with its icon and the attacker's coloured name, after
    /// the history's timestamp when one is given.
    fn attack_line(
        &self,
        domestic: bool,
        attacker: &str,
        color: &str,
        time: Option<f64>,
    ) -> HtmlElement {
        let template = if domestic {
            lang::translate("%(icon)sYour livestock has been attacked by %(attacker)s!")
        } else {
            lang::translate("%(icon)sYou have been attacked by %(attacker)s!")
        };
        let mut shown = lang::format(
            &template,
            &[
                ("icon", &ICON_MARK.to_string()),
                ("attacker", &ATTACKER_MARK.to_string()),
            ],
        );
        if let Some(time) = time {
            shown = lang::format(
                &lang::translate("%(time)s %(message)s"),
                &[("time", &chat::stamp(time)), ("message", &shown)],
            );
        }
        let root: HtmlElement = create(&self.document, "div").expect("a div");
        let span = |text: &str| {
            let span: HtmlElement = create(&self.document, "span").expect("a span");
            span.set_text_content(Some(text));
            span
        };
        let mut pending = String::new();
        for c in shown.chars() {
            if c != ICON_MARK && c != ATTACKER_MARK {
                pending.push(c);
                continue;
            }
            if !pending.is_empty() {
                let _ = root.append_child(&span(&pending));
                pending.clear();
            }
            if c == ICON_MARK {
                let icon = span("");
                let _ = icon.set_attribute("class", "icon");
                let _ = icon
                    .style()
                    .set_property("background-image", "var(--gui-icon-focusattacked)");
                let _ = root.append_child(&icon);
            } else {
                let name = span(attacker);
                let _ = name.style().set_property("color", color);
                let _ = root.append_child(&name);
            }
        }
        if !pending.is_empty() {
            let _ = root.append_child(&span(&pending));
        }
        root
    }

    pub fn controls(&self) -> Controls {
        Controls {
            input: self.input.clone(),
            send: self.send.clone(),
            cancel: self.cancel.clone(),
            extended: self.extended.clone(),
            filter: self.filter.clone(),
            to: self.to.clone(),
            ban: self.ban.clone(),
        }
    }

    /// The observer member the host's Ban may act on: the chosen addressee,
    /// when it is an observer who is not banned yet.
    pub fn ban_target(&self) -> Option<(usize, String)> {
        let you = self.chat.you()?;
        if self.chat.host() != Some(you) {
            return None;
        }
        let member = self
            .to
            .value()
            .strip_prefix("member:")
            .and_then(|member| member.parse::<usize>().ok())?;
        let entry = self
            .chat
            .members()
            .into_iter()
            .find(|entry| entry.member == member)?;
        (entry.observer && entry.player.is_none() && !entry.banned).then_some((member, entry.name))
    }

    /// The chat is there to read and write: false on a replay or an ended
    /// match, whose socket answered `closed`.
    pub fn available(&self) -> bool {
        !self.chat.closed()
    }

    /// Whether this visitor is an observer by the server's test. Before the
    /// first members list the page's own seat decides, so T on a seated
    /// page opens Allies from the start.
    pub fn is_observer(&self) -> bool {
        let Some(you) = self.chat.you() else {
            return !crate::seated();
        };
        self.chat
            .members()
            .iter()
            .find(|member| member.member == you)
            .is_none_or(|member| member.observer)
    }

    pub fn is_open(&self) -> bool {
        !self.panel.hidden()
    }

    pub fn last_private(&self) -> Option<usize> {
        self.last_private
    }

    /// The language changed, so the captions and lists are written again.
    pub fn invalidate(&mut self) {
        self.labelled = false;
        self.members_dirty = true;
        self.history_dirty = true;
    }

    /// `Chat.openPage`: the addressee chosen, the history drawn, the input
    /// focused.
    pub fn open(&mut self, channel: Channel, to: Option<usize>, palette: &[u32]) {
        if !self.available() {
            return;
        }
        self.write_labels();
        let wanted = match (channel, to) {
            (Channel::Private, Some(member)) => format!("member:{member}"),
            (Channel::Allies, _) => "allies".to_string(),
            (Channel::Enemies, _) => "enemies".to_string(),
            (Channel::Observers, _) => "observers".to_string(),
            _ => "everyone".to_string(),
        };
        self.rebuild_addressees(palette, Some(wanted));
        self.members_dirty = false;
        self.history_dirty = true;
        self.panel.set_hidden(false);
        let _ = self.input.focus();
    }

    /// `closePage`: the text is dropped, not kept for the next time.
    pub fn close(&mut self) {
        if !self.is_open() {
            return;
        }
        self.input.set_value("");
        let _ = self.input.blur();
        self.panel.set_hidden(true);
    }

    /// The addressee or the filter changed: back to typing.
    pub fn refocus(&self) {
        if self.is_open() {
            let _ = self.input.focus();
        }
    }

    pub fn filter_changed(&mut self) {
        self.history_dirty = true;
        self.refocus();
    }

    /// The History checkbox: kept across visits, the window resized.
    pub fn extended_changed(&mut self) {
        chat::store_setting(chat::EXTENDED_KEY, self.extended.checked());
        self.apply_extended();
        self.history_dirty = true;
        self.refocus();
    }

    fn apply_extended(&self) {
        let on = self.extended.checked();
        self.history_page.set_hidden(!on);
        let _ = self
            .panel
            .set_attribute("data-extended", if on { "1" } else { "0" });
    }

    /// Sends the text to the chosen addressee, then closes, as the engine's
    /// chatSubmittedHandlers do.
    pub fn submit(&mut self) {
        let text = self.input.value();
        if !text.is_empty() && self.can_write() {
            let value = self.to.value();
            let (channel, to) = match value.as_str() {
                "allies" => (Channel::Allies, None),
                "enemies" => (Channel::Enemies, None),
                "observers" => (Channel::Observers, None),
                other => match other
                    .strip_prefix("member:")
                    .and_then(|member| member.parse::<usize>().ok())
                {
                    Some(member) => (Channel::Private, Some(member)),
                    None => (Channel::Everyone, None),
                },
            };
            self.chat.send(channel, to, &text);
            if channel == Channel::Private {
                self.last_private = to;
            }
        }
        self.close();
    }

    fn can_write(&self) -> bool {
        self.chat.you().is_some() && !self.chat.closed()
    }

    /// Each frame: fresh entries into history and overlay, expired overlay
    /// lines out, the addressee list when members changed. True to play the
    /// nick sound.
    pub fn frame(
        &mut self,
        now: f64,
        palette: &[u32],
        seated: bool,
        hide_observers: bool,
        timestamps: bool,
    ) -> bool {
        let mut ring = false;
        // The broadcast's chat is closed, yet its notification lines show.
        self.overlay
            .set_hidden(self.chat.closed() && !self.overlay_busy());
        while let Some((expires, line)) = self.overlay_lines.front() {
            if *expires > now {
                break;
            }
            line.remove();
            self.overlay_lines.pop_front();
        }
        if !self.labelled {
            self.write_labels();
        }
        let host = self.chat.you().is_some() && self.chat.host() == self.chat.you();
        self.ban.set_hidden(!host);
        self.ban.set_disabled(self.ban_target().is_none());
        let can_send = self.can_write();
        if self.can_send != Some(can_send) {
            self.can_send = Some(can_send);
            self.input.set_disabled(!can_send);
            self.send.set_disabled(!can_send);
            if can_send {
                self.input.set_max_length(self.chat.max_length() as i32);
            }
        }
        if self.chat.take_members_changed() {
            self.members_dirty = true;
        }
        if self.members_dirty && self.is_open() {
            self.members_dirty = false;
            self.rebuild_addressees(palette, None);
        }

        let you = self.chat.you();
        let my_name = self.chat.my_name().unwrap_or_default();
        let hidden = |entry: &Entry| {
            seated && hide_observers && entry.from_observer && Some(entry.from_member) != you
        };
        for entry in self.chat.take_fresh() {
            let player = entry.from_player.is_some() && !entry.from_observer;
            let live = entry.live;
            let shown = !hidden(&entry);
            let mention = live && shown && chat::is_nick_mention(&entry, you, &my_name);
            if shown
                && live
                && let Some(line) = chat::game_line(&entry, timestamps)
            {
                let color = name_color(palette, entry.from_player);
                let element = chat::render(&self.document, &line, color);
                self.push_overlay(now, element);
            }
            self.items.push(Item::Message { entry, player });
            self.trim_items();
            self.history_dirty = true;
            if mention && now - self.last_nick_ms > chat::NICK_THRESHOLD_MS {
                self.last_nick_ms = now;
                ring = true;
            }
        }
        if let Some(error) = self.chat.take_error() {
            let text = lang::translate(&error);
            let line = self.system_line(&text);
            self.push_overlay(now, line);
            self.items.push(Item::System(text));
            self.trim_items();
            self.history_dirty = true;
        }

        let key = format!("{timestamps}{}", seated && hide_observers);
        if key != self.drawn {
            self.drawn = key;
            self.history_dirty = true;
        }
        if self.history_dirty && self.is_open() && self.extended.checked() {
            self.history_dirty = false;
            self.draw_history(palette, seated && hide_observers, timestamps);
        }
        ring
    }

    /// The history keeps as many lines as the chat keeps entries.
    fn trim_items(&mut self) {
        if self.items.len() > chat::KEEP {
            let excess = self.items.len() - chat::KEEP;
            self.items.drain(..excess);
        }
    }

    fn system_line(&self, text: &str) -> HtmlElement {
        let line: HtmlElement = create(&self.document, "div").expect("a div");
        line.set_text_content(Some(text));
        line
    }

    fn push_overlay(&mut self, now: f64, line: HtmlElement) {
        let _ = self.overlay.append_child(&line);
        self.overlay_lines.push_back((now + OVERLAY_MS, line));
        while self.overlay_lines.len() > OVERLAY_LINES
            && let Some((_, oldest)) = self.overlay_lines.pop_front()
        {
            oldest.remove();
        }
    }

    /// The captions and the filter list, in the loaded language. The
    /// engine's strings have contexts, which `data-i18n` cannot name, so
    /// they are written here and again on a language change.
    fn write_labels(&mut self) {
        self.labelled = true;
        let context = |element: &HtmlElement, context: &str, text: &str| {
            element.set_text_content(Some(&lang::translate_context(context, text)));
        };
        context(&self.captions.filter, "chat input", "Filter:");
        context(&self.captions.to, "chat input", "To:");
        context(&self.captions.text, "chat input", "Text:");
        context(&self.captions.extended, "chat", "History");
        self.cancel
            .set_text_content(Some(&lang::translate("Cancel")));
        self.send.set_text_content(Some(&lang::translate("Send")));
        let tip = |element: &HtmlElement, text: &str| {
            element.set_title(&lang::translate_context("chat input", text));
        };
        tip(&self.to, "Select chat addressee.");
        tip(&self.filter, "Filter the chat history.");
        tip(&self.input, "Type the message to send.");

        let selected = self.filter.selected_index().max(0);
        self.filter.set_text_content(None);
        for (key, label) in FILTERS {
            if let Some(option) =
                self.option(key, &lang::translate_context("chat history filter", label))
            {
                let _ = self.filter.append_child(&option);
            }
        }
        self.filter.set_selected_index(selected);
    }

    fn option(&self, value: &str, label: &str) -> Option<HtmlOptionElement> {
        let option: HtmlOptionElement = create(&self.document, "option").ok()?;
        option.set_value(value);
        option.set_text_content(Some(label));
        Some(option)
    }

    /// `ChatAddressees.onUpdatePlayers`.
    fn rebuild_addressees(&mut self, palette: &[u32], wanted: Option<String>) {
        let previous = wanted.unwrap_or_else(|| self.to.value());
        let you = self.chat.you();
        let observer = self.is_observer();
        let label = |text: &str| lang::translate_context("chat addressee", text);
        let mut list: Vec<(String, String, Option<String>)> =
            vec![("everyone".to_string(), label("Everyone"), None)];
        if !observer {
            list.push(("allies".to_string(), label("Allies"), None));
            list.push(("enemies".to_string(), label("Enemies"), None));
        }
        list.push(("observers".to_string(), label("Observers"), None));

        // `sortGUIDsByPlayerID`: players by number, observers last.
        let members = self.chat.members();
        let mut others: Vec<&chat::Member> = members
            .iter()
            .filter(|member| Some(member.member) != you && member.online)
            // D48: an observer's message to a player is refused.
            .filter(|member| !observer || member.observer)
            .collect();
        others.sort_by_key(|member| match (member.observer, member.player) {
            (false, Some(player)) => (0, player),
            _ => (1, 0),
        });
        for member in others {
            // An `<option>` holds no markup, so the whole entry takes the
            // colour rather than the engine's box alone (V94).
            let (text, color) = match (member.observer, member.player) {
                (false, Some(player)) => (
                    format!("\u{25A0} {}", member.name),
                    name_color(palette, Some(player)),
                ),
                _ => (member.name.clone(), None),
            };
            list.push((format!("member:{}", member.member), text, color));
        }
        // The partner went offline: the engine keeps them as a mock entry.
        if let Some(index) = previous
            .strip_prefix("member:")
            .and_then(|member| member.parse::<usize>().ok())
            && !list.iter().any(|(value, _, _)| *value == previous)
        {
            let name = members
                .iter()
                .find(|member| member.member == index)
                .map(|member| member.name.clone())
                .unwrap_or_default();
            list.push((
                previous.clone(),
                lang::format(
                    &lang::translate("\\[OFFLINE] %(player)s"),
                    &[("player", &name)],
                )
                .replace("\\[", "["),
                None,
            ));
        }

        let selected = list
            .iter()
            .position(|(value, _, _)| *value == previous)
            .unwrap_or(0);
        self.to.set_text_content(None);
        for (value, text, color) in &list {
            if let Some(option) = self.option(value, text) {
                if let Some(color) = color {
                    let _ = option.style().set_property("color", color);
                }
                let _ = self.to.append_child(&option);
            }
        }
        self.to.set_selected_index(selected as i32);
    }

    /// `ChatHistory.displayChatHistory`: the lines the filter keeps, drawn
    /// again. Stays at the bottom if it was there.
    fn draw_history(&mut self, palette: &[u32], hide_observers: bool, timestamps: bool) {
        let filter = self.filter.value();
        let you = self.chat.you();
        let at_bottom =
            self.history.scroll_height() - self.history.scroll_top() - self.history.client_height()
                < 8
                || self.history.child_element_count() == 0;
        self.history.set_text_content(None);
        for item in &self.items {
            match item {
                Item::Message { entry, player } => {
                    if hide_observers && entry.from_observer && Some(entry.from_member) != you {
                        continue;
                    }
                    let kept = match filter.as_str() {
                        "player" => *player,
                        "ally" => entry.channel == Channel::Allies,
                        "enemy" => entry.channel == Channel::Enemies,
                        "observer" => entry.channel == Channel::Observers,
                        "private" => entry.channel == Channel::Private,
                        _ => true,
                    };
                    if !kept {
                        continue;
                    }
                    if let Some(line) = chat::game_line(entry, timestamps) {
                        let color = name_color(palette, entry.from_player);
                        let element = chat::render(&self.document, &line, color);
                        let _ = self.history.append_child(&element);
                    }
                }
                Item::Notice {
                    notice,
                    received_ms,
                } => {
                    // As `ChatHistory.Filters`: an AI's line has a guid, so
                    // it is a message; the AI is no "player" in them.
                    let kept = matches!(
                        (filter.as_str(), notice.filter),
                        ("all", _)
                            | ("gamenotifications", Filter::Game)
                            | ("chatnotifications", Filter::Network)
                            | ("chat", Filter::Chat(_))
                            | ("ally", Filter::Chat(Channel::Allies))
                            | ("enemy", Filter::Chat(Channel::Enemies))
                            | ("observer", Filter::Chat(Channel::Observers))
                            | ("private", Filter::Chat(Channel::Private))
                    );
                    if kept {
                        let time = timestamps.then_some(*received_ms);
                        let line = self.notice_line(notice, time);
                        let _ = self.history.append_child(&line);
                    }
                }
                Item::Attack {
                    domestic,
                    attacker,
                    color,
                    received_ms,
                } => {
                    if filter == "all" || filter == "gamenotifications" {
                        let time = timestamps.then_some(*received_ms);
                        let line = self.attack_line(*domestic, attacker, color, time);
                        let _ = self.history.append_child(&line);
                    }
                }
                Item::System(text) => {
                    if filter == "all" {
                        let line = self.system_line(text);
                        let _ = self.history.append_child(&line);
                    }
                }
            }
        }
        if at_bottom {
            self.history.set_scroll_top(self.history.scroll_height());
        }
    }
}

fn parse_attack(value: &str) -> Option<Attack> {
    let mut parts = value.split(' ');
    let attack = Attack {
        target: parts.next()?.parse().ok()?,
        unit: parts.next()? == "1",
        x: parts.next()?.parse().ok()?,
        z: parts.next()?.parse().ok()?,
    };
    Some(attack)
}

/// `colorizePlayernameByID`: the player's colour, white for gaia and
/// observers.
fn name_color(palette: &[u32], player: Option<u8>) -> Option<String> {
    let color = player
        .filter(|player| *player != 0)
        .and_then(|player| palette.get(usize::from(player)))
        .copied()
        .unwrap_or(0xFFFF_FFFF);
    Some(css_color(color, 1.0))
}
