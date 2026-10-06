mod assets;
mod blend;
mod diplomacy;
mod far;
mod field;
mod gpu;
mod icons;
mod impostor;
mod march;
mod minimap;
mod opfs;
mod pose;
mod render;
mod stream;
mod terrain;
mod unit;
mod variation;
mod worker;

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen_futures::JsFuture;
use web_sys::Document;
use web_sys::HtmlButtonElement;
use web_sys::HtmlCanvasElement;
use web_sys::HtmlElement;
use web_sys::HtmlInputElement;
use web_sys::HtmlOptionElement;
use web_sys::HtmlSelectElement;
use web_sys::MessageEvent;
use web_sys::Response;
use web_sys::Worker;
use web_sys::WorkerOptions;
use web_sys::WorkerType;

use crate::assets::Progress;
use crate::diplomacy::Perspective;
use crate::field::Field;
use crate::icons::IconAtlas;
use crate::minimap::Minimap;
use crate::render::Renderer;
use crate::stream::Scene;
use crate::terrain::TerrainImage;
use crate::worker::COMMAND_CURRENT_MATCH;
use crate::worker::COMMAND_LIVE;
use crate::worker::COMMAND_PAUSE;
use crate::worker::COMMAND_PLAY;
use crate::worker::COMMAND_SCRUB;
use crate::worker::COMMAND_SEEK;
use crate::worker::COMMAND_SPEED;
use crate::worker::POST_SCENE;
use crate::worker::POST_STATUS;
use crate::worker::STATUS_ENDED;
use crate::worker::STATUS_LIVE;
use crate::worker::STATUS_NEXT_READY;
use crate::worker::STATUS_PAUSED;

type FrameCallback = Closure<dyn FnMut(f64)>;

/// A multiplayer turn is 200 ms of simulation time (DEFAULT_TURN_LENGTH);
/// the real interval depends on game speed and is measured.
const TURN_MS: f64 = 200.0;
const TURN_MS_MIN: f64 = 50.0;
const TURN_MS_MAX: f64 = 1000.0;
/// Low enough that network jitter does not make the pace wobble.
const TURN_SMOOTHING: f64 = 0.1;
/// Playback speeds in game time; the match's own speed is added if missing.
const SPEEDS: [f32; 5] = [0.5, 1.0, 2.0, 4.0, 8.0];

/// What draws the match: the minimap view of `/map.html` or the 3D view
/// of `/`. The stream, playback and controls are the same.
trait View {
    fn max_size(&self) -> u32;
    fn resize(&mut self, width: u32, height: u32, device_pixel_ratio: f32);
    /// Returns false when no frame could be drawn, so the caller retries.
    fn draw(&mut self, progress: f32) -> bool;
    /// `palette` is the displayed colour of each player id.
    fn set_scene(&mut self, scene: &Scene, palette: &[u32]);
    /// `None` for a new match whose terrain has not arrived yet.
    fn set_terrain(&mut self, image: Option<&TerrainImage>);
    fn set_icon_atlas(&mut self, _atlas: IconAtlas) {}
    /// Whether it moves between turns on its own, and so needs every frame.
    fn animating(&self) -> bool {
        false
    }
    /// Downloads to show on the loading bar.
    fn progress(&self) -> Progress {
        Progress::Idle
    }
    /// What the stream has brought since the page loaded, for statistics.
    fn set_stream_bytes(&mut self, _bytes: StreamBytes) {}
    /// The ground the camera sees, x and z in metres, for the minimap.
    fn view_outline(&self) -> Option<[[f32; 2]; 4]> {
        None
    }
    /// Looks at the ground point `x`, `z` in metres.
    fn move_to(&mut self, _x: f32, _z: f32) {}
}

/// Bytes since the page loaded: live messages and history segments, as the
/// worker counts them, and the terrain.
#[derive(Clone, Copy, Default)]
struct StreamBytes {
    live: u64,
    segments: u64,
    terrain: u64,
}

impl View for Renderer {
    fn max_size(&self) -> u32 {
        Renderer::max_size(self)
    }

    fn resize(&mut self, width: u32, height: u32, device_pixel_ratio: f32) {
        Renderer::resize(self, width, height, device_pixel_ratio);
    }

    fn draw(&mut self, progress: f32) -> bool {
        Renderer::draw(self, progress)
    }

    fn set_scene(&mut self, scene: &Scene, palette: &[u32]) {
        Renderer::set_scene(self, scene, palette);
    }

    fn set_terrain(&mut self, image: Option<&TerrainImage>) {
        Renderer::set_terrain(self, image);
    }

    fn set_icon_atlas(&mut self, atlas: IconAtlas) {
        Renderer::set_icon_atlas(self, atlas);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Map,
    Observer,
}

/// The observer page's download bar, `#loading` with `#loading-fill` and
/// `#loading-text`.
struct LoadingBar {
    bar: HtmlElement,
    fill: HtmlElement,
    text: HtmlElement,
    /// What is shown, so the DOM is touched only on change.
    shown: String,
}

impl LoadingBar {
    fn show(&mut self, progress: Progress) {
        let (label, width, busy) = match progress {
            Progress::Idle => (String::new(), 0.0, false),
            Progress::Starting => ("Loading the model index".to_string(), 0.0, true),
            Progress::Bytes { done, total } => (
                format!(
                    "Loading models {:.1} / {:.1} MB",
                    done as f64 / 1e6,
                    total as f64 / 1e6
                ),
                done as f64 / total.max(1) as f64 * 100.0,
                true,
            ),
        };
        let key = format!("{label}|{width:.0}");
        if key == self.shown {
            return;
        }
        self.shown = key;
        let _ = self
            .bar
            .set_attribute("style", if busy { "" } else { "display: none" });
        let _ = self
            .fill
            .set_attribute("style", &format!("width: {width:.0}%"));
        self.text.set_text_content(Some(&label));
    }
}

struct App {
    canvas: HtmlCanvasElement,
    renderer: Box<dyn View>,
    minimap: Option<Minimap>,
    loading: Option<LoadingBar>,
    scene: Scene,
    /// How far the last drawn frame moved the dots towards the current turn.
    progress: f32,
    /// Frame timestamp at which the current turn started to play.
    turn_start: f64,
    turn_interval: f64,
    /// Live turns come at the host's pace, which is measured; played back
    /// ones come at a pace the worker states.
    measure_turns: bool,
    turn_arrived: bool,
    scene_changed: bool,
    dirty: bool,
    /// The terrain shown or being fetched; 0 for none.
    terrain_id: u64,
    diplomacy_colors: bool,
    perspective: Perspective,
    /// The displayed colour of each player id.
    palette: Vec<u32>,
    diplomacy_button: HtmlButtonElement,
    perspective_select: HtmlSelectElement,
    /// The players the perspective list was built for, gaia excluded.
    player_names: Vec<String>,
    worker: Worker,
    clock: HtmlElement,
    /// The turn the clock shows.
    clock_turn: Option<u32>,
    play_button: HtmlButtonElement,
    speed_select: HtmlSelectElement,
    /// The match speed the speed list was built for; 0 before any.
    speeds_for: f32,
    timeline: HtmlInputElement,
    /// Status updates must not pull the thumb from under the pointer.
    dragging: bool,
    live_button: HtmlButtonElement,
    live: bool,
    paused: bool,
    stream_bytes: StreamBytes,
    match_over: MatchOver,
}

/// The popup and button shown while the match on screen has ended.
struct MatchOver {
    button: HtmlButtonElement,
    popup: HtmlElement,
    text: HtmlElement,
    popup_button: HtmlButtonElement,
    close_button: HtmlButtonElement,
    ended: bool,
    next_ready: bool,
    /// When the worker moves on, on the frame clock.
    switch_at: f64,
    /// Closed by the viewer; it opens again when another match ends.
    closed: bool,
    shown: String,
}

impl MatchOver {
    fn show_status(&mut self, flags: u8, switch_in_ms: u32, now: f64) {
        let ended = flags & STATUS_ENDED != 0;
        if ended && !self.ended {
            self.closed = false;
        }
        self.ended = ended;
        self.next_ready = flags & STATUS_NEXT_READY != 0;
        self.switch_at = now + f64::from(switch_in_ms);
        self.button.set_hidden(!ended);
        self.button.set_disabled(!self.next_ready);
        self.popup_button.set_disabled(!self.next_ready);
        self.popup.set_hidden(!ended || self.closed);
    }

    fn close(&mut self) {
        self.closed = true;
        self.popup.set_hidden(true);
    }

    fn frame(&mut self, now: f64) {
        if !self.ended || self.closed {
            return;
        }
        let left = self.switch_at - now;
        let text = if left > 0.0 {
            let seconds = (left / 1000.0).ceil() as u64;
            format!(
                "Match over. Next match in {}:{:02}",
                seconds / 60,
                seconds % 60
            )
        } else {
            "Match over. Waiting for the next match".to_string()
        };
        if text != self.shown {
            self.text.set_text_content(Some(&text));
            self.shown = text;
        }
    }
}

impl App {
    /// `now` is the requestAnimationFrame timestamp in milliseconds.
    fn frame(&mut self, now: f64) {
        let (width, height, ratio) = canvas_size(&self.canvas, self.renderer.max_size());
        if width != self.canvas.width() || height != self.canvas.height() {
            self.canvas.set_width(width);
            self.canvas.set_height(height);
            self.renderer.resize(width, height, ratio);
            self.dirty = true;
        }
        if self.turn_arrived {
            self.turn_arrived = false;
            let gap = now - self.turn_start;
            // A longer gap is a stall or a hidden tab, not the game's pace.
            if self.measure_turns && gap <= TURN_MS_MAX {
                self.turn_interval += (gap - self.turn_interval) * TURN_SMOOTHING;
                self.turn_interval = self.turn_interval.clamp(TURN_MS_MIN, TURN_MS_MAX);
            }
            self.turn_start = now;
        }
        if self.scene_changed {
            self.scene_changed = false;
            self.renderer.set_scene(&self.scene, &self.palette);
            if let Some(minimap) = &mut self.minimap {
                minimap.set_scene(&self.scene, &self.palette);
            }
            self.dirty = true;
        }
        if let Some([x, z]) = self.minimap.as_ref().and_then(Minimap::take_target) {
            self.renderer.move_to(x, z);
        }
        if let Some(loading) = &mut self.loading {
            loading.show(self.renderer.progress());
        }
        self.renderer.set_stream_bytes(self.stream_bytes);
        let progress = ((now - self.turn_start) / self.turn_interval).clamp(0.0, 1.0) as f32;
        if self.dirty || progress != self.progress || self.renderer.animating() {
            self.dirty = !self.renderer.draw(progress);
            if !self.dirty {
                self.progress = progress;
            }
        }
        if let Some(minimap) = &mut self.minimap {
            minimap.frame(progress, self.renderer.view_outline());
        }
        self.match_over.frame(now);
    }

    /// A message replaced the player table.
    fn players_changed(&mut self) {
        let players = &self.scene.players;
        if !players
            .iter()
            .skip(1)
            .map(|player| &player.name)
            .eq(self.player_names.iter())
        {
            self.player_names = players
                .iter()
                .skip(1)
                .map(|player| player.name.clone())
                .collect();
            // Keeps the "Observer" entry from the page.
            self.perspective_select.set_length(1);
            for (index, name) in self.player_names.iter().enumerate() {
                let id = (index + 1).to_string();
                if let Ok(option) = HtmlOptionElement::new_with_text_and_value(name, &id) {
                    let _ = self
                        .perspective_select
                        .add_with_html_option_element(&option);
                }
            }
            // Other names are another match, where the old choice means
            // nothing.
            self.perspective = Perspective::Observer;
            self.perspective_select.set_value("0");
        }
        self.update_palette();
    }

    fn show_controls(&self) {
        let pressed = if self.diplomacy_colors {
            "true"
        } else {
            "false"
        };
        let _ = self.diplomacy_button.set_attribute("aria-pressed", pressed);
        // Without diplomacy colours every perspective looks the same.
        self.perspective_select.set_disabled(!self.diplomacy_colors);
    }

    fn command(&self, command: u8, value: f64) {
        let mut bytes = vec![command];
        bytes.extend_from_slice(&value.to_le_bytes());
        let _ = self
            .worker
            .post_message(&js_sys::Uint8Array::from(&bytes[..]));
    }

    fn show_clock(&mut self) {
        let turn = self.scene.turn;
        if self.clock_turn != Some(turn) {
            self.clock_turn = Some(turn);
            self.clock.set_text_content(Some(&format_clock(turn)));
        }
    }

    fn show_status(&mut self, first: u32, newest: u32, playhead: u32, flags: u8, speed: f32) {
        self.live = flags & STATUS_LIVE != 0;
        self.paused = flags & STATUS_PAUSED != 0;
        self.play_button.set_disabled(false);
        self.play_button
            .set_text_content(Some(if self.paused { "Play" } else { "Pause" }));
        self.live_button.set_disabled(false);
        let pressed = if self.live { "true" } else { "false" };
        let _ = self.live_button.set_attribute("aria-pressed", pressed);
        self.timeline.set_disabled(false);
        if !self.dragging {
            self.timeline.set_min(&first.to_string());
            self.timeline.set_max(&newest.to_string());
            self.timeline.set_value(&playhead.to_string());
        }
        let match_speed = self.scene.game_speed;
        if match_speed != self.speeds_for {
            self.speeds_for = match_speed;
            self.speed_select.set_length(0);
            let mut speeds = SPEEDS.to_vec();
            if !speeds.contains(&match_speed) {
                speeds.push(match_speed);
                speeds.sort_by(f32::total_cmp);
            }
            for speed in speeds {
                let label = format!("{speed}x");
                if let Ok(option) =
                    HtmlOptionElement::new_with_text_and_value(&label, &speed.to_string())
                {
                    let _ = self.speed_select.add_with_html_option_element(&option);
                }
            }
        }
        self.speed_select.set_value(&speed.to_string());
        // Live plays at the match's own pace.
        self.speed_select.set_disabled(self.live);
    }

    fn update_palette(&mut self) {
        self.palette =
            diplomacy::palette(&self.scene.players, self.diplomacy_colors, self.perspective);
        self.scene_changed = true;
    }
}

fn set_panic_hook() {
    // Without this a panic shows up only as "unreachable" in the console.
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&info.to_string().into());
    }));
}

/// Entry point of map.html; the stream worker's is `worker::worker_main`.
#[wasm_bindgen]
pub fn page_main() {
    set_panic_hook();
    wasm_bindgen_futures::spawn_local(async {
        if let Err(error) = run(Page::Map, None).await {
            web_sys::console::error_1(&error.into());
        }
    });
}

/// Entry point of index.html: the same stream and controls as the
/// page's, drawn in 3D. `glue` is the URL the page imported this module's
/// glue from; the stream worker imports the same one, so a cached older
/// worker can never pair an old glue with this wasm.
#[wasm_bindgen]
pub fn observer_main(glue: String) {
    set_panic_hook();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = run(Page::Observer, Some(glue)).await {
            web_sys::console::error_1(&error.into());
        }
    });
}

/// Entry point of unit.html.
#[wasm_bindgen]
pub fn unit_main() {
    set_panic_hook();
    wasm_bindgen_futures::spawn_local(async {
        if let Err(error) = unit::run().await {
            web_sys::console::error_1(&error.into());
        }
    });
}

async fn run(page: Page, glue: Option<String>) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let canvas: HtmlCanvasElement = window
        .document()
        .and_then(|document| document.get_element_by_id("view"))
        .and_then(|element| element.dyn_into().ok())
        .ok_or("no <canvas id=\"view\">")?;
    // The real limit is known only once there is a device; start within the
    // WebGL2 minimum and let the first frame resize.
    let (width, height, ratio) = canvas_size(&canvas, 2048);
    canvas.set_width(width);
    canvas.set_height(height);
    let renderer: Box<dyn View> = match page {
        Page::Map => Box::new(Renderer::new(canvas.clone(), ratio).await?),
        Page::Observer => Box::new(Field::new(canvas.clone()).await?),
    };
    let document = window.document().ok_or("no document")?;
    let minimap = match page {
        Page::Map => None,
        Page::Observer => Some(Minimap::new(element(&document, "minimap")?).await?),
    };
    let loading = match page {
        Page::Map => None,
        Page::Observer => Some(LoadingBar {
            bar: element(&document, "loading")?,
            fill: element(&document, "loading-fill")?,
            text: element(&document, "loading-text")?,
            shown: String::new(),
        }),
    };
    let diplomacy_button = element(&document, "diplomacy")?;
    let perspective_select = element(&document, "perspective")?;
    let options = WorkerOptions::new();
    options.set_type(WorkerType::Module);
    let worker_url = match &glue {
        Some(glue) => format!("worker.js?glue={}", js_sys::encode_uri_component(glue)),
        None => "worker.js".to_string(),
    };
    let worker = Worker::new_with_options(&worker_url, &options)
        .map_err(|_| "cannot start the stream worker")?;

    let app = Rc::new(RefCell::new(App {
        canvas,
        renderer,
        minimap,
        loading,
        scene: Scene::default(),
        progress: 1.0,
        turn_start: 0.0,
        turn_interval: TURN_MS,
        measure_turns: true,
        turn_arrived: false,
        scene_changed: false,
        dirty: true,
        terrain_id: 0,
        diplomacy_colors: false,
        perspective: Perspective::Observer,
        palette: Vec::new(),
        diplomacy_button,
        perspective_select,
        player_names: Vec::new(),
        worker,
        clock: element(&document, "clock")?,
        clock_turn: None,
        play_button: element(&document, "play")?,
        speed_select: element(&document, "speed")?,
        speeds_for: 0.0,
        timeline: element(&document, "timeline")?,
        dragging: false,
        live_button: element(&document, "live")?,
        live: true,
        paused: false,
        stream_bytes: StreamBytes::default(),
        match_over: MatchOver {
            button: element(&document, "current")?,
            popup: element(&document, "match-over")?,
            text: element(&document, "match-over-text")?,
            popup_button: element(&document, "match-over-current")?,
            close_button: element(&document, "match-over-close")?,
            ended: false,
            next_ready: false,
            switch_at: 0.0,
            closed: false,
            shown: String::new(),
        },
    }));
    wire_controls(&app);
    wire_playback(&app);
    connect(&app);
    wasm_bindgen_futures::spawn_local(load_icons(Rc::clone(&app)));
    animate(app);
    Ok(())
}

fn element<T: JsCast>(document: &Document, id: &str) -> Result<T, String> {
    document
        .get_element_by_id(id)
        .and_then(|element| element.dyn_into().ok())
        .ok_or_else(|| format!("no element #{id} of the right kind"))
}

fn wire_controls(app: &Rc<RefCell<App>>) {
    let (button, select) = {
        let app = app.borrow();
        (app.diplomacy_button.clone(), app.perspective_select.clone())
    };

    let target = Rc::clone(app);
    let on_click = Closure::<dyn FnMut()>::new(move || {
        let app = &mut *target.borrow_mut();
        app.diplomacy_colors = !app.diplomacy_colors;
        app.show_controls();
        app.update_palette();
    });
    button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_click.forget();

    let target = Rc::clone(app);
    let on_change = Closure::<dyn FnMut()>::new(move || {
        let app = &mut *target.borrow_mut();
        app.perspective = match app.perspective_select.value().parse() {
            Ok(0) | Err(_) => Perspective::Observer,
            Ok(id) => Perspective::Player(id),
        };
        app.update_palette();
    });
    select.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();

    app.borrow().show_controls();
}

fn wire_playback(app: &Rc<RefCell<App>>) {
    let (play, speed, timeline, live) = {
        let app = app.borrow();
        (
            app.play_button.clone(),
            app.speed_select.clone(),
            app.timeline.clone(),
            app.live_button.clone(),
        )
    };

    let target = Rc::clone(app);
    let on_play = Closure::<dyn FnMut()>::new(move || {
        let app = target.borrow();
        let command = if app.paused {
            COMMAND_PLAY
        } else {
            COMMAND_PAUSE
        };
        app.command(command, 0.0);
    });
    play.set_onclick(Some(on_play.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_play.forget();

    let target = Rc::clone(app);
    let on_speed = Closure::<dyn FnMut()>::new(move || {
        let app = target.borrow();
        if let Ok(speed) = app.speed_select.value().parse::<f64>() {
            app.command(COMMAND_SPEED, speed);
        }
    });
    speed.set_onchange(Some(on_speed.as_ref().unchecked_ref()));
    on_speed.forget();

    // While dragging only keyframes are shown; the exact turn is decoded on
    // release (init.md 3).
    let target = Rc::clone(app);
    let on_drag = Closure::<dyn FnMut()>::new(move || {
        let app = &mut *target.borrow_mut();
        app.dragging = true;
        if let Ok(turn) = app.timeline.value().parse::<f64>() {
            app.command(COMMAND_SCRUB, turn);
        }
    });
    timeline.set_oninput(Some(on_drag.as_ref().unchecked_ref()));
    on_drag.forget();

    let target = Rc::clone(app);
    let on_release = Closure::<dyn FnMut()>::new(move || {
        let app = &mut *target.borrow_mut();
        app.dragging = false;
        if let Ok(turn) = app.timeline.value().parse::<f64>() {
            app.command(COMMAND_SEEK, turn);
        }
    });
    timeline.set_onchange(Some(on_release.as_ref().unchecked_ref()));
    on_release.forget();

    let target = Rc::clone(app);
    let on_live = Closure::<dyn FnMut()>::new(move || {
        target.borrow().command(COMMAND_LIVE, 0.0);
    });
    live.set_onclick(Some(on_live.as_ref().unchecked_ref()));
    on_live.forget();

    let (current, popup_current, close) = {
        let app = app.borrow();
        (
            app.match_over.button.clone(),
            app.match_over.popup_button.clone(),
            app.match_over.close_button.clone(),
        )
    };
    let target = Rc::clone(app);
    let on_current = Closure::<dyn FnMut()>::new(move || {
        target.borrow().command(COMMAND_CURRENT_MATCH, 0.0);
    });
    current.set_onclick(Some(on_current.as_ref().unchecked_ref()));
    popup_current.set_onclick(Some(on_current.as_ref().unchecked_ref()));
    on_current.forget();

    let target = Rc::clone(app);
    let on_close = Closure::<dyn FnMut()>::new(move || {
        target.borrow_mut().match_over.close();
    });
    close.set_onclick(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();
}

fn connect(app: &Rc<RefCell<App>>) {
    let worker = app.borrow().worker.clone();
    let receiver = Rc::clone(app);
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let bytes = js_sys::Uint8Array::new(&event.data()).to_vec();
        match bytes.split_first() {
            Some((&POST_SCENE, rest)) => {
                if let Some((turn_ms, message)) = rest.split_first_chunk::<8>() {
                    receive(&receiver, f64::from_le_bytes(*turn_ms), message);
                }
            }
            Some((&POST_STATUS, rest)) => {
                let u32_at = |at: usize| {
                    rest.get(at..at + 4)
                        .and_then(|bytes| bytes.try_into().ok())
                        .map(u32::from_le_bytes)
                };
                let (Some(first), Some(newest), Some(playhead), Some(&flags), Some(speed)) =
                    (u32_at(0), u32_at(4), u32_at(8), rest.get(12), u32_at(13))
                else {
                    return;
                };
                let u64_at = |at: usize| {
                    rest.get(at..at + 8)
                        .and_then(|bytes| bytes.try_into().ok())
                        .map(u64::from_le_bytes)
                };
                if let (Some(live), Some(segments)) = (u64_at(17), u64_at(25)) {
                    let mut app = receiver.borrow_mut();
                    app.stream_bytes.live = live;
                    app.stream_bytes.segments = segments;
                }
                // The frame clock, which the popup's countdown runs on.
                let now = web_sys::window()
                    .and_then(|window| window.performance())
                    .map_or(0.0, |performance| performance.now());
                receiver
                    .borrow_mut()
                    .match_over
                    .show_status(flags, u32_at(33).unwrap_or(0), now);
                receiver.borrow_mut().show_status(
                    first,
                    newest,
                    playhead,
                    flags,
                    f32::from_bits(speed),
                );
            }
            _ => {}
        }
    });
    worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    // The worker lives as long as the page, so its handler does too.
    on_message.forget();

    let on_error = Closure::<dyn FnMut()>::new(|| {
        web_sys::console::error_1(&"stream worker failed".into());
    });
    worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();
}

/// `turn_ms` is the real time per turn when played back, 0 when live.
fn receive(app_rc: &Rc<RefCell<App>>, turn_ms: f64, message: &[u8]) {
    let app = &mut *app_rc.borrow_mut();
    match app.scene.apply(message, app.progress) {
        // The dots now start where they are drawn, so the new turn plays
        // from 0; a second turn before the next frame starts there too.
        Ok(()) => {
            app.progress = 0.0;
            app.turn_arrived = true;
            app.scene_changed = true;
            if turn_ms > 0.0 {
                app.turn_interval = turn_ms;
                app.measure_turns = false;
            } else if !app.measure_turns {
                // Back to live: start measuring from the match's speed
                // rather than from the last playback speed.
                app.measure_turns = true;
                app.turn_interval =
                    (TURN_MS / f64::from(app.scene.game_speed)).clamp(TURN_MS_MIN, TURN_MS_MAX);
            }
            app.show_clock();
            if app.scene.players_changed {
                app.scene.players_changed = false;
                app.players_changed();
            }
            let terrain_id = app.scene.terrain_id;
            if terrain_id != app.terrain_id {
                app.terrain_id = terrain_id;
                // A new match: the old picture belongs to another map.
                app.renderer.set_terrain(None);
                if let Some(minimap) = &mut app.minimap {
                    minimap.set_terrain(None);
                }
                if terrain_id != 0 {
                    let app = Rc::clone(app_rc);
                    wasm_bindgen_futures::spawn_local(load_terrain(app, terrain_id));
                }
            }
        }
        // The scene may be half-patched now and every later message would
        // build on it, so stop rather than show a wrong picture.
        Err(error) => {
            web_sys::console::error_1(&format!("bad stream message: {error}").into());
            app.worker.terminate();
        }
    }
}

async fn load_terrain(app: Rc<RefCell<App>>, id: u64) {
    match fetch_terrain(id).await {
        Ok((image, size)) => {
            let mut app = app.borrow_mut();
            app.stream_bytes.terrain += size;
            // Another match may have started during the download.
            if app.terrain_id == id {
                app.renderer.set_terrain(Some(&image));
                if let Some(minimap) = &mut app.minimap {
                    minimap.set_terrain(Some(&image));
                }
                app.dirty = true;
            }
        }
        // Not fatal: the dots still work on a plain background.
        Err(error) => {
            web_sys::console::error_1(&format!("terrain {id:016x}: {error}").into());
        }
    }
}

async fn load_icons(app: Rc<RefCell<App>>) {
    let atlas = fetch_bytes("/icons.bin")
        .await
        .and_then(|bytes| icons::decode(&bytes).map_err(String::from));
    match atlas {
        Ok(atlas) => {
            let mut app = app.borrow_mut();
            match &mut app.minimap {
                Some(minimap) => minimap.set_icon_atlas(atlas),
                None => app.renderer.set_icon_atlas(atlas),
            }
            app.scene_changed = true;
        }
        // Not fatal: every entity still has its dot.
        Err(error) => web_sys::console::error_1(&format!("icons: {error}").into()),
    }
}

async fn fetch_terrain(id: u64) -> Result<(TerrainImage, u64), String> {
    let bytes = fetch_bytes(&format!("/terrain/{id:016x}")).await?;
    let image = terrain::build(&bytes).map_err(String::from)?;
    Ok((image, bytes.len() as u64))
}

/// Bytes a finished fetch of `url`, a path on this origin, moved over the
/// network and its body's size, from its Resource Timing entry; a body the
/// HTTP cache served moves at most the headers of a revalidation.
fn transfer_sizes(url: &str) -> Option<(u64, u64)> {
    let window = web_sys::window()?;
    let performance = window.performance()?;
    let origin = window.location().origin().ok()?;
    let entries = performance.get_entries_by_name(&format!("{origin}{url}"));
    let entry = entries.get(entries.length().checked_sub(1)?);
    let read = |name: &str| js_sys::Reflect::get(&entry, &name.into()).ok()?.as_f64();
    Some((
        read("transferSize")? as u64,
        read("encodedBodySize")? as u64,
    ))
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let response: Response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|_| "request failed")?
        .dyn_into()
        .map_err(|_| "not a response")?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let body = response.array_buffer().map_err(|_| "no body")?;
    let buffer = JsFuture::from(body).await.map_err(|_| "body failed")?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

fn animate(app: Rc<RefCell<App>>) {
    let callback: Rc<RefCell<Option<FrameCallback>>> = Rc::new(RefCell::new(None));
    let next = Rc::clone(&callback);
    // The closure holds an Rc to itself, which keeps the loop alive for the
    // lifetime of the page.
    *callback.borrow_mut() = Some(Closure::new(move |now: f64| {
        app.borrow_mut().frame(now);
        request_frame(next.borrow().as_ref().unwrap());
    }));
    request_frame(callback.borrow().as_ref().unwrap());
}

fn request_frame(callback: &FrameCallback) {
    if let Some(window) = web_sys::window() {
        let _ = window.request_animation_frame(callback.as_ref().unchecked_ref());
    }
}

/// The canvas's CSS size in device pixels, clamped to what the GPU accepts.
fn canvas_size(canvas: &HtmlCanvasElement, max: u32) -> (u32, u32, f32) {
    let ratio = web_sys::window().map_or(1.0, |window| window.device_pixel_ratio());
    let scale = |css: i32| ((f64::from(css) * ratio).round() as u32).clamp(1, max);
    (
        scale(canvas.client_width()),
        scale(canvas.client_height()),
        ratio as f32,
    )
}

/// Game time, `m:ss` or `h:mm:ss`.
fn format_clock(turn: u32) -> String {
    let seconds = (f64::from(turn) * TURN_MS / 1000.0) as u64;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn log(message: &str) {
    web_sys::console::log_1(&message.into());
}
