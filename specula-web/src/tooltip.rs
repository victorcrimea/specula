//! The engine's session tooltips (`CTooltip` with gui/common/setup.xml's
//! `sessionToolTip` styles) for the page's panels. An element that has one
//! carries its text in `data-tooltip`, in the engine's markup, which the
//! panels fill as they update; `data-tooltip-style` names a style other
//! than `sessionToolTip`. One `#tooltip` box shows the text of the element
//! under the mouse after the style's delay, at the style's offset from the
//! mouse, kept inside the window. On touch, which the engine does not
//! have, a press held `LONG_PRESS_MS` shows it until the next touch.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::Document;
use web_sys::Element;
use web_sys::HtmlElement;
use web_sys::MouseEvent;

use crate::lang::translate;

/// The engine's markup, set by the panels.
pub const ATTRIBUTE: &str = "data-tooltip";
/// A setup.xml tooltip name, `sessionToolTip` when absent.
pub const STYLE_ATTRIBUTE: &str = "data-tooltip-style";
const LONG_PRESS_MS: f64 = 500.0;
/// A touch that moves further is a pan, not a press.
const LONG_PRESS_SLOP: f64 = 10.0;
/// gui/common/setup.xml's colours, which `[color]` may name.
const COLORS: [(&str, &str); 16] = [
    ("black", "0 0 0"),
    ("white", "255 255 255"),
    ("red", "255 0 0"),
    ("blue", "0 0 255"),
    ("yellow", "255 255 0"),
    ("gold", "237 227 167"),
    ("lightgray", "243 242 240"),
    ("gray", "143 142 140"),
    ("darkgray", "43 42 40"),
    ("green", "0 200 0"),
    ("mustard", "191 191 2"),
    ("brown", "159 98 24"),
    ("orange", "255 165 0"),
    ("darkred", "80 0 0"),
    ("darkbrown", "30 20 10"),
    ("transparent", "0 0 0 0"),
];

/// A `<tooltip>` of setup.xml: its font, delay, offset and anchor.
struct Style {
    font: &'static str,
    delay_ms: f64,
    offset: [f64; 2],
    /// `anchor="bottom"`: the box ends at the offset rather than starts.
    bottom: bool,
}

fn style(name: &str) -> Style {
    match name {
        "sessionToolTipInstantly" => Style {
            font: "sans-14",
            delay_ms: 0.0,
            offset: [16.0, 32.0],
            bottom: false,
        },
        "sessionToolTipBold" => Style {
            font: "sans-bold-14",
            delay_ms: 100.0,
            offset: [16.0, 32.0],
            bottom: false,
        },
        "sessionToolTipBottom" => Style {
            font: "sans-14",
            delay_ms: 100.0,
            offset: [-4.0, -4.0],
            bottom: true,
        },
        "sessionToolTipBottomBold" => Style {
            font: "sans-bold-14",
            delay_ms: 100.0,
            offset: [-4.0, -4.0],
            bottom: true,
        },
        _ => Style {
            font: "sans-14",
            delay_ms: 100.0,
            offset: [16.0, 32.0],
            bottom: false,
        },
    }
}

/// Where the box is wanted, and from when.
struct Pending {
    target: Element,
    at: [f64; 2],
    /// `performance.now()` from which it shows.
    from: f64,
}

struct State {
    tooltip: HtmlElement,
    /// What the mouse is over, or what a long press showed.
    pending: Option<Pending>,
    /// The markup and style the box shows, `None` while hidden.
    shown: Option<(String, String)>,
    /// A touch that may become a long press: where it started, and when.
    press: Option<([f64; 2], f64, Element)>,
    /// The box stays for a long press until the next touch.
    held: bool,
    /// The box's style as last written.
    placed: String,
}

pub struct Tooltips {
    state: Rc<RefCell<State>>,
}

impl Tooltips {
    /// Over the page's `#tooltip`.
    pub fn new(document: &Document) -> Result<Self, String> {
        let tooltip: HtmlElement = crate::element(document, "tooltip")?;
        let state = Rc::new(RefCell::new(State {
            tooltip,
            pending: None,
            shown: None,
            press: None,
            held: false,
            placed: String::new(),
        }));
        wire(&state, document);
        Ok(Tooltips { state })
    }

    /// Shows the box once its delay is up, follows the markup of the
    /// element it is for, and hides it when that element is gone.
    pub fn frame(&self, now: f64) {
        let state = &mut *self.state.borrow_mut();
        if let Some((at, since, target)) = &state.press
            && now - since >= LONG_PRESS_MS
        {
            state.pending = Some(Pending {
                target: target.clone(),
                at: *at,
                from: now,
            });
            state.press = None;
            state.held = true;
        }
        let Some(pending) = &state.pending else {
            hide(state);
            return;
        };
        let markup = pending.target.get_attribute(ATTRIBUTE).unwrap_or_default();
        let visible = pending.target.is_connected()
            && pending
                .target
                .dyn_ref::<HtmlElement>()
                .is_none_or(|element| element.offset_parent().is_some());
        if markup.is_empty() || !visible {
            hide(state);
            return;
        }
        let name = pending
            .target
            .get_attribute(STYLE_ATTRIBUTE)
            .unwrap_or_default();
        let style = style(&name);
        if now < pending.from + style.delay_ms && !state.held {
            return;
        }
        let at = pending.at;
        if state.shown.as_ref() != Some(&(markup.clone(), name.clone())) {
            state.tooltip.set_inner_html(&to_html(&markup));
            // Measured in its font before `place` moves it.
            state.placed = font_css(style.font);
            let _ = state.tooltip.set_attribute("style", &state.placed);
            state.tooltip.set_hidden(false);
            state.shown = Some((markup, name));
        }
        place(state, &style, at);
    }
}

fn hide(state: &mut State) {
    if state.shown.take().is_some() {
        state.tooltip.set_hidden(true);
    }
    state.pending = None;
    state.held = false;
}

/// `CTooltip::SetupText`'s placement.
fn place(state: &mut State, style: &Style, at: [f64; 2]) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let width = f64::from(state.tooltip.offset_width());
    let height = f64::from(state.tooltip.offset_height());
    let window_width = window
        .inner_width()
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    let window_height = window
        .inner_height()
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    let mut left = at[0] + style.offset[0];
    let mut top = if style.bottom {
        at[1] + style.offset[1] - height
    } else {
        at[1] + style.offset[1]
    };
    if top < 0.0 {
        top = 0.0;
    } else if top + height > window_height {
        top = window_height - height;
    }
    if left < 0.0 {
        left = 0.0;
    } else if left + width > window_width {
        left = window_width - width;
    }
    let placed = format!(
        "{}transform: translate({left}px, {top}px)",
        font_css(style.font)
    );
    if placed != state.placed {
        let _ = state.tooltip.set_attribute("style", &placed);
        state.placed = placed;
    }
}

/// The element under `event` that has a tooltip, if any.
fn target(event: &MouseEvent) -> Option<Element> {
    event
        .target()?
        .dyn_into::<Element>()
        .ok()?
        .closest(&format!("[{ATTRIBUTE}]"))
        .ok()
        .flatten()
}

fn wire(state: &Rc<RefCell<State>>, document: &Document) {
    let shared = Rc::clone(state);
    let on_move = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        let state = &mut *shared.borrow_mut();
        let at = [f64::from(event.client_x()), f64::from(event.client_y())];
        if pointer_type(&event) != "mouse" {
            if let Some((start, _, _)) = &state.press
                && (at[0] - start[0]).hypot(at[1] - start[1]) > LONG_PRESS_SLOP
            {
                state.press = None;
            }
            return;
        }
        let now = now();
        match (target(&event), &mut state.pending) {
            (Some(target), Some(pending)) if pending.target == target => pending.at = at,
            (Some(target), _) => {
                // Moving onto another element with the box up shows its
                // text at once, as `GUITooltip` does.
                let from = if state.shown.is_some() { 0.0 } else { now };
                state.pending = Some(Pending { target, at, from });
            }
            (None, _) => state.pending = None,
        }
    });
    let _ =
        document.add_event_listener_with_callback("pointermove", on_move.as_ref().unchecked_ref());
    // The page lives as long as its tooltips.
    on_move.forget();

    let shared = Rc::clone(state);
    let on_down = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        let state = &mut *shared.borrow_mut();
        if pointer_type(&event) == "mouse" {
            // `GUITooltip` hides the box on a click.
            hide(state);
            return;
        }
        hide(state);
        state.press = target(&event).map(|target| {
            (
                [f64::from(event.client_x()), f64::from(event.client_y())],
                now(),
                target,
            )
        });
    });
    let _ =
        document.add_event_listener_with_callback("pointerdown", on_down.as_ref().unchecked_ref());
    on_down.forget();

    let shared = Rc::clone(state);
    let on_up = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if pointer_type(&event) != "mouse" {
            shared.borrow_mut().press = None;
        }
    });
    for name in ["pointerup", "pointercancel"] {
        let _ = document.add_event_listener_with_callback(name, on_up.as_ref().unchecked_ref());
    }
    on_up.forget();

    let shared = Rc::clone(state);
    let on_leave = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if pointer_type(&event) == "mouse" && event.related_target().is_none() {
            shared.borrow_mut().pending = None;
        }
    });
    let _ =
        document.add_event_listener_with_callback("pointerout", on_leave.as_ref().unchecked_ref());
    on_leave.forget();

    // A long press would otherwise open the browser's menu on some phones.
    let shared = Rc::clone(state);
    let on_menu = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if shared.borrow().held && target(&event).is_some() {
            event.prevent_default();
        }
    });
    let _ =
        document.add_event_listener_with_callback("contextmenu", on_menu.as_ref().unchecked_ref());
    on_menu.forget();
}

/// `pointerType`, read through the base `MouseEvent` a `PointerEvent` is.
fn pointer_type(event: &MouseEvent) -> String {
    js_sys::Reflect::get(event, &"pointerType".into())
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_default()
}

fn now() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
}

/// An engine font name's CSS, as `sans-bold-stroke-14`: weight, style,
/// size and outline.
fn font_css(font: &str) -> String {
    let mut css = String::new();
    for part in font.split('-') {
        match part {
            "bold" => css.push_str("font-weight: 700; "),
            "italic" => css.push_str("font-style: italic; "),
            "stroke" => css.push_str("-webkit-text-stroke: 2px black; paint-order: stroke fill; "),
            _ => {
                if let Ok(size) = part.parse::<u32>() {
                    css.push_str(&format!("font-size: {size}px; "));
                }
            }
        }
    }
    css
}

/// A `[color]` value, a name of `COLORS` or "r g b [a]", as CSS.
fn css_color(value: &str) -> Option<String> {
    let value = COLORS
        .iter()
        .find(|(name, _)| *name == value)
        .map_or(value, |(_, rgb)| rgb);
    let parts: Vec<u32> = value
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    match parts.as_slice() {
        [r, g, b] => Some(format!("rgb({r}, {g}, {b})")),
        [r, g, b, a] => Some(format!("rgba({r}, {g}, {b}, {})", f64::from(*a) / 255.0)),
        _ => None,
    }
}

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// A tag's quoted value of `key`, as `font="sans-13"`.
fn tag_value<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let start = tag.find(&format!("{key}=\""))? + key.len() + 2;
    let end = start + tag[start..].find('"')?;
    Some(&tag[start..end])
}

/// The engine's text markup (`CGUIString`: `[font]`, `[color]`, `[icon]`)
/// as HTML; other tags are dropped, their text kept.
pub fn to_html(markup: &str) -> String {
    let mut out = String::new();
    let mut rest = markup;
    while let Some(open) = rest.find('[') {
        escape(&rest[..open], &mut out);
        let Some(close) = rest[open..].find(']') else {
            escape(&rest[open..], &mut out);
            return out;
        };
        let tag = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        if tag.starts_with('/') {
            if tag == "/font" || tag == "/color" {
                out.push_str("</span>");
            }
        } else if tag.starts_with("font=") {
            let font = tag_value(tag, "font").unwrap_or_default();
            out.push_str(&format!("<span style=\"{}\">", font_css(font)));
        } else if tag.starts_with("color=") {
            let color = tag_value(tag, "color")
                .and_then(css_color)
                .unwrap_or_default();
            out.push_str(&format!("<span style=\"color: {color}\">"));
        } else if tag.starts_with("icon=") {
            // setup.xml's resource icons, `icon_<code>`.
            let icon = tag_value(tag, "icon").unwrap_or_default();
            if let Some(code) = icon.strip_prefix("icon_") {
                let mut name = String::new();
                escape(code, &mut name);
                out.push_str(&format!(
                    "<span class=\"icon\" style=\"background-image: var(--gui-icon-{name})\"></span>"
                ));
            }
        }
    }
    escape(rest, &mut out);
    out
}

/// `setStringTags`: the tags in object order, the last outermost.
pub fn tags(text: &str, tags: &[(&str, &str)]) -> String {
    let mut result = text.to_string();
    for (tag, value) in tags {
        result = format!("[{tag}=\"{value}\"]{result}[/{tag}]");
    }
    result
}

/// `coloredText`.
pub fn colored(text: &str, color: &str) -> String {
    tags(text, &[("color", color)])
}

/// tooltips.js' `g_TooltipTextFormats` helpers.
pub fn header_font(text: &str) -> String {
    tags(text, &[("font", "sans-bold-13")])
}

pub fn body_font(text: &str) -> String {
    tags(text, &[("font", "sans-13")])
}

pub fn objection_font(text: &str) -> String {
    tags(text, &[("font", "sans-bold-13"), ("color", "red")])
}

pub fn unit_font(text: &str) -> String {
    tags(text, &[("font", "sans-10"), ("color", "orange")])
}

/// `resourceIcon`, whose `displace` the box leaves to its CSS.
pub fn resource_icon(code: &str) -> String {
    format!("[icon=\"icon_{code}\" displace=\"0 3\"]")
}

/// `getSecondsString`.
pub fn seconds_string(seconds: i64) -> String {
    format!(
        "{seconds} {}",
        unit_font(if seconds == 1 { "second" } else { "seconds" })
    )
}

/// `getCurrentHealthTooltip`, with `label` or "Health:".
pub fn current_health(hitpoints: f64, max: f64, label: Option<&str>) -> String {
    if max <= 0.0 {
        return String::new();
    }
    format!(
        "{} {} / {}",
        header_font(&label.map_or_else(|| translate("Health:"), str::to_string)),
        hitpoints.round(),
        max.round()
    )
}

/// `getCurrentCaptureTooltip`, with `label` or "Capture points:".
pub fn current_capture(points: f64, max: f64, label: Option<&str>) -> String {
    if max <= 0.0 {
        return String::new();
    }
    format!(
        "{} {} / {}",
        header_font(&label.map_or_else(|| translate("Capture points:"), str::to_string)),
        points.round(),
        max.round()
    )
}

/// `getEntityNames` at the default gui.session.howtoshownames 0.
pub fn entity_names(generic: &str, specific: &str) -> String {
    if specific.is_empty() {
        return generic.to_string();
    }
    if specific == generic || generic.is_empty() {
        return specific.to_string();
    }
    format!("{specific} ({generic})")
}

/// A resource's `name` and `description` in simulation/data/resources/.
pub fn resource_data(code: &str) -> Option<(&'static str, &'static str)> {
    match code {
        "food" => Some((
            "Food",
            "Harvest from animals, berry bushes, fish, or fields.",
        )),
        "wood" => Some(("Wood", "Cut down from trees or groves.")),
        "stone" => Some(("Stone", "Mine from rocks, stone quarries or ruins.")),
        "metal" => Some(("Metal", "Mine from metal mines or quarries.")),
        _ => None,
    }
}
