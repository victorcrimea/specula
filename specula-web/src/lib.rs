mod icons;
mod render;
mod stream;
mod terrain;

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen_futures::JsFuture;
use web_sys::BinaryType;
use web_sys::HtmlCanvasElement;
use web_sys::MessageEvent;
use web_sys::Response;
use web_sys::WebSocket;

use crate::render::Renderer;
use crate::stream::Scene;
use crate::terrain::TerrainImage;

type FrameCallback = Closure<dyn FnMut(f64)>;

/// A multiplayer turn is 200 ms of simulation time (DEFAULT_TURN_LENGTH);
/// the real interval depends on game speed and is measured.
const TURN_MS: f64 = 200.0;
const TURN_MS_MIN: f64 = 50.0;
const TURN_MS_MAX: f64 = 1000.0;
/// Low enough that network jitter does not make the pace wobble.
const TURN_SMOOTHING: f64 = 0.1;

struct App {
    canvas: HtmlCanvasElement,
    renderer: Renderer,
    scene: Scene,
    /// How far the last drawn frame moved the dots towards the current turn.
    progress: f32,
    /// Frame timestamp at which the current turn started to play.
    turn_start: f64,
    turn_interval: f64,
    turn_arrived: bool,
    scene_changed: bool,
    dirty: bool,
    /// The terrain shown or being fetched; 0 for none.
    terrain_id: u64,
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
            if gap <= TURN_MS_MAX {
                self.turn_interval += (gap - self.turn_interval) * TURN_SMOOTHING;
                self.turn_interval = self.turn_interval.clamp(TURN_MS_MIN, TURN_MS_MAX);
            }
            self.turn_start = now;
        }
        if self.scene_changed {
            self.scene_changed = false;
            self.renderer.set_scene(&self.scene);
            self.dirty = true;
        }
        let progress = ((now - self.turn_start) / self.turn_interval).clamp(0.0, 1.0) as f32;
        if self.dirty || progress != self.progress {
            self.dirty = !self.renderer.draw(progress);
            if !self.dirty {
                self.progress = progress;
            }
        }
    }
}

#[wasm_bindgen(start)]
pub fn start() {
    // Without this a panic shows up only as "unreachable" in the console.
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&info.to_string().into());
    }));
    wasm_bindgen_futures::spawn_local(async {
        if let Err(error) = run().await {
            web_sys::console::error_1(&error.into());
        }
    });
}

async fn run() -> Result<(), String> {
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
    let renderer = Renderer::new(canvas.clone(), ratio).await?;

    let app = Rc::new(RefCell::new(App {
        canvas,
        renderer,
        scene: Scene::default(),
        progress: 1.0,
        turn_start: 0.0,
        turn_interval: TURN_MS,
        turn_arrived: false,
        scene_changed: false,
        dirty: true,
        terrain_id: 0,
    }));
    connect(&window, &app)?;
    wasm_bindgen_futures::spawn_local(load_icons(Rc::clone(&app)));
    animate(app);
    Ok(())
}

fn connect(window: &web_sys::Window, app: &Rc<RefCell<App>>) -> Result<(), String> {
    let location = window.location();
    let scheme = match location.protocol().as_deref() {
        Ok("https:") => "wss",
        _ => "ws",
    };
    let host = location.host().map_err(|_| "no location host")?;
    let url = format!("{scheme}://{host}/ws");
    let socket = WebSocket::new(&url).map_err(|_| format!("cannot open {url}"))?;
    socket.set_binary_type(BinaryType::Arraybuffer);

    let receiver = Rc::clone(app);
    let closer = socket.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() else {
            return;
        };
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        let app = &mut *receiver.borrow_mut();
        match app.scene.apply(&bytes, app.progress) {
            // The dots now start where they are drawn, so the new turn plays
            // from 0; a second turn before the next frame starts there too.
            Ok(()) => {
                app.progress = 0.0;
                app.turn_arrived = true;
                app.scene_changed = true;
                let terrain_id = app.scene.terrain_id;
                if terrain_id != app.terrain_id {
                    app.terrain_id = terrain_id;
                    // A new match: the old picture belongs to another map.
                    app.renderer.set_terrain(None);
                    if terrain_id != 0 {
                        let app = Rc::clone(&receiver);
                        wasm_bindgen_futures::spawn_local(load_terrain(app, terrain_id));
                    }
                }
            }
            // The scene may be half-patched now and every later delta would
            // build on it, so stop rather than show a wrong picture.
            Err(error) => {
                web_sys::console::error_1(&format!("bad stream message: {error}").into());
                let _ = closer.close();
            }
        }
    });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    // The socket lives as long as the page, so its handler does too.
    on_message.forget();

    let on_close = Closure::<dyn FnMut()>::new(|| log("stream closed"));
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();
    Ok(())
}

async fn load_terrain(app: Rc<RefCell<App>>, id: u64) {
    match fetch_terrain(id).await {
        Ok(image) => {
            let mut app = app.borrow_mut();
            // Another match may have started during the download.
            if app.terrain_id == id {
                app.renderer.set_terrain(Some(&image));
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
            app.renderer.set_icon_atlas(atlas);
            app.scene_changed = true;
        }
        // Not fatal: every entity still has its dot.
        Err(error) => web_sys::console::error_1(&format!("icons: {error}").into()),
    }
}

async fn fetch_terrain(id: u64) -> Result<TerrainImage, String> {
    let bytes = fetch_bytes(&format!("/terrain/{id:016x}")).await?;
    terrain::build(&bytes).map_err(String::from)
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

fn log(message: &str) {
    web_sys::console::log_1(&message.into());
}
