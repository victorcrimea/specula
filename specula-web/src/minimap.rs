//! The observer page's minimap: the map page's picture in a corner, with
//! the ground the 3D camera sees outlined on it, and a click or drag on it
//! moving the camera there, as the game's `CMiniMap`.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::HtmlCanvasElement;
use web_sys::MouseEvent;

use crate::diplomacy::Perspective;
use crate::icons::IconAtlas;
use crate::render::Renderer;
use crate::stream::Scene;
use crate::terrain::TerrainImage;

/// The flare presses: the map point and whether it was the left button.
type FlarePresses = Rc<RefCell<Vec<([f32; 2], bool)>>>;

pub struct Minimap {
    canvas: HtmlCanvasElement,
    renderer: Renderer,
    /// A map point clicked or dragged to, in metres, not yet handed on.
    target: Rc<Cell<Option<[f32; 2]>>>,
    /// In metres; read by the pointer handlers too.
    map_size: Rc<Cell<f32>>,
    /// Flares are armed: a press flares instead of moving the camera.
    flare_armed: Rc<Cell<bool>>,
    /// The presses while armed: the map point and whether it was the left
    /// button.
    flare_presses: FlarePresses,
    /// The canvas's `data-cursor`, empty for none.
    cursor: String,
    circular: bool,
    outline: Option<[[f32; 2]; 4]>,
    progress: f32,
    dirty: bool,
}

impl Minimap {
    pub async fn new(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let (width, height, ratio) = crate::canvas_size(&canvas, 2048);
        canvas.set_width(width);
        canvas.set_height(height);
        let renderer = Renderer::new(canvas.clone(), ratio).await?;
        let target = Rc::new(Cell::new(None));
        let map_size = Rc::new(Cell::new(0.0));
        let flare_armed = Rc::new(Cell::new(false));
        let flare_presses = Rc::new(RefCell::new(Vec::new()));
        wire_pointer(&canvas, &target, &map_size, &flare_armed, &flare_presses);
        Ok(Minimap {
            canvas,
            renderer,
            target,
            map_size,
            flare_armed,
            flare_presses,
            cursor: String::new(),
            circular: false,
            outline: None,
            progress: 1.0,
            dirty: true,
        })
    }

    /// The map point to move the camera to, once per click or drag step.
    pub fn take_target(&self) -> Option<[f32; 2]> {
        self.target.take()
    }

    pub fn set_flare_armed(&self, armed: bool) {
        self.flare_armed.set(armed);
    }

    /// The presses since the last call while flares are armed: the map
    /// point, and whether it was the left button.
    pub fn take_flare_presses(&self) -> Vec<([f32; 2], bool)> {
        std::mem::take(&mut *self.flare_presses.borrow_mut())
    }

    pub fn set_cursor(&mut self, cursor: Option<&str>) {
        let cursor = cursor.unwrap_or_default();
        if cursor == self.cursor {
            return;
        }
        let _ = if cursor.is_empty() {
            self.canvas.remove_attribute("data-cursor")
        } else {
            self.canvas.set_attribute("data-cursor", cursor)
        };
        self.cursor = cursor.to_string();
    }

    pub fn set_scene(&mut self, scene: &Scene, palette: &[u32]) {
        self.map_size.set(scene.map_size as f32);
        self.renderer.set_scene(scene, palette);
        self.dirty = true;
    }

    pub fn set_perspective(&mut self, perspective: Perspective) {
        self.renderer.set_perspective(perspective);
        self.dirty = true;
    }

    pub fn set_terrain(&mut self, image: Option<&TerrainImage>) {
        self.circular = image.is_some_and(|image| image.circular);
        self.renderer.set_terrain(image);
        self.dirty = true;
    }

    pub fn set_icon_atlas(&mut self, atlas: IconAtlas) {
        self.renderer.set_icon_atlas(atlas);
    }

    /// `outline` is the camera's view on the ground, as
    /// `View::view_outline`.
    pub fn frame(&mut self, progress: f32, outline: Option<[[f32; 2]; 4]>) {
        let (width, height, ratio) = crate::canvas_size(&self.canvas, self.renderer.max_size());
        if width != self.canvas.width() || height != self.canvas.height() {
            self.canvas.set_width(width);
            self.canvas.set_height(height);
            self.renderer.resize(width, height, ratio);
            self.dirty = true;
        }
        if outline != self.outline {
            self.outline = outline;
            let segments = match outline {
                None => Vec::new(),
                // Off a square map the canvas edge crops it.
                Some(points) if !self.circular => (0..4)
                    .map(|index| {
                        let (from, to) = (points[index], points[(index + 1) % 4]);
                        [from[0], from[1], to[0], to[1]]
                    })
                    .collect(),
                Some(points) => {
                    let half = self.map_size.get() / 2.0;
                    crop_by_circle(&points, [half, half], half)
                }
            };
            self.renderer.set_outline(&segments);
            self.dirty = true;
        }
        if self.dirty || progress != self.progress {
            self.dirty = !self.renderer.draw(progress);
            if !self.dirty {
                self.progress = progress;
            }
        }
    }
}

/// `CropPointsByCircle` from CMiniMap.cpp: the parts of the closed
/// polygon's edges inside the circle, as segments.
fn crop_by_circle(points: &[[f32; 2]; 4], center: [f32; 2], radius: f32) -> Vec<[f32; 4]> {
    const EPS: f32 = 1e-3;
    let dot = |a: [f32; 2], b: [f32; 2]| a[0] * b[0] + a[1] * b[1];
    let mut lines = Vec::with_capacity(points.len());
    for (index, &current) in points.iter().enumerate() {
        let next = points[(index + 1) % points.len()];
        let delta = [next[0] - current[0], next[1] - current[1]];
        let length = delta[0].hypot(delta[1]);
        if length <= 0.0 {
            continue;
        }
        let direction = [delta[0] / length, delta[1] / length];
        let normal = [direction[1], -direction[0]];
        let offset = dot(normal, current) - dot(normal, center);
        if offset.abs() + EPS >= radius {
            continue;
        }
        let closest = [
            center[0] + normal[0] * offset,
            center[1] + normal[1] * offset,
        ];
        let half_chord = (radius * radius - offset * offset).sqrt();
        let a = [
            closest[0] - direction[0] * half_chord,
            closest[1] - direction[1] * half_chord,
        ];
        let b = [
            closest[0] + direction[0] * half_chord,
            closest[1] + direction[1] * half_chord,
        ];
        if dot(direction, current) + EPS > dot(direction, b)
            || dot(direction, next) - EPS < dot(direction, a)
        {
            continue;
        }
        let from = if dot(direction, current) > dot(direction, a) {
            current
        } else {
            a
        };
        let to = if dot(direction, next) < dot(direction, b) {
            next
        } else {
            b
        };
        lines.push([from[0], from[1], to[0], to[1]]);
    }
    lines
}

/// The primary button or a finger moves the camera on press and while
/// dragged, as `CMiniMap::HandleMessage`; while flares are armed a press is
/// a flare instead. The handlers live as long as the page.
fn wire_pointer(
    canvas: &HtmlCanvasElement,
    target: &Rc<Cell<Option<[f32; 2]>>>,
    map_size: &Rc<Cell<f32>>,
    flare_armed: &Rc<Cell<bool>>,
    flare_presses: &FlarePresses,
) {
    let held: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
    let point = {
        let canvas = canvas.clone();
        let map_size = Rc::clone(map_size);
        move |event: &MouseEvent| -> Option<[f32; 2]> {
            let size = map_size.get();
            if size <= 0.0 {
                return None;
            }
            let (width, height) = (
                canvas.client_width().max(1) as f32,
                canvas.client_height().max(1) as f32,
            );
            // Undoes the dots' letterbox.
            let side = width.min(height);
            let x = ((event.offset_x() as f32 / width * 2.0 - 1.0) * width / side + 1.0) / 2.0;
            let z = ((1.0 - event.offset_y() as f32 / height * 2.0) * height / side + 1.0) / 2.0;
            Some([x.clamp(0.0, 1.0) * size, z.clamp(0.0, 1.0) * size])
        }
    };
    let listen = |name: &str, handler: Closure<dyn FnMut(MouseEvent)>| {
        let _ = canvas.add_event_listener_with_callback(name, handler.as_ref().unchecked_ref());
        handler.forget();
    };

    let (down_held, down_target, down_point) = (Rc::clone(&held), Rc::clone(target), point.clone());
    let down_canvas = canvas.clone();
    let down_armed = Rc::clone(flare_armed);
    let down_presses = Rc::clone(flare_presses);
    listen(
        "pointerdown",
        Closure::new(move |event: MouseEvent| {
            if down_armed.get() {
                let button = event.button();
                if button == 0 || button == 2 {
                    event.prevent_default();
                    if let Some(point) = down_point(&event) {
                        down_presses.borrow_mut().push((point, button == 0));
                    }
                }
                return;
            }
            if event.button() != 0 {
                return;
            }
            event.prevent_default();
            let id = crate::field::pointer_id(&event);
            // Keeps the drag ours when it leaves the minimap.
            let _ = down_canvas.set_pointer_capture(id);
            down_held.set(Some(id));
            if let Some(point) = down_point(&event) {
                down_target.set(Some(point));
            }
        }),
    );

    let (move_held, move_target) = (Rc::clone(&held), Rc::clone(target));
    listen(
        "pointermove",
        Closure::new(move |event: MouseEvent| {
            if move_held.get() != Some(crate::field::pointer_id(&event)) {
                return;
            }
            if let Some(point) = point(&event) {
                move_target.set(Some(point));
            }
        }),
    );

    for name in ["pointerup", "pointercancel"] {
        let held = Rc::clone(&held);
        listen(
            name,
            Closure::new(move |event: MouseEvent| {
                if held.get() == Some(crate::field::pointer_id(&event)) {
                    held.set(None);
                }
            }),
        );
    }

    listen(
        "contextmenu",
        Closure::new(|event: MouseEvent| event.prevent_default()),
    );
}
