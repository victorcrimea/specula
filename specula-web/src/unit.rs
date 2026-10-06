//! The unit page: one actor, baked by specula-tools' bake-unit into
//! `/unit-<hash>.bin` (format v1, documented there) and named by
//! `/unit.txt`. Ten thousand copies march about in squads (march.rs) on a
//! flat ground under an orbit camera; each frame their places and animation
//! frames go to
//! the GPU as one instance buffer. Units outside the camera's view are left
//! out; the rest are sorted by their size on screen into the baked levels of
//! detail and, smallest, impostors (impostor.rs), each group one instanced
//! draw per part.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::HtmlCanvasElement;
use web_sys::HtmlElement;
use web_sys::MouseEvent;
use web_sys::WheelEvent;

use crate::FrameCallback;
use crate::gpu;
use crate::gpu::Gpu;
use crate::impostor;
use crate::impostor::Atlas;
use crate::impostor::Kit;
use crate::march;
use crate::march::March;

const VERSION: u8 = 1;
/// View and projection, sun, player colour, VAT layout, misc, then the
/// camera and atlas fields impostor.wgsl reads.
pub const VIEW_SIZE: u64 = 176;
/// f32x4 x, z, heading sine and cosine, f32x4 frame a, frame b, blend, 0;
/// impostors have their atlas cell, columns and rows in the second.
pub const INSTANCE_SIZE: u64 = 32;
const UV_ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4];
/// Bounding sphere diameters on screen, in pixels, from which each baked
/// level of detail is used: the full mesh, then the coarser ones; below the
/// last, an impostor.
const LEVEL_PIXELS: [f32; 3] = [200.0, 90.0, impostor::CELL as f32 * 0.6];
/// Part flags and alpha reference, padded to 16 bytes for WebGL2.
const PART_SIZE: u64 = 16;
const FLAG_PLAYER_COLOR: u8 = 1;
const BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.06,
    g: 0.07,
    b: 0.08,
    a: 1.0,
};
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
/// Player 1's colour, 21 55 149 in simulation/data/settings/player_defaults.json.
const PLAYER_COLOR: [f32; 3] = [21.0 / 255.0, 55.0 / 255.0, 149.0 / 255.0];
const GROUND_HALF_SIZE: f32 = 220.0;
/// The animations the gaits of march.rs play, by `march::IDLE` and
/// `march::WALK`.
const GAITS: [&str; 2] = ["Idle", "Walk"];
const LEVELS: usize = LEVEL_PIXELS.len() + 1;
const AMBIENT: f32 = 0.45;
const SUN_STRENGTH: f32 = 0.75;
/// The point the camera orbits, about chest height on infantry.
const TARGET: [f32; 3] = [0.0, 2.2, 0.0];
const FIELD_OF_VIEW: f32 = 0.7;
const NEAR: f32 = 0.5;
const FAR: f32 = 2000.0;
const MIN_DISTANCE: f32 = 2.0;
const MAX_DISTANCE: f32 = 900.0;
/// How often the frame rate shown is recomputed; shorter flickers.
const FPS_INTERVAL_MS: f64 = 500.0;
/// Radians per CSS pixel of drag.
const DRAG_SPEED: f32 = 0.008;

struct UnitFile {
    vertex_count: u32,
    texture_width: u32,
    min: [f32; 3],
    max: [f32; 3],
    uvs: Vec<u8>,
    indices: Vec<u8>,
    parts: Vec<PartData>,
    animations: Vec<AnimationData>,
}

struct PartData {
    /// First index and count of each level of detail, the full mesh first.
    levels: Vec<(u32, u32)>,
    flags: u8,
    alpha_ref: f32,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

struct AnimationData {
    name: String,
    frame_ms: f32,
    frames: u32,
    rows: u32,
    positions: Vec<u8>,
    normals: Vec<u8>,
}

struct Animation {
    frame_ms: f32,
    frames: u32,
    bind_group: wgpu::BindGroup,
    atlas: Atlas,
}

pub struct Part {
    /// First index and count of each level of detail, the full mesh first.
    pub levels: Vec<(u32, u32)>,
    pub bind_group: wgpu::BindGroup,
}

/// A sphere around one unit in every frame and heading, centred above its
/// feet; enough to tell when it is surely out of view.
pub struct Reach {
    pub centre_y: f32,
    pub radius: f32,
}

/// What goes into the `View` uniform of unit.wgsl and impostor.wgsl.
pub struct ViewParams<'a> {
    /// Row-major, for column vectors.
    pub view_proj: [[f32; 4]; 4],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub eye: [f32; 3],
    pub vertex_count: u32,
    pub texture_width: u32,
    pub reach: &'a Reach,
}

impl Reach {
    /// The bounds cover every baked frame; a unit turns about y, so the
    /// horizontal reach is the farthest bounds corner from the y axis.
    fn new(min: [f32; 3], max: [f32; 3]) -> Self {
        let horizontal = [min[0], max[0]]
            .iter()
            .flat_map(|x| [min[2], max[2]].map(|z| (x * x + z * z).sqrt()))
            .fold(0.0f32, f32::max);
        let half_height = (max[1] - min[1]) / 2.0;
        Reach {
            centre_y: (min[1] + max[1]) / 2.0,
            radius: (horizontal * horizontal + half_height * half_height).sqrt(),
        }
    }
}

struct Viewer {
    canvas: HtmlCanvasElement,
    fps: HtmlElement,
    /// Frames drawn since `fps_since`, in page milliseconds.
    fps_frames: u32,
    fps_since: f64,
    gpu: Gpu,
    unit_pipeline: wgpu::RenderPipeline,
    ground_pipeline: wgpu::RenderPipeline,
    view_buffer: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    depth: wgpu::TextureView,
    uvs: wgpu::Buffer,
    indices: wgpu::Buffer,
    parts: Vec<Part>,
    /// By gait.
    animations: [Animation; 2],
    march: March,
    reach: Reach,
    kit: Kit,
    /// Instances this frame, by gait and then level of detail, impostors
    /// last.
    groups: [[Vec<u8>; LEVELS]; 2],
    instances: wgpu::Buffer,
    instance_bytes: Vec<u8>,
    last_frame: Option<f64>,
    vertex_count: u32,
    texture_width: u32,
    yaw: f32,
    pitch: f32,
    distance: f32,
    /// The pointer position of an ongoing drag.
    drag: Option<(i32, i32)>,
}

pub async fn run() -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let canvas: HtmlCanvasElement = crate::element(&document, "view")?;
    let fps: HtmlElement = crate::element(&document, "fps")?;
    let (width, height, _) = crate::canvas_size(&canvas, 2048);
    canvas.set_width(width);
    canvas.set_height(height);

    let name = unit_name().await?;
    let bytes = crate::fetch_bytes(&format!("/{name}"))
        .await
        .map_err(|error| format!("/{name}: {error}"))?;
    let file = decode(&bytes).map_err(|error| format!("/{name}: {error}"))?;
    crate::log(&format!(
        "unit: {} vertices, {} parts, bounds {:?} to {:?}",
        file.vertex_count,
        file.parts.len(),
        file.min,
        file.max
    ));

    let gpu = gpu::init(canvas.clone()).await?;
    let viewer = Viewer::new(canvas, fps, gpu, &file)?;
    let viewer = Rc::new(RefCell::new(viewer));
    wire(&viewer)?;
    animate(viewer);
    Ok(())
}

/// The current bake's file name, from the pointer bake-unit writes.
async fn unit_name() -> Result<String, String> {
    let bytes = crate::fetch_bytes("/unit.txt")
        .await
        .map_err(|error| format!("/unit.txt: {error}"))?;
    let name = std::str::from_utf8(&bytes)
        .map_err(|_| "/unit.txt is not UTF-8")?
        .trim();
    // The name goes into a URL; anything else is not bake-unit's.
    let valid = name.starts_with("unit-")
        && name.ends_with(".bin")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
    if !valid {
        return Err(format!("/unit.txt names {name:?}, not a unit file"));
    }
    Ok(name.to_string())
}

fn wire(viewer: &Rc<RefCell<Viewer>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let canvas = viewer.borrow().canvas.clone();

    // The handlers live as long as the page, so they are leaked on purpose.
    let target = Rc::clone(viewer);
    let on_down = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        target.borrow_mut().drag = Some((event.client_x(), event.client_y()));
    });
    canvas.set_onmousedown(Some(on_down.as_ref().unchecked_ref()));
    on_down.forget();

    // On the window, so a drag that leaves the canvas still ends.
    let target = Rc::clone(viewer);
    let on_move = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        let viewer = &mut *target.borrow_mut();
        let Some((x, y)) = viewer.drag else {
            return;
        };
        let (dx, dy) = (event.client_x() - x, event.client_y() - y);
        viewer.yaw -= dx as f32 * DRAG_SPEED;
        viewer.pitch = (viewer.pitch + dy as f32 * DRAG_SPEED).clamp(-0.1, 1.5);
        viewer.drag = Some((event.client_x(), event.client_y()));
    });
    window.set_onmousemove(Some(on_move.as_ref().unchecked_ref()));
    on_move.forget();

    let target = Rc::clone(viewer);
    let on_up = Closure::<dyn FnMut(MouseEvent)>::new(move |_: MouseEvent| {
        target.borrow_mut().drag = None;
    });
    window.set_onmouseup(Some(on_up.as_ref().unchecked_ref()));
    on_up.forget();

    let target = Rc::clone(viewer);
    let on_wheel = Closure::<dyn FnMut(WheelEvent)>::new(move |event: WheelEvent| {
        let viewer = &mut *target.borrow_mut();
        let factor = (event.delta_y() * 0.001).exp() as f32;
        viewer.distance = (viewer.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
    });
    canvas.set_onwheel(Some(on_wheel.as_ref().unchecked_ref()));
    on_wheel.forget();
    Ok(())
}

fn animate(viewer: Rc<RefCell<Viewer>>) {
    let callback: Rc<RefCell<Option<FrameCallback>>> = Rc::new(RefCell::new(None));
    let next = Rc::clone(&callback);
    // The closure holds an Rc to itself, which keeps the loop alive for the
    // lifetime of the page.
    *callback.borrow_mut() = Some(Closure::new(move |now: f64| {
        viewer.borrow_mut().frame(now);
        crate::request_frame(next.borrow().as_ref().unwrap());
    }));
    crate::request_frame(callback.borrow().as_ref().unwrap());
}

fn now() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
}

impl Viewer {
    fn new(
        canvas: HtmlCanvasElement,
        fps: HtmlElement,
        gpu: Gpu,
        file: &UnitFile,
    ) -> Result<Self, String> {
        let device = &gpu.device;
        let queue = &gpu.queue;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("unit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("unit.wgsl").into()),
        });

        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("unit view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        // Read with textureLoad, so they need no filtering.
        let vat_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let animation_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("unit animation"),
            entries: &[vat_entry(0), vat_entry(1)],
        });
        let part_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("unit part"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unit view"),
            size: VIEW_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("unit view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });

        let format = gpu.config.format;
        let depth_state = wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        };
        let depth_stencil = Some(depth_state.clone());
        let unit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("unit"),
            bind_group_layouts: &[
                Some(&view_layout),
                Some(&animation_layout),
                Some(&part_layout),
            ],
            immediate_size: 0,
        });
        let unit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("unit"),
            layout: Some(&unit_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_unit"),
                compilation_options: Default::default(),
                buffers: &unit_vertex_buffers(),
            },
            // No culling until the winding is checked against the engine's;
            // the depth test hides back faces of closed meshes anyway.
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: depth_stencil.clone(),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_unit"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let ground_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ground"),
            bind_group_layouts: &[Some(&view_layout)],
            immediate_size: 0,
        });
        let ground_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ground"),
            layout: Some(&ground_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_ground"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: depth_stencil.clone(),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_ground"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let uvs = create_buffer(
            device,
            queue,
            "unit uvs",
            &file.uvs,
            wgpu::BufferUsages::VERTEX,
        );
        let indices = create_buffer(
            device,
            queue,
            "unit indices",
            &file.indices,
            wgpu::BufferUsages::INDEX,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("unit skin"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let mut parts = Vec::new();
        for data in &file.parts {
            // Not sRGB: the engine does no sRGB conversion either.
            let view = create_skin(device, queue, data.width, data.height, &data.rgba);
            let mut uniform = Vec::with_capacity(PART_SIZE as usize);
            let player_color = if data.flags & FLAG_PLAYER_COLOR != 0 {
                1.0f32
            } else {
                0.0
            };
            for value in [player_color, data.alpha_ref, 0.0, 0.0] {
                uniform.extend_from_slice(&value.to_le_bytes());
            }
            let buffer = create_buffer(
                device,
                queue,
                "unit part",
                &uniform,
                wgpu::BufferUsages::UNIFORM,
            );
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("unit part"),
                layout: &part_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            });
            parts.push(Part {
                levels: data.levels.clone(),
                bind_group,
            });
        }

        let kit = Kit::new(
            device,
            format,
            &shader,
            &view_layout,
            &animation_layout,
            &part_layout,
            depth_state,
        );
        let reach = Reach::new(file.min, file.max);
        let mut animations = Vec::new();
        for gait in GAITS {
            let data = file
                .animations
                .iter()
                .find(|animation| animation.name == gait)
                .ok_or_else(|| format!("the unit has no {gait} animation"))?;
            let texture = |bytes: &[u8]| {
                create_texture(
                    device,
                    queue,
                    file.texture_width,
                    data.rows,
                    wgpu::TextureFormat::Rgba16Float,
                    bytes,
                )
            };
            let positions = texture(&data.positions);
            let normals = texture(&data.normals);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("unit animation"),
                layout: &animation_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&positions),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&normals),
                    },
                ],
            });
            let scene = impostor::Scene {
                uvs: &uvs,
                indices: &indices,
                parts: &parts,
                reach: &reach,
                vertex_count: file.vertex_count,
                texture_width: file.texture_width,
            };
            let atlas = kit.bake(device, queue, &scene, &bind_group, data.frames);
            animations.push(Animation {
                frame_ms: data.frame_ms,
                frames: data.frames,
                bind_group,
                atlas,
            });
        }

        let animations: [Animation; 2] = animations
            .try_into()
            .map_err(|_| "one animation per gait")?;
        let march = March::new(timing(&animations));
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unit instances"),
            size: INSTANCE_SIZE * march.units.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let depth = create_depth(device, gpu.config.width, gpu.config.height);
        Ok(Viewer {
            canvas,
            fps,
            fps_frames: 0,
            fps_since: now(),
            gpu,
            unit_pipeline,
            ground_pipeline,
            view_buffer,
            view_bind_group,
            depth,
            uvs,
            indices,
            parts,
            animations,
            march,
            reach,
            kit,
            groups: Default::default(),
            instances,
            instance_bytes: Vec::new(),
            last_frame: None,
            vertex_count: file.vertex_count,
            texture_width: file.texture_width,
            yaw: std::f32::consts::PI - 0.6,
            pitch: impostor::ELEVATION,
            distance: 330.0,
            drag: None,
        })
    }

    fn frame(&mut self, now: f64) {
        let max = self.gpu.device.limits().max_texture_dimension_2d;
        let (width, height, _) = crate::canvas_size(&self.canvas, max);
        if width != self.gpu.config.width || height != self.gpu.config.height {
            self.canvas.set_width(width);
            self.canvas.set_height(height);
            self.gpu.config.width = width;
            self.gpu.config.height = height;
            self.gpu
                .surface
                .configure(&self.gpu.device, &self.gpu.config);
            self.depth = create_depth(&self.gpu.device, width, height);
        }

        let frame = match self.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.gpu
                    .surface
                    .configure(&self.gpu.device, &self.gpu.config);
                return;
            }
            _ => return,
        };

        let elapsed = self.last_frame.map_or(0.0, |last| (now - last).max(0.0));
        self.last_frame = Some(now);
        self.march
            .update(elapsed / 1000.0, timing(&self.animations));
        let aspect = width as f32 / height as f32;
        let (view_proj, right, up, eye) = self.camera(aspect);
        let planes = frustum_planes(&view_proj);
        // Sphere diameter on screen = radius * this / view depth.
        let pixel_scale = height as f32 / (FIELD_OF_VIEW / 2.0).tan();
        for group in self.groups.iter_mut().flatten() {
            group.clear();
        }
        for unit in &self.march.units {
            let (sin, cos) = unit.heading.sin_cos();
            // Models face -z (CCmpPosition turns them by rotY + PI); this
            // turns -z onto the direction of travel, (sin, cos).
            let place = [unit.x as f32, unit.z as f32, -sin as f32, -cos as f32];
            let centre = [place[0], self.reach.centre_y, place[1]];
            let outside = planes.iter().any(|plane| {
                plane[0] * centre[0] + plane[1] * centre[1] + plane[2] * centre[2] + plane[3]
                    < -self.reach.radius
            });
            if outside {
                continue;
            }
            let animation = &self.animations[unit.gait];
            let position = unit.frame;
            let depth = view_proj[3][0] * centre[0]
                + view_proj[3][1] * centre[1]
                + view_proj[3][2] * centre[2]
                + view_proj[3][3];
            let diameter = self.reach.radius * pixel_scale / depth.max(NEAR);
            let level = LEVEL_PIXELS
                .iter()
                .position(|pixels| diameter >= *pixels)
                .unwrap_or(LEVEL_PIXELS.len());
            let frame_data = if level < LEVEL_PIXELS.len() {
                // Looping, as the engine plays Idle and Walk: the last
                // frame blends back into the first.
                let a = position.floor() as u32 % animation.frames.max(1);
                let b = (a + 1) % animation.frames.max(1);
                [a as f32, b as f32, position.fract() as f32, 0.0]
            } else {
                // The direction to the camera, turned back into the unit's
                // own frame (the inverse of unit.wgsl's turn).
                let (dx, dz) = (eye[0] - centre[0], eye[2] - centre[2]);
                let local_x = dx * place[3] - dz * place[2];
                let local_z = dx * place[2] + dz * place[3];
                let yaw = f64::from(local_x).atan2(f64::from(local_z));
                let [columns, rows] = animation.atlas.grid();
                [
                    animation.atlas.cell(position, yaw) as f32,
                    columns,
                    rows,
                    0.0,
                ]
            };
            let group = &mut self.groups[unit.gait][level];
            for value in place.into_iter().chain(frame_data) {
                group.extend_from_slice(&value.to_le_bytes());
            }
        }
        self.instance_bytes.clear();
        for group in self.groups.iter().flatten() {
            self.instance_bytes.extend_from_slice(group);
        }
        if !self.instance_bytes.is_empty() {
            self.gpu
                .queue
                .write_buffer(&self.instances, 0, &self.instance_bytes);
        }
        let bytes = view_bytes(&ViewParams {
            view_proj,
            right,
            up,
            eye,
            vertex_count: self.vertex_count,
            texture_width: self.texture_width,
            reach: &self.reach,
        });
        self.gpu.queue.write_buffer(&self.view_buffer, 0, &bytes);

        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("unit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BACKGROUND),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.view_bind_group, &[]);
            pass.set_pipeline(&self.ground_pipeline);
            pass.draw(0..4, 0..1);
            // Each group is drawn from its own slice of the buffer: WebGL2
            // has no first-instance offset.
            let mut offset = 0;
            for (animation, groups) in self.animations.iter().zip(&self.groups) {
                for (level, group) in groups.iter().enumerate() {
                    let count = (group.len() as u64 / INSTANCE_SIZE) as u32;
                    let end = offset + group.len() as u64;
                    if count == 0 {
                        continue;
                    }
                    let slice = self.instances.slice(offset..end);
                    offset = end;
                    if level == LEVEL_PIXELS.len() {
                        pass.set_pipeline(&self.kit.pipeline);
                        pass.set_bind_group(1, &animation.atlas.bind_group, &[]);
                        pass.set_vertex_buffer(0, slice);
                        pass.draw(0..4, 0..count);
                        continue;
                    }
                    pass.set_pipeline(&self.unit_pipeline);
                    pass.set_bind_group(1, &animation.bind_group, &[]);
                    pass.set_vertex_buffer(0, self.uvs.slice(..));
                    pass.set_vertex_buffer(1, slice);
                    pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
                    for part in &self.parts {
                        let (first, index_count) = part.levels[level.min(part.levels.len() - 1)];
                        pass.set_bind_group(2, &part.bind_group, &[]);
                        pass.draw_indexed(first..first + index_count, 0, 0..count);
                    }
                }
            }
        }
        self.gpu.queue.submit([encoder.finish()]);
        self.gpu.queue.present(frame);

        self.fps_frames += 1;
        let span = now - self.fps_since;
        if span >= FPS_INTERVAL_MS {
            let rate = f64::from(self.fps_frames) * 1000.0 / span;
            let count = |group: &Vec<u8>| group.len() as u64 / INSTANCE_SIZE;
            let levels: [u64; LEVELS] = std::array::from_fn(|level| {
                self.groups.iter().map(|gait| count(&gait[level])).sum()
            });
            let walking: u64 = self.groups[march::WALK].iter().map(count).sum();
            let in_view: u64 = levels.iter().sum();
            self.fps.set_text_content(Some(&format!(
                "{rate:.0} FPS, {in_view} in view ({walking} walking): meshes {}/{}/{}, impostors {}",
                levels[0], levels[1], levels[2], levels[3]
            )));
            self.fps_frames = 0;
            self.fps_since = now;
        }
    }

    /// The view-projection matrix and the camera's right, up and position.
    /// Left-handed with depth 0..1, like the engine's camera
    /// (`CMatrix3D::SetPerspective` puts view z in w), so models are not
    /// mirrored.
    fn camera(&self, aspect: f32) -> ([[f32; 4]; 4], [f32; 3], [f32; 3], [f32; 3]) {
        let target = TARGET;
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let eye = [
            target[0] + self.distance * cos_pitch * sin_yaw,
            target[1] + self.distance * sin_pitch,
            target[2] + self.distance * cos_pitch * cos_yaw,
        ];
        let (view, right, up) = look_at(eye, target);
        let f = 1.0 / (FIELD_OF_VIEW / 2.0).tan();
        let projection = [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, FAR / (FAR - NEAR), -NEAR * FAR / (FAR - NEAR)],
            [0.0, 0.0, 1.0, 0.0],
        ];
        (multiply(&projection, &view), right, up, eye)
    }
}

/// A left-handed view matrix, row-major, and the camera's right and up.
pub fn look_at(eye: [f32; 3], target: [f32; 3]) -> ([[f32; 4]; 4], [f32; 3], [f32; 3]) {
    let forward = normalize(sub(target, eye));
    let right = normalize(cross([0.0, 1.0, 0.0], forward));
    let up = cross(forward, right);
    let view = [
        [right[0], right[1], right[2], -dot(right, eye)],
        [up[0], up[1], up[2], -dot(up, eye)],
        [forward[0], forward[1], forward[2], -dot(forward, eye)],
        [0.0, 0.0, 0.0, 1.0],
    ];
    (view, right, up)
}

pub fn multiply(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut result = [[0.0; 4]; 4];
    for (r, row) in result.iter_mut().enumerate() {
        for (c, value) in row.iter_mut().enumerate() {
            *value = (0..4).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    result
}

/// The `View` uniform's bytes; `VIEW_SIZE` long.
pub fn view_bytes(params: &ViewParams) -> Vec<u8> {
    let sun = normalize([0.4, 0.8, -0.45]);
    let mut uniform: Vec<f32> = Vec::with_capacity((VIEW_SIZE / 4) as usize);
    // WGSL matrices are column-major.
    for column in 0..4 {
        for row in params.view_proj.iter() {
            uniform.push(row[column]);
        }
    }
    uniform.extend_from_slice(&[sun[0], sun[1], sun[2], 0.0]);
    uniform.extend_from_slice(&[PLAYER_COLOR[0], PLAYER_COLOR[1], PLAYER_COLOR[2], 1.0]);
    uniform.extend_from_slice(&[
        params.vertex_count as f32,
        params.texture_width as f32,
        0.0,
        0.0,
    ]);
    uniform.extend_from_slice(&[GROUND_HALF_SIZE, AMBIENT, SUN_STRENGTH, 0.0]);
    let [right, up, eye] = [params.right, params.up, params.eye];
    uniform.extend_from_slice(&[right[0], right[1], right[2], params.reach.radius]);
    uniform.extend_from_slice(&[up[0], up[1], up[2], params.reach.centre_y]);
    uniform.extend_from_slice(&[eye[0], eye[1], eye[2], 0.0]);
    uniform
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// The UV and per-instance buffers vs_unit reads.
pub fn unit_vertex_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 2] {
    [
        Some(wgpu::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &UV_ATTRIBUTES,
        }),
        Some(wgpu::VertexBufferLayout {
            array_stride: INSTANCE_SIZE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &INSTANCE_ATTRIBUTES,
        }),
    ]
}

/// Milliseconds per frame and frame count of each gait's animation.
fn timing(animations: &[Animation; 2]) -> march::Timing {
    animations
        .each_ref()
        .map(|animation| (f64::from(animation.frame_ms), animation.frames))
}

/// The six planes of the view volume as (normal, distance), normals
/// pointing inwards and of unit length, from a row-major view-projection
/// matrix whose clip depth runs 0..1.
fn frustum_planes(m: &[[f32; 4]; 4]) -> [[f32; 4]; 6] {
    let combine = |a: [f32; 4], b: [f32; 4], sign: f32| -> [f32; 4] {
        std::array::from_fn(|i| a[i] + sign * b[i])
    };
    let planes = [
        combine(m[3], m[0], 1.0),
        combine(m[3], m[0], -1.0),
        combine(m[3], m[1], 1.0),
        combine(m[3], m[1], -1.0),
        m[2],
        combine(m[3], m[2], -1.0),
    ];
    planes.map(|plane| {
        let length = (plane[0] * plane[0] + plane[1] * plane[1] + plane[2] * plane[2]).sqrt();
        plane.map(|value| value / length)
    })
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    [v[0] / length, v[1] / length, v[2] / length]
}

pub fn create_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        // Sizes must be multiples of 4 for write_buffer.
        size: (contents.len() as u64).next_multiple_of(4).max(4),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, contents);
    buffer
}

fn create_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    bytes: &[u8],
) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let texel_bytes = format.block_copy_size(None).unwrap_or(4);
    queue.write_texture(
        texture.as_image_copy(),
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * texel_bytes),
            rows_per_image: None,
        },
        size,
    );
    texture.create_view(&Default::default())
}

/// An RGBA8 texture with its full mip chain, each level the 2x2 average of
/// the one above, so distant units do not shimmer.
fn create_skin(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::TextureView {
    let levels = 32 - width.max(height).max(1).leading_zeros();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("unit skin"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let (mut width, mut height) = (width, height);
    let mut pixels = rgba.to_vec();
    for level in 0..levels {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        (pixels, width, height) = halve(&pixels, width, height);
    }
    texture.create_view(&Default::default())
}

/// Averages 2x2 blocks; an odd last row or column is averaged with itself.
fn halve(pixels: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let (half_width, half_height) = ((width / 2).max(1), (height / 2).max(1));
    let mut out = Vec::with_capacity((half_width * half_height * 4) as usize);
    for y in 0..half_height {
        for x in 0..half_width {
            let (x0, y0) = (x * 2, y * 2);
            let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
            for channel in 0..4 {
                let at =
                    |x: u32, y: u32| u32::from(pixels[((y * width + x) * 4 + channel) as usize]);
                let sum = at(x0, y0) + at(x1, y0) + at(x0, y1) + at(x1, y1);
                out.push(((sum + 2) / 4) as u8);
            }
        }
    }
    (out, half_width, half_height)
}

pub fn create_depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("unit depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn decode(bytes: &[u8]) -> Result<UnitFile, &'static str> {
    let mut cursor = Cursor(bytes);
    if cursor.take(4)? != b"SPCU" {
        return Err("not a unit file");
    }
    if cursor.u8()? != VERSION {
        return Err("unknown unit file version");
    }
    let vertex_count = cursor.u32()?;
    let texture_width = cursor.u32()?;
    let min = [cursor.f32()?, cursor.f32()?, cursor.f32()?];
    let max = [cursor.f32()?, cursor.f32()?, cursor.f32()?];
    let uvs = cursor.take(vertex_count as usize * 8)?.to_vec();
    let index_count = cursor.u32()?;
    let indices = cursor.take(index_count as usize * 4)?.to_vec();
    let mut parts = Vec::new();
    for _ in 0..cursor.u32()? {
        let mut levels = Vec::new();
        for _ in 0..cursor.u8()? {
            levels.push((cursor.u32()?, cursor.u32()?));
        }
        if levels.is_empty() {
            return Err("part without a mesh");
        }
        let flags = cursor.u8()?;
        let alpha_ref = cursor.f32()?;
        let width = cursor.u32()?;
        let height = cursor.u32()?;
        let rgba = cursor.take(width as usize * height as usize * 4)?.to_vec();
        parts.push(PartData {
            levels,
            flags,
            alpha_ref,
            width,
            height,
            rgba,
        });
    }
    let mut animations = Vec::new();
    for _ in 0..cursor.u32()? {
        let length = usize::from(cursor.u16()?);
        let name = std::str::from_utf8(cursor.take(length)?)
            .map_err(|_| "animation name not UTF-8")?
            .to_string();
        let frame_ms = cursor.f32()?;
        let frames = cursor.u32()?;
        let rows = (frames * vertex_count).div_ceil(texture_width);
        let size = rows as usize * texture_width as usize * 8;
        let positions = cursor.take(size)?.to_vec();
        let normals = cursor.take(size)?.to_vec();
        animations.push(AnimationData {
            name,
            frame_ms,
            frames,
            rows,
            positions,
            normals,
        });
    }
    if animations.is_empty() {
        return Err("no animations");
    }
    Ok(UnitFile {
        vertex_count,
        texture_width,
        min,
        max,
        uvs,
        indices,
        parts,
        animations,
    })
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], &'static str> {
        if self.0.len() < length {
            return Err("unit file truncated");
        }
        let (bytes, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, &'static str> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, &'static str> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, &'static str> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}
