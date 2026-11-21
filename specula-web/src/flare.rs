//! Flares, as the session's do: `triggerFlareAction` and `handleFlare`
//! (`gui/session/input.js`, `gui/session/messages.js`) limit how often one
//! is sent and received, and `CMiniMap::DrawFlare` animates it on the
//! minimap. The 3D marker is `field.rs`'s.

use std::collections::VecDeque;

use web_sys::Document;
use web_sys::HtmlButtonElement;
use web_sys::HtmlElement;

use crate::command_row;
use crate::game_menu;
use crate::lang;
use crate::panel;
use crate::tooltip;

/// `g_FlareCooldown`.
const COOLDOWN_MS: f64 = 3000.0;
/// `g_FlareRateLimitScope` and `g_FlareRateLimitMaximumFlares`.
const RECEIVE_SCOPE_MS: f64 = 6000.0;
const RECEIVE_MAXIMUM: usize = 16;
/// The group of `g_FlareSound`.
pub const SOUND: &str = "interface/alarm/alarm_ally.xml";
/// `gui.session.flarelifetime`'s default (config/default.cfg) and range
/// (`options.json`).
pub(crate) const DEFAULT_LIFETIME_S: f64 = 6.0;
pub(crate) const MAX_LIFETIME_S: f64 = 60.0;
/// `flare_animation_speed`, `flare_texture_count`, `flare_start_fade_seconds`,
/// `flare_stop_fade_seconds` and `flare_render_size` of
/// `gui/session/minimap/MiniMap.xml`; the render size is a half size.
const ANIMATION_SPEED: f64 = 10.67;
const FRAMES: u32 = 16;
const START_FADE_S: f64 = 0.25;
const STOP_FADE_S: f64 = 0.5;
const HALF_SIZE_PX: f64 = 32.0;

pub const LIFETIME_SETTINGS_KEY: &str = "specula.flarelifetime";

struct Shown {
    x: f32,
    z: f32,
    at: f64,
    element: HtmlElement,
    frames: [HtmlElement; 2],
}

pub struct Flares {
    pub(crate) button: HtmlButtonElement,
    layer: HtmlElement,
    /// The page sends as an observer, whose flares reach only the other
    /// observers.
    pub(crate) observer: bool,
    /// The button's mode (`INPUT_FLARE`).
    pub(crate) toggled: bool,
    /// K is down (`session.flare`).
    pub(crate) held: bool,
    last_sent: Option<f64>,
    received: VecDeque<f64>,
    shown: Vec<Shown>,
    lifetime_s: f64,
}

impl Flares {
    pub fn new(document: &Document, observer: bool) -> Result<Self, String> {
        let button: HtmlButtonElement = crate::element(document, "flare")?;
        let layer: HtmlElement = crate::element(document, "minimap-flares")?;
        if observer {
            let _ = button.set_attribute("data-observer", "");
        }
        let label = lang::translate(if observer {
            "Send a flare to other observers."
        } else {
            "Send a flare to your allies."
        });
        let _ = button.set_attribute("aria-label", &label);
        let _ = button.set_attribute(
            tooltip::ATTRIBUTE,
            &format!("{}{}", command_row::hotkey_prefix("session.flare"), label),
        );
        button.set_hidden(false);
        Ok(Flares {
            button,
            layer,
            observer,
            toggled: false,
            held: false,
            last_sent: None,
            received: VecDeque::new(),
            shown: Vec::new(),
            lifetime_s: DEFAULT_LIFETIME_S,
        })
    }

    pub fn armed(&self) -> bool {
        self.toggled || self.held
    }

    pub fn cursor(&self) -> &'static str {
        if self.observer {
            "action-observer-flare"
        } else {
            "action-player-flare"
        }
    }

    /// The button enters the mode, as `INPUT_FLARE` does; pressing it again
    /// keeps it.
    pub fn press_button(&mut self) {
        self.toggled = true;
    }

    pub fn set_held(&mut self, held: bool) {
        self.held = held;
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.button.disabled() == enabled {
            self.button.set_disabled(!enabled);
        }
        if !enabled {
            self.toggled = false;
            self.held = false;
        }
    }

    /// Whether a flare may be sent now, which records it.
    pub fn take_send(&mut self, now: f64) -> bool {
        if self.last_sent.is_some_and(|last| now < last + COOLDOWN_MS) {
            return false;
        }
        self.last_sent = Some(now);
        true
    }

    /// Whether a flare may be shown now, which records it: vanilla's
    /// receive-side limit, so a busy stream cannot flood the screen.
    pub fn take_received(&mut self, now: f64) -> bool {
        self.received.retain(|at| now - at < RECEIVE_SCOPE_MS);
        if self.received.len() >= RECEIVE_MAXIMUM {
            return false;
        }
        self.received.push_back(now);
        true
    }

    pub fn add(&mut self, document: &Document, x: f32, z: f32, color: u32, now: f64) {
        if self.lifetime_s == 0.0 {
            return;
        }
        let Ok(element) = panel::create::<HtmlElement>(document, "div") else {
            return;
        };
        element.set_class_name("flare");
        let _ = element
            .style()
            .set_property("--flare-color", &panel::css_color(color, 1.0));
        let mut frames = Vec::with_capacity(2);
        for _ in 0..2 {
            let Ok(frame) = panel::create::<HtmlElement>(document, "div") else {
                return;
            };
            frame.set_class_name("flare-frame");
            frame.set_hidden(true);
            let _ = element.append_child(&frame);
            frames.push(frame);
        }
        let _ = self.layer.append_child(&element);
        let [first, second] = frames.try_into().unwrap_or_else(|_| unreachable!());
        self.shown.push(Shown {
            x,
            z,
            at: now,
            element,
            frames: [first, second],
        });
    }

    pub fn set_lifetime(&mut self, seconds: f64) {
        self.lifetime_s = seconds.clamp(0.0, MAX_LIFETIME_S);
    }

    pub fn frame(&mut self, now: f64, map_size: f32) {
        if self.shown.is_empty() {
            return;
        }
        let width = self.layer.client_width() as f64;
        let height = self.layer.client_height() as f64;
        let side = width.min(height);
        let mut index = 0;
        while index < self.shown.len() {
            let shown = &mut self.shown[index];
            let dt = (now - shown.at) / 1000.0;
            if dt > self.lifetime_s {
                let _ = self.layer.remove_child(&shown.element);
                self.shown.remove(index);
                continue;
            }
            if map_size > 0.0 && width > 0.0 && height > 0.0 {
                let u = 2.0 * f64::from(shown.x) / f64::from(map_size) - 1.0;
                let v = 2.0 * f64::from(shown.z) / f64::from(map_size) - 1.0;
                let left = (u * side / width + 1.0) / 2.0 * width;
                let top = (1.0 - v * side / height) / 2.0 * height;
                let style = shown.element.style();
                let _ = style.set_property("left", &format!("{left}px"));
                let _ = style.set_property("top", &format!("{top}px"));
            }
            let step = (dt * ANIMATION_SPEED).floor();
            set_frame(&shown.frames[0], (step as u64 % u64::from(FRAMES)) as u32);
            let second = step >= f64::from(FRAMES / 2);
            if second {
                set_frame(
                    &shown.frames[1],
                    ((step as u64 - u64::from(FRAMES / 2)) % u64::from(FRAMES)) as u32,
                );
            }
            shown.frames[1].set_hidden(!second);
            let alpha = smoothstep(dt / START_FADE_S)
                .min(smoothstep((self.lifetime_s - dt) / STOP_FADE_S))
                .clamp(0.0, 1.0);
            let _ = shown
                .element
                .style()
                .set_property("opacity", &format!("{alpha}"));
            index += 1;
        }
    }
}

/// One frame of the strip, which is `FRAMES` frames wide. The mask has no
/// standard `mask-position-x`, so the whole position is set.
fn set_frame(frame: &HtmlElement, index: u32) {
    let position = format!("-{}px 0", index * 2 * HALF_SIZE_PX as u32);
    let _ = frame.style().set_property("mask-position", &position);
}

/// `SmoothStep(0, 1, t)`.
fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Sends a flare; the caller shows it at once, as vanilla does.
pub async fn send(observer: bool, x: f32, z: f32) {
    let result = if observer {
        game_menu::post_path(&format!("/flare?x={x}&z={z}")).await
    } else {
        game_menu::post(&format!("flare?x={x}&z={z}")).await
    };
    if let Err(error) = result {
        web_sys::console::warn_1(&error.into());
    }
}
