//! The heartbeat behind the server's `/stats`: while the page is shown, one
//! beacon a minute says how long it was watched, in which mode, with which
//! renderer, settings and frame times. The visitor is the server's `sv`
//! cookie, which a same-origin beacon carries.
//!
//! The browser APIs are reached through `Reflect`, as `lang.rs` reaches
//! `navigator`, so that no web-sys feature is added for them.

use std::cell::RefCell;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::HtmlInputElement;
use web_sys::HtmlSelectElement;

/// Soon enough to count a visitor who leaves within the first minute.
const FIRST_BEAT_MS: f64 = 15_000.0;
const BEAT_MS: f64 = 60_000.0;
/// A longer gap between frames is a hidden tab or a sleeping device, which
/// is not watching.
const MAX_FRAME_GAP_MS: f64 = 5_000.0;
const URL: &str = "/beat";

#[derive(Default)]
struct Beat {
    backend: &'static str,
    mode: &'static str,
    /// Watched milliseconds not yet reported.
    watched: f64,
    last_frame: Option<f64>,
    next_at: Option<f64>,
    windows: u32,
    fps: f64,
    cpu: f64,
    cpu_windows: u32,
    gpu: f64,
    gpu_windows: u32,
}

thread_local! {
    static BEAT: RefCell<Beat> = RefCell::new(Beat::default());
}

/// One stats window of the 3D view; `cpu` and `gpu` in milliseconds, `None`
/// where not measured.
pub fn record(backend: &'static str, fps: f64, cpu: Option<f64>, gpu: Option<f64>) {
    BEAT.with(|beat| {
        let mut beat = beat.borrow_mut();
        beat.backend = backend;
        beat.windows += 1;
        beat.fps += fps;
        if let Some(cpu) = cpu {
            beat.cpu += cpu;
            beat.cpu_windows += 1;
        }
        if let Some(gpu) = gpu {
            beat.gpu += gpu;
            beat.gpu_windows += 1;
        }
    });
}

/// Sends what is left when the page goes away, so that a visit's last
/// minute is not lost.
pub fn start() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let on_hide = Closure::<dyn FnMut()>::new(|| {
        let line = BEAT.with(|beat| {
            let mut beat = beat.borrow_mut();
            (beat.watched >= 1000.0).then(|| take_line(&mut beat))
        });
        if let Some(line) = line {
            send(&line);
        }
    });
    let _ = window.add_event_listener_with_callback("pagehide", on_hide.as_ref().unchecked_ref());
    // The listener lives as long as the page.
    on_hide.forget();
}

/// Called every frame; `mode` is "live", "replay" or "map".
pub fn frame(now: f64, mode: &'static str) {
    let line = BEAT.with(|beat| {
        let mut beat = beat.borrow_mut();
        beat.mode = mode;
        if let Some(last) = beat.last_frame.replace(now) {
            let gap = now - last;
            if gap <= MAX_FRAME_GAP_MS {
                beat.watched += gap;
            }
        }
        let next_at = *beat.next_at.get_or_insert(now + FIRST_BEAT_MS);
        (now >= next_at && visible()).then(|| {
            beat.next_at = Some(now + BEAT_MS);
            take_line(&mut beat)
        })
    });
    if let Some(line) = line {
        send(&line);
    }
}

/// The renderer could not start, which no beat would ever tell.
pub fn fail(reason: &str) {
    let reason: String = reason
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .take(200)
        .collect();
    send(&format!("fail\t{reason}"));
}

fn take_line(beat: &mut Beat) -> String {
    let average = |sum: f64, count: u32| {
        if count > 0 {
            format!("{:.1}", sum / f64::from(count))
        } else {
            "-".to_string()
        }
    };
    let backend = if beat.backend.is_empty() {
        "-"
    } else {
        beat.backend
    };
    let line = format!(
        "beat\t{}\t{}\t{backend}\t{}\t{}\t{}\t{}",
        (beat.watched / 1000.0).round(),
        beat.mode,
        average(beat.fps, beat.windows),
        average(beat.cpu, beat.cpu_windows),
        average(beat.gpu, beat.gpu_windows),
        settings(),
    );
    beat.watched = 0.0;
    beat.windows = 0;
    beat.fps = 0.0;
    beat.cpu = 0.0;
    beat.cpu_windows = 0;
    beat.gpu = 0.0;
    beat.gpu_windows = 0;
    line
}

fn visible() -> bool {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return false;
    };
    js_sys::Reflect::get(&document, &"visibilityState".into())
        .ok()
        .and_then(|state| state.as_string())
        // An old browser without the property is shown whenever it draws.
        .is_none_or(|state| state == "visible")
}

/// `id=value` of every list and checkbox in the settings, space separated.
fn settings() -> String {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return String::new();
    };
    let Ok(query) = js_sys::Reflect::get(&document, &"querySelectorAll".into()) else {
        return String::new();
    };
    let Ok(query) = query.dyn_into::<js_sys::Function>() else {
        return String::new();
    };
    let Ok(nodes) = query.call1(&document, &"#settings select, #settings input".into()) else {
        return String::new();
    };
    let mut pairs = Vec::new();
    for node in js_sys::Array::from(&nodes).iter() {
        if let Some(select) = node.dyn_ref::<HtmlSelectElement>() {
            pairs.push((select.id(), select.value()));
        } else if let Some(input) = node.dyn_ref::<HtmlInputElement>() {
            let value = match input.type_().as_str() {
                "checkbox" => if input.checked() { "1" } else { "0" }.to_string(),
                "range" | "number" => input.value(),
                _ => continue,
            };
            pairs.push((input.id(), value));
        }
    }
    let clean = |text: &str| -> String {
        text.chars()
            .filter(|c| !c.is_whitespace() && *c != '=')
            .collect()
    };
    pairs
        .iter()
        .filter(|(id, _)| !id.is_empty())
        .map(|(id, value)| format!("{}={}", clean(id), clean(value)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn send(line: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(navigator) = js_sys::Reflect::get(&window, &"navigator".into()) else {
        return;
    };
    let Ok(send_beacon) = js_sys::Reflect::get(&navigator, &"sendBeacon".into()) else {
        return;
    };
    if let Ok(send_beacon) = send_beacon.dyn_into::<js_sys::Function>() {
        let _ = send_beacon.call2(&navigator, &URL.into(), &JsValue::from_str(line));
    }
}
