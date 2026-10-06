//! The observer page's 3D view: the map's heightmap in minimap colours with
//! a water plane, and every entity with its actor's models, variant and
//! animation as the stream describes them. Pieces stream in on demand
//! (assets.rs); an entity shows once its own pieces have arrived.
//!
//! Models are skinned on the GPU from the pose atlas (pose.rs) and drawn
//! instanced, one draw per mesh, level of detail and texture. Props are
//! placed on the CPU, on their parent's bone at the current time. Meshes
//! and textures start as their coarse files; the detail files are asked
//! for only for entities drawn finer than the coarsest level. Entities
//! smaller still are impostors (far.rs) once their look's atlas is ready.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::rc::Rc;

use specula_assets::bc;
use specula_assets::files::AnimationData;
use specula_assets::files::FileRef;
use specula_assets::files::Mesh;
use specula_assets::files::NO_BONE;
use specula_assets::files::Texture;
use specula_assets::files::TextureFormat;
use specula_assets::files::VERTEX_SIZE;
use specula_assets::files::level_extent;
use specula_assets::math::Mat4;
use specula_assets::math::Quat;
use specula_assets::math::Vec3;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::Event;
use web_sys::HtmlCanvasElement;
use web_sys::HtmlElement;
use web_sys::MouseEvent;
use web_sys::WheelEvent;

use crate::StreamBytes;
use crate::TURN_MS;
use crate::View;
use crate::assets;
use crate::assets::Get;
use crate::assets::Progress;
use crate::assets::Store;
use crate::far;
use crate::far::Capture;
use crate::far::CaptureDraw;
use crate::far::Far;
use crate::far::LookKey;
use crate::far::Request;
use crate::gpu;
use crate::impostor;
use crate::pose;
use crate::stream::Dot;
use crate::stream::Scene;
use crate::stream::VariantSet;
use crate::stream::turn_towards;
use crate::terrain::TILE_SIZE;
use crate::terrain::TerrainImage;
use crate::unit::DEPTH_FORMAT;
use crate::unit::create_buffer;
use crate::unit::create_depth;
use crate::unit::frustum_planes;
use crate::unit::look_at;
use crate::unit::multiply;
use crate::variation;
use crate::variation::Model;
use crate::variation::ModelAnimation;

/// The stream's positions are in 1/16 m.
const POSITION_UNITS_PER_METRE: f32 = 16.0;
/// Matrix, sun, misc, water, camera right, up and eye; far.wgsl `View`,
/// of which model.wgsl and field.wgsl read the first four.
const VIEW_SIZE: u64 = 160;
/// Three f32x4 matrix rows, u32x4 pose, f32x4 params; model.wgsl.
const INSTANCE_SIZE: usize = 80;
/// Bounding sphere diameters on screen, in pixels, from which each baked
/// level of detail is used: the full mesh, then the coarser ones, and
/// last an impostor, its cells magnified up to 1.2 times.
const LEVEL_PIXELS: [f32; 3] = [400.0, 180.0, impostor::CELL as f32 * 1.2];
const LEVELS: usize = LEVEL_PIXELS.len() + 1;
/// The coarsest mesh level, the one the coarse files hold; finer levels
/// ask for the detail files.
const COARSE_LEVEL: usize = 2;
const FAR_LEVEL: usize = 3;
/// The pick impostors choose their animation variant with.
const FAR_PICK: u64 = 0;
/// How often the frame rate shown is recomputed; shorter flickers.
const FPS_INTERVAL_MS: f64 = 500.0;
const AMBIENT: f32 = 0.45;
const SUN_STRENGTH: f32 = 0.75;
const FIELD_OF_VIEW: f32 = 0.7;
const NEAR: f32 = 1.0;
const FAR: f32 = 4000.0;
const MIN_DISTANCE: f32 = 10.0;
const MAX_DISTANCE: f32 = 1500.0;
const START_DISTANCE: f32 = 120.0;
const START_PITCH: f32 = 0.85;
/// Radians per CSS pixel of drag.
const ROTATE_SPEED: f32 = 0.006;
/// Ground crossed per second by the pan keys, in camera distances.
const KEY_PAN_SPEED: f32 = 1.0;
/// A longer frame is a hidden tab or a stall; the camera should not leap.
const KEY_STEP_MAX_S: f32 = 0.1;
/// Zooming at a point near the horizon would throw the camera far off;
/// the point is kept within this many camera distances of the target.
const ZOOM_ANCHOR_MAX: f32 = 4.0;
/// Props reach past the root mesh (riders, spears, banners); a sphere
/// this much larger than its bounds still holds them.
const REACH_MARGIN: f32 = 1.5;
const BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.06,
    g: 0.07,
    b: 0.08,
    a: 1.0,
};
const MESH_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    3 => Uint8x4,
    4 => Unorm8x4,
];
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    5 => Float32x4,
    6 => Float32x4,
    7 => Float32x4,
    8 => Uint32x4,
    9 => Float32x4,
];
const TERRAIN_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];

/// (actor, variant set, animation, seed) as the stream indexes them: what
/// decides an entity's variation.
type ModelKey = (u32, u32, u32, u32);

/// (mesh, level of detail, texture) of one instanced draw.
type GroupKey = (u64, usize, u64);

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    levels: Vec<(u32, u32)>,
    /// Uploaded from the full mesh rather than the coarse file alone.
    complete: bool,
}

/// A look's capture laid out on the CPU before the pose atlas goes up:
/// its frames, instances, and per draw (slot, group, byte range, count).
struct Prepared {
    slots: u32,
    frames: u32,
    instances: Vec<u8>,
    draws: Vec<(u32, GroupKey, u64, u64, u32)>,
}

struct Ground {
    image: TerrainImage,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    bind_group: wgpu::BindGroup,
}

struct Pointer {
    id: i32,
    button: i16,
    /// In CSS pixels.
    x: f32,
    y: f32,
}

struct Camera {
    /// The ground point looked at, x and z in metres.
    target: [f32; 2],
    yaw: f32,
    pitch: f32,
    distance: f32,
    /// Mice, fingers and pens down on the canvas.
    pointers: Vec<Pointer>,
    /// Set over the map's centre once the first keyframe tells its size.
    placed: bool,
    /// Pan keys held, `KEY_*` bits.
    keys: u8,
    /// When `step` last ran, in page milliseconds.
    last_step: f64,
}

const KEY_FORWARD: u8 = 1;
const KEY_BACK: u8 = 2;
const KEY_LEFT: u8 = 4;
const KEY_RIGHT: u8 = 8;

/// The pan bit of a `KeyboardEvent.code`: by position, so WASD sits under
/// the same fingers on any layout.
fn key_bit(code: &str) -> u8 {
    match code {
        "KeyW" | "ArrowUp" => KEY_FORWARD,
        "KeyS" | "ArrowDown" => KEY_BACK,
        "KeyA" | "ArrowLeft" => KEY_LEFT,
        "KeyD" | "ArrowRight" => KEY_RIGHT,
        _ => 0,
    }
}

/// What the view keeps of the scene between turns.
#[derive(Default)]
struct Entities {
    turn: u32,
    match_id: u64,
    map_size: u32,
    dots: Vec<(u32, Dot)>,
    strings: Vec<String>,
    variant_sets: Vec<VariantSet>,
}

pub struct Field {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    bc: bool,
    model_pipeline: wgpu::RenderPipeline,
    terrain_pipeline: wgpu::RenderPipeline,
    water_pipeline: wgpu::RenderPipeline,
    view_buffer: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    poses: pose::Atlas,
    pose_layout: wgpu::BindGroupLayout,
    pose_bind_group: wgpu::BindGroup,
    pose_generation: u32,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::BindGroup,
    ground: Option<Ground>,
    store: Rc<RefCell<Store>>,
    meshes: HashMap<u64, GpuMesh>,
    /// By coarse file hash; true once uploaded with the detail.
    textures: HashMap<u64, (wgpu::BindGroup, bool)>,
    entities: Entities,
    palette: Vec<u32>,
    models: HashMap<ModelKey, Option<Rc<Model>>>,
    camera: Rc<RefCell<Camera>>,
    instances: wgpu::Buffer,
    groups: HashMap<GroupKey, Vec<u8>>,
    /// Model instances placed this frame, captures included.
    placed: u32,
    far: Far,
    far_instances: wgpu::Buffer,
    far_groups: HashMap<LookKey, Vec<u8>>,
    /// Where the camera is this frame, which impostor cells depend on.
    eye: [f32; 3],
    stats: Stats,
}

/// What the `#fps` box shows, as on the unit page.
struct Stats {
    element: HtmlElement,
    /// Frames drawn since `since`, in page milliseconds.
    frames: u32,
    since: f64,
    /// This frame's model instances, and its entities drawn at each level
    /// of detail, the full mesh first, impostors last.
    models: u32,
    levels: [u32; LEVELS],
    stream: StreamBytes,
}

impl Field {
    pub async fn new(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let gpu::Gpu {
            surface,
            device,
            queue,
            config,
        } = gpu::init(canvas.clone()).await?;
        let bc = device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let model_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model"),
            source: wgpu::ShaderSource::Wgsl(include_str!("model.wgsl").into()),
        });
        let field_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("field"),
            source: wgpu::ShaderSource::Wgsl(include_str!("field.wgsl").into()),
        });

        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("field view"),
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
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("field view"),
            size: VIEW_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("field view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });
        // Read with textureLoad, so it needs no filtering.
        let pose_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("poses"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("field texture"),
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
            ],
        });
        // ObjectEntry.cpp gives model textures CLAMP_TO_EDGE.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("field"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let depth_stencil = |write: bool| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        };
        let model_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model"),
            bind_group_layouts: &[
                Some(&view_layout),
                Some(&pose_layout),
                Some(&texture_layout),
            ],
            immediate_size: 0,
        });
        let model_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: VERTEX_SIZE as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &MESH_ATTRIBUTES,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE_SIZE as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &INSTANCE_ATTRIBUTES,
            }),
        ];
        let model_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("model"),
            layout: Some(&model_layout),
            vertex: wgpu::VertexState {
                module: &model_shader,
                entry_point: Some("vs_model"),
                compilation_options: Default::default(),
                buffers: &model_buffers,
            },
            // The engine draws models double-sided where the material asks;
            // without culling every model looks right.
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_stencil(true)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &model_shader,
                entry_point: Some("fs_model"),
                compilation_options: Default::default(),
                targets: &[Some(config.format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let terrain_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain"),
            bind_group_layouts: &[Some(&view_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let terrain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain"),
            layout: Some(&terrain_layout),
            vertex: wgpu::VertexState {
                module: &field_shader,
                entry_point: Some("vs_terrain"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &TERRAIN_ATTRIBUTES,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_stencil(true)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &field_shader,
                entry_point: Some("fs_terrain"),
                compilation_options: Default::default(),
                targets: &[Some(config.format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let water_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water"),
            bind_group_layouts: &[Some(&view_layout)],
            immediate_size: 0,
        });
        let water_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water"),
            layout: Some(&water_layout),
            vertex: wgpu::VertexState {
                module: &field_shader,
                entry_point: Some("vs_water"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            // Seen through, so it must not hide what is under it.
            depth_stencil: Some(depth_stencil(false)),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &field_shader,
                entry_point: Some("fs_water"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let far = Far::new(
            &device,
            config.format,
            &model_shader,
            &model_buffers,
            &view_layout,
            &pose_layout,
            &texture_layout,
            START_PITCH,
        );
        let poses = pose::Atlas::new(&device);
        let pose_bind_group = pose_bind_group(&device, &pose_layout, &poses.view);
        let pose_generation = poses.generation;
        let white = {
            let texture = Texture {
                format: TextureFormat::Rgba8,
                width: 1,
                height: 1,
                levels: vec![vec![0xFF; 4]],
                detail: None,
            };
            let view = upload_texture(&device, &queue, &texture, false);
            texture_bind_group(&device, &texture_layout, &view, &sampler)
        };
        let depth = create_depth(&device, config.width, config.height);
        let instances = create_buffer(
            &device,
            &queue,
            "model instances",
            &[],
            wgpu::BufferUsages::VERTEX,
        );
        let far_instances = create_buffer(
            &device,
            &queue,
            "impostor instances",
            &[],
            wgpu::BufferUsages::VERTEX,
        );
        let camera = Rc::new(RefCell::new(Camera {
            target: [0.0, 0.0],
            yaw: 0.0,
            pitch: START_PITCH,
            distance: START_DISTANCE,
            pointers: Vec::new(),
            placed: false,
            keys: 0,
            last_step: now(),
        }));
        wire_camera(&canvas, &camera)?;
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or("no document")?;
        let stats = Stats {
            element: crate::element(&document, "fps")?,
            frames: 0,
            since: now(),
            models: 0,
            levels: [0; LEVELS],
            stream: StreamBytes::default(),
        };
        Ok(Field {
            surface,
            device,
            queue,
            config,
            depth,
            bc,
            model_pipeline,
            terrain_pipeline,
            water_pipeline,
            view_buffer,
            view_bind_group,
            poses,
            pose_layout,
            pose_bind_group,
            pose_generation,
            texture_layout,
            sampler,
            white,
            ground: None,
            store: assets::new(),
            meshes: HashMap::new(),
            textures: HashMap::new(),
            entities: Entities::default(),
            palette: Vec::new(),
            models: HashMap::new(),
            camera,
            instances,
            groups: HashMap::new(),
            placed: 0,
            far,
            far_instances,
            far_groups: HashMap::new(),
            eye: [0.0; 3],
            stats,
        })
    }

    /// The view-projection matrix, row-major, left-handed with depth 0..1
    /// like the engine's camera (as `unit.rs`), and the eye, right and up.
    fn view_projection(&self) -> ([[f32; 4]; 4], [f32; 3], [f32; 3], [f32; 3]) {
        let camera = self.camera.borrow();
        let ground = self.ground.as_ref().map_or(0.0, |ground| {
            ground.image.ground(camera.target[0], camera.target[1])
        });
        let target = [camera.target[0], ground, camera.target[1]];
        let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = camera.pitch.sin_cos();
        let eye = [
            target[0] + camera.distance * cos_pitch * sin_yaw,
            target[1] + camera.distance * sin_pitch,
            target[2] + camera.distance * cos_pitch * cos_yaw,
        ];
        let (view, right, up) = look_at(eye, target);
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let f = 1.0 / (FIELD_OF_VIEW / 2.0).tan();
        let projection = [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, FAR / (FAR - NEAR), -NEAR * FAR / (FAR - NEAR)],
            [0.0, 0.0, 1.0, 0.0],
        ];
        (multiply(&projection, &view), eye, right, up)
    }

    fn write_view(&self, view_proj: &[[f32; 4]; 4], camera: [[f32; 3]; 3]) {
        let sun = Vec3::new(0.4, 0.8, -0.45).normalized();
        let (size, circular, water) = match &self.ground {
            Some(ground) => (
                ground.image.size as f32 * TILE_SIZE,
                if ground.image.circular { 1.0 } else { 0.0 },
                ground.image.water_level,
            ),
            None => (self.entities.map_size as f32, 0.0, 0.0),
        };
        let mut uniform: Vec<f32> = Vec::with_capacity((VIEW_SIZE / 4) as usize);
        // WGSL matrices are column-major.
        for column in 0..4 {
            for row in view_proj {
                uniform.push(row[column]);
            }
        }
        uniform.extend_from_slice(&[sun.x, sun.y, sun.z, 0.0]);
        uniform.extend_from_slice(&[AMBIENT, SUN_STRENGTH, size, circular]);
        uniform.extend_from_slice(&[water, 0.0, 0.0, 0.0]);
        for [x, y, z] in camera {
            uniform.extend_from_slice(&[x, y, z, 0.0]);
        }
        let bytes: Vec<u8> = uniform
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        self.queue.write_buffer(&self.view_buffer, 0, &bytes);
    }

    /// Fills `groups` and `far_groups` with this frame's instances.
    fn place_entities(&mut self, progress: f32, view_proj: &[[f32; 4]; 4]) {
        let mut groups = std::mem::take(&mut self.groups);
        for group in groups.values_mut().chain(self.far_groups.values_mut()) {
            group.clear();
        }
        self.placed = 0;
        self.stats.levels = [0; LEVELS];
        let planes = frustum_planes(view_proj);
        let pixel_scale = self.config.height as f32 / (FIELD_OF_VIEW / 2.0).tan();
        let dots = std::mem::take(&mut self.entities.dots);
        for (id, dot) in &dots {
            self.place_entity(
                *id,
                dot,
                progress,
                view_proj,
                &planes,
                pixel_scale,
                &mut groups,
            );
        }
        self.entities.dots = dots;
        self.groups = groups;
        self.stats.models = self.placed;
    }

    #[allow(clippy::too_many_arguments)]
    fn place_entity(
        &mut self,
        id: u32,
        dot: &Dot,
        progress: f32,
        view_proj: &[[f32; 4]; 4],
        planes: &[[f32; 4]; 6],
        pixel_scale: f32,
        groups: &mut HashMap<GroupKey, Vec<u8>>,
    ) {
        let Some(actor) = string(&self.entities.strings, dot.actor) else {
            return;
        };
        let manifest = match self.store.borrow_mut().actor(actor) {
            Get::Ready(manifest) => manifest,
            Get::Pending | Get::Missing => return,
        };
        let key = (dot.actor, dot.variants, dot.animation, dot.seed);
        let model = match self.models.get(&key) {
            Some(model) => model.clone(),
            None => {
                let selections = self.selections(dot);
                let model = variation::resolve(&manifest, &selections, dot.seed).map(Rc::new);
                self.models.insert(key, model.clone());
                model
            }
        };
        let Some(model) = model else {
            return;
        };
        let root_mesh = match self.store.borrow_mut().mesh(model.mesh, false) {
            Get::Ready(mesh) => mesh,
            Get::Pending | Get::Missing => return,
        };

        let lerp = |from: f32, to: i32| from + (to as f32 - from) * progress;
        let x = lerp(dot.from_x, dot.x) / POSITION_UNITS_PER_METRE;
        let z = lerp(dot.from_z, dot.z) / POSITION_UNITS_PER_METRE;
        // Over ground, follow the terrain between turns rather than cut
        // across it, as the engine places ground units each frame.
        let y = match &self.ground {
            Some(ground) => {
                let height = |x: f32, z: f32, y: f32| {
                    y / POSITION_UNITS_PER_METRE
                        - ground
                            .image
                            .ground(x / POSITION_UNITS_PER_METRE, z / POSITION_UNITS_PER_METRE)
                };
                let from = height(dot.from_x, dot.from_z, dot.from_y);
                let to = height(dot.x as f32, dot.z as f32, dot.y as f32);
                ground.image.ground(x, z) + from + (to - from) * progress
            }
            None => lerp(dot.from_y, dot.y) / POSITION_UNITS_PER_METRE,
        };
        let height = root_mesh.max[1] - root_mesh.min[1];
        // CCmpPosition::GetConstructionProgressOffset, with the root
        // mesh's height for the selection box's.
        let sink = if dot.construction == 0 {
            0.0
        } else {
            (f32::from(dot.construction) / 255.0 - 1.0) * height
        };
        let angle = dot.from_angle + turn_towards(dot.from_angle, dot.angle) * progress;
        let rot_y = angle / 256.0 * std::f32::consts::TAU;
        // CCmpPosition::GetInterpolatedTransform turns models by rotY + PI.
        let half = (rot_y + std::f32::consts::PI) / 2.0;
        let mut transform = Mat4::from_quat(Quat {
            x: 0.0,
            y: half.sin(),
            z: 0.0,
            w: half.cos(),
        });
        transform.translate(Vec3::new(x, y + sink, z));

        let extent = Vec3::new(
            root_mesh.max[0] - root_mesh.min[0],
            height,
            root_mesh.max[2] - root_mesh.min[2],
        );
        let radius = extent.length() / 2.0 * REACH_MARGIN + 1.0;
        let centre = [x, y + sink + (root_mesh.min[1] + root_mesh.max[1]) / 2.0, z];
        let outside = planes.iter().any(|plane| {
            plane[0] * centre[0] + plane[1] * centre[1] + plane[2] * centre[2] + plane[3] < -radius
        });
        if outside {
            return;
        }
        let depth = view_proj[3][0] * centre[0]
            + view_proj[3][1] * centre[1]
            + view_proj[3][2] * centre[2]
            + view_proj[3][3];
        let diameter = 2.0 * radius * pixel_scale / depth.max(NEAR);
        let level = LEVEL_PIXELS
            .iter()
            .position(|pixels| diameter >= *pixels)
            .unwrap_or(LEVEL_PIXELS.len());

        let state = string(&self.entities.strings, dot.animation)
            .map_or_else(|| "idle".to_string(), str::to_lowercase);
        let color = owner_color(dot, &self.palette);
        let mut pick = DefaultHasher::new();
        (id, dot.animation_turn).hash(&mut pick);
        let entity = EntityState {
            dot: *dot,
            turn: self.entities.turn,
            progress,
            state,
            pick: pick.finish(),
            color,
            level,
            time: None,
        };
        if level == FAR_LEVEL && self.place_far(&model, &root_mesh, &entity, centre, rot_y) {
            self.stats.levels[FAR_LEVEL] += 1;
            return;
        }
        let placed = self.placed;
        self.place_model(&model, &transform, &entity, None, 0, groups);
        // Counted once something of it is drawn, not while it loads.
        if self.placed > placed {
            self.stats.levels[level.min(COARSE_LEVEL)] += 1;
        }
    }

    /// Draws the entity as its look's impostor; false while the atlas is
    /// not ready, which asks for it.
    fn place_far(
        &mut self,
        model: &Rc<Model>,
        root_mesh: &Mesh,
        entity: &EntityState,
        centre: [f32; 3],
        rot_y: f32,
    ) -> bool {
        // One look per animation, not per variant of it: at this size the
        // variant does not show, and every look is an atlas to hold.
        let Some(animation) = self.root_animation(model, root_mesh, &entity.state, FAR_PICK) else {
            return false;
        };
        let dot = &entity.dot;
        let animation_hash = animation.as_ref().map_or(0, |(chosen, _)| chosen.file.hash);
        let key = (dot.actor, dot.animation, animation_hash);
        if self.far.get(key).is_none() {
            if self.far.asked(key) {
                return false;
            }
            let actor = string(&self.entities.strings, dot.actor).unwrap_or_default();
            let extent = Vec3::new(
                root_mesh.max[0] - root_mesh.min[0],
                root_mesh.max[1] - root_mesh.min[1],
                root_mesh.max[2] - root_mesh.min[2],
            );
            let request = Request {
                model: Rc::clone(model),
                dot: *dot,
                state: entity.state.clone(),
                pick: FAR_PICK,
                radius: extent.length() / 2.0 * REACH_MARGIN + 1.0,
                centre_y: (root_mesh.min[1] + root_mesh.max[1]) / 2.0,
                file: far::file_name(actor, &entity.state, animation_hash, START_PITCH),
            };
            self.far.want(key, request);
            return false;
        }
        let frame = animation.as_ref().map_or(0.0, |(chosen, data)| {
            let (time, _) = animation_time(entity, chosen, data);
            f64::from(time / data.frame_ms.max(f32::EPSILON))
        });
        // The model turns by rotY + PI (place_entity); the cell is picked
        // by the direction to the camera in the model's own frame.
        let (sin, cos) = (rot_y + std::f32::consts::PI).sin_cos();
        let (dx, dz) = (self.eye[0] - centre[0], self.eye[2] - centre[2]);
        let yaw = f64::from((cos * dx - sin * dz).atan2(sin * dx + cos * dz));
        let Some(look) = self.far.get(key) else {
            return false;
        };
        let cell = look.atlas.cell(frame, yaw);
        let place = [
            centre[0],
            centre[1],
            centre[2],
            look.radius,
            sin,
            cos,
            look.atlas.columns as f32,
            look.atlas.rows as f32,
        ];
        let bytes = self.far_groups.entry(key).or_default();
        for value in place {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in [cell, entity.color] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        true
    }

    /// The root model's animation as `place_model` picks it, with its
    /// data: `Some(None)` for a model that does not animate, `None` while
    /// the data is on its way.
    fn root_animation<'a>(
        &mut self,
        model: &'a Model,
        root_mesh: &Mesh,
        state: &str,
        pick: u64,
    ) -> Option<Option<(&'a ModelAnimation, Rc<AnimationData>)>> {
        let chosen = root_mesh
            .skeleton
            .and_then(|_| choose_animation(&model.animations, state, None, pick));
        match chosen {
            Some(chosen) => match self.store.borrow_mut().animation(chosen.file) {
                Get::Ready(data) => Some(Some((chosen, data))),
                Get::Pending => None,
                Get::Missing => Some(None),
            },
            None => Some(None),
        }
    }

    /// Lays out the capture of a look's atlas, every frame slot placed in
    /// the model's own space at its coarsest level; `None` while a piece
    /// of it is still on its way.
    fn prepare_capture(&mut self, request: &Request) -> Option<Prepared> {
        let model = Rc::clone(&request.model);
        let root_mesh = match self.store.borrow_mut().mesh(model.mesh, false) {
            Get::Ready(mesh) => mesh,
            Get::Pending | Get::Missing => return None,
        };
        let animation = self.root_animation(&model, &root_mesh, &request.state, request.pick)?;
        let (frames, frame_ms, slots) = match &animation {
            Some((_, data)) => (
                data.frames,
                data.frame_ms,
                far::slots(&request.state, data.frames),
            ),
            None => (1, 0.0, 1),
        };
        let mut prepared = Prepared {
            slots,
            frames,
            instances: Vec::new(),
            draws: Vec::new(),
        };
        for slot in 0..slots {
            // As impostor::Atlas::cell spreads slots over the frames.
            let position = slot as f32 * frames as f32 / slots as f32;
            let entity = EntityState {
                dot: request.dot,
                turn: self.entities.turn,
                progress: 0.0,
                state: request.state.clone(),
                pick: request.pick,
                color: 0xFFFF_FFFF,
                level: usize::MAX,
                time: Some(position * frame_ms),
            };
            let mut groups = HashMap::new();
            if !self.place_model(&model, &Mat4::identity(), &entity, None, 0, &mut groups) {
                return None;
            }
            for (key, bytes) in groups {
                let start = prepared.instances.len() as u64;
                prepared.instances.extend_from_slice(&bytes);
                let count = (bytes.len() / INSTANCE_SIZE) as u32;
                prepared
                    .draws
                    .push((slot, key, start, prepared.instances.len() as u64, count));
            }
        }
        Some(prepared)
    }

    /// Draws the prepared atlases; the pose atlas must be up already.
    fn bake(&mut self, prepared: Vec<(LookKey, Request, Prepared)>) {
        for (key, request, prepared) in prepared {
            let draws: Vec<CaptureDraw> = prepared
                .draws
                .iter()
                .filter_map(|(slot, (mesh, level, texture), start, end, count)| {
                    let gpu = self.meshes.get(mesh)?;
                    Some(CaptureDraw {
                        slot: *slot,
                        vertices: &gpu.vertices,
                        indices: &gpu.indices,
                        index_range: gpu.levels[*level],
                        texture: self
                            .textures
                            .get(texture)
                            .map_or(&self.white, |(group, _)| group),
                        instances: (*start, *end),
                        count: *count,
                    })
                })
                .collect();
            let capture = Capture {
                slots: prepared.slots,
                frames: prepared.frames,
                instances: &prepared.instances,
                draws: &draws,
                poses: &self.pose_bind_group,
            };
            self.far
                .bake(&self.device, &self.queue, key, request, &capture);
        }
    }

    /// Draws `model` with `transform` into `groups` and places its props.
    /// `root_id` is the animation id the root picked, which props follow.
    /// False when a piece of the tree is still on its way.
    fn place_model(
        &mut self,
        model: &Model,
        transform: &Mat4,
        entity: &EntityState,
        root_id: Option<&str>,
        depth: usize,
        groups: &mut HashMap<GroupKey, Vec<u8>>,
    ) -> bool {
        let detail = entity.level < COARSE_LEVEL;
        let mesh = match self.store.borrow_mut().mesh(model.mesh, detail) {
            Get::Ready(mesh) => mesh,
            Get::Pending => return false,
            Get::Missing => return true,
        };
        let skeleton = match mesh.skeleton {
            Some(file) => match self.store.borrow_mut().skeleton(file) {
                Get::Ready(skeleton) => Some((file.hash, skeleton)),
                Get::Pending => return false,
                Get::Missing => return true,
            },
            None => None,
        };
        let chosen = choose_animation(&model.animations, &entity.state, root_id, entity.pick);
        let animation = match (chosen, &skeleton) {
            (Some(chosen), Some(_)) => match self.store.borrow_mut().animation(chosen.file) {
                Get::Ready(data) => Some((chosen, data)),
                // Better nothing for a moment than a model in its bind pose.
                Get::Pending => return false,
                Get::Missing => None,
            },
            _ => None,
        };
        let texture = match model.texture {
            Some(file) => match self.texture(file, detail) {
                Some(Some(hash)) => hash,
                Some(None) => 0,
                None => return false,
            },
            None => 0,
        };

        let timing = animation.as_ref().map(|(chosen, data)| {
            let (time, looping) = animation_time(entity, chosen, data);
            (data, pose::frames(data, time, looping))
        });
        let mut pose = [0u32; 3];
        let mut blend = 0.0;
        if let (Some((data, frames)), Some((skeleton_hash, skeleton))) = (&timing, &skeleton) {
            let chosen_hash = animation.as_ref().map_or(0, |(chosen, _)| chosen.file.hash);
            if let Some(range) = self
                .poses
                .range(chosen_hash, data, *skeleton_hash, skeleton)
            {
                pose = [range.frame(frames.0), range.frame(frames.1), range.bones];
                blend = frames.2;
            }
        }
        self.add_instance(
            &mesh, model, transform, entity, pose, blend, texture, groups,
        );

        if depth >= 8 {
            return true;
        }
        let mut complete = true;
        let root_id = root_id.or(animation.as_ref().map(|(chosen, _)| chosen.id.as_str()));
        for (point_name, prop) in &model.props {
            let Some(point) = mesh
                .prop_points
                .iter()
                .find(|point| point.name == *point_name)
            else {
                continue;
            };
            let [px, py, pz] = point.position;
            let [qx, qy, qz, qw] = point.rotation;
            let point_matrix = Mat4::from_bone(
                Vec3::new(px, py, pz),
                Quat {
                    x: qx,
                    y: qy,
                    z: qz,
                    w: qw,
                },
            );
            // bake.rs pose_frame: parent * bone * point, or parent * point
            // for a point on the model's origin.
            let mut placed = *transform;
            if point.bone != NO_BONE
                && let Some((_, skeleton)) = &skeleton
            {
                let bone = pose::bone_matrix(
                    timing.map(|(data, frames)| (data.as_ref(), frames)),
                    skeleton,
                    usize::from(point.bone),
                );
                placed = placed.mul(&bone);
            }
            placed = placed.mul(&point_matrix);
            complete &= self.place_model(prop, &placed, entity, root_id, depth + 1, groups);
        }
        complete
    }

    #[allow(clippy::too_many_arguments)]
    fn add_instance(
        &mut self,
        mesh: &Mesh,
        model: &Model,
        transform: &Mat4,
        entity: &EntityState,
        pose: [u32; 3],
        blend: f32,
        texture: u64,
        groups: &mut HashMap<GroupKey, Vec<u8>>,
    ) {
        let complete = mesh.detail.is_none();
        let current = self
            .meshes
            .get(&model.mesh.hash)
            .is_some_and(|gpu| gpu.complete == complete);
        if !current {
            let gpu = GpuMesh {
                vertices: create_buffer(
                    &self.device,
                    &self.queue,
                    "mesh",
                    &mesh.vertices,
                    wgpu::BufferUsages::VERTEX,
                ),
                indices: create_buffer(
                    &self.device,
                    &self.queue,
                    "mesh indices",
                    &mesh
                        .indices
                        .iter()
                        .flat_map(|index| index.to_le_bytes())
                        .collect::<Vec<u8>>(),
                    wgpu::BufferUsages::INDEX,
                ),
                levels: mesh.levels.clone(),
                complete,
            };
            self.meshes.insert(model.mesh.hash, gpu);
        }
        self.placed += 1;
        let level = entity.level.min(mesh.levels.len() - 1);
        let bytes = groups.entry((model.mesh.hash, level, texture)).or_default();
        for row in &transform.m[..3] {
            for value in row {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        for value in [pose[0], pose[1], pose[2], entity.color] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let tinted = if model.player_color { 1.0 } else { 0.0 };
        for value in [blend, tinted, model.alpha_ref, 0.0f32] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }

    /// The bind group of a texture: `Some(Some(hash))` when ready,
    /// `Some(None)` to draw it white (it will not come), `None` while it
    /// is on its way. Uploaded again once its detail has arrived.
    fn texture(&mut self, file: FileRef, detail: bool) -> Option<Option<u64>> {
        let texture = match self.store.borrow_mut().texture(file, detail) {
            Get::Ready(texture) => texture,
            Get::Pending => return None,
            Get::Missing => return Some(None),
        };
        let complete = texture.detail.is_none();
        let current = self
            .textures
            .get(&file.hash)
            .is_some_and(|(_, held)| *held == complete);
        if !current {
            let view = upload_texture(&self.device, &self.queue, &texture, self.bc);
            let group =
                texture_bind_group(&self.device, &self.texture_layout, &view, &self.sampler);
            self.textures.insert(file.hash, (group, complete));
        }
        Some(Some(file.hash))
    }

    /// `CUnit::ReloadObject`'s entity selections: the variant selections
    /// and the animation name, lower case as `SetEntitySelection` keeps
    /// them.
    fn selections(&self, dot: &Dot) -> BTreeSet<String> {
        let strings = &self.entities.strings;
        let mut selections: BTreeSet<String> = (dot.variants as usize)
            .checked_sub(1)
            .and_then(|index| self.entities.variant_sets.get(index))
            .into_iter()
            .flatten()
            .filter_map(|(_, selection)| strings.get(*selection as usize))
            .map(|selection| selection.to_lowercase())
            .collect();
        if let Some(animation) = string(strings, dot.animation) {
            selections.insert(animation.to_lowercase());
        }
        selections
    }

    fn upload_ground(&mut self, image: &TerrainImage) {
        let tiles = image.size as usize;
        let side = tiles + 1;
        let height = |i: usize, j: usize| f32::from(image.heights[j * side + i]) / 92.0;
        let mut vertices = Vec::with_capacity(side * side * 24);
        for j in 0..side {
            for i in 0..side {
                let left = height(i.saturating_sub(1), j);
                let right = height((i + 1).min(tiles), j);
                let down = height(i, j.saturating_sub(1));
                let up = height(i, (j + 1).min(tiles));
                let normal = Vec3::new(left - right, 2.0 * TILE_SIZE, down - up).normalized();
                for value in [
                    i as f32 * TILE_SIZE,
                    height(i, j),
                    j as f32 * TILE_SIZE,
                    normal.x,
                    normal.y,
                    normal.z,
                ] {
                    vertices.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        let mut indices: Vec<u8> = Vec::with_capacity(tiles * tiles * 24);
        for j in 0..tiles {
            for i in 0..tiles {
                let at = |di: usize, dj: usize| ((j + dj) * side + i + di) as u32;
                // The same diagonal as CTerrain::GetExactGroundLevel, so
                // units stand on the surface drawn.
                let triangles = if image.triangulation_dir(i, j) {
                    [at(0, 0), at(1, 0), at(0, 1), at(1, 0), at(1, 1), at(0, 1)]
                } else {
                    [at(0, 0), at(1, 1), at(0, 1), at(0, 0), at(1, 0), at(1, 1)]
                };
                for index in triangles {
                    indices.extend_from_slice(&index.to_le_bytes());
                }
            }
        }
        let minimap = Texture {
            format: TextureFormat::Rgba8,
            width: image.size,
            height: image.size,
            levels: vec![image.pixels.clone()],
            detail: None,
        };
        let view = upload_texture(&self.device, &self.queue, &minimap, false);
        let bind_group =
            texture_bind_group(&self.device, &self.texture_layout, &view, &self.sampler);
        self.ground = Some(Ground {
            image: image.clone(),
            vertices: create_buffer(
                &self.device,
                &self.queue,
                "terrain",
                &vertices,
                wgpu::BufferUsages::VERTEX,
            ),
            indices: create_buffer(
                &self.device,
                &self.queue,
                "terrain indices",
                &indices,
                wgpu::BufferUsages::INDEX,
            ),
            index_count: (tiles * tiles * 6) as u32,
            bind_group,
        });
    }
}

impl View for Field {
    fn max_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    fn resize(&mut self, width: u32, height: u32, _device_pixel_ratio: f32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.depth = create_depth(&self.device, self.config.width, self.config.height);
    }

    fn animating(&self) -> bool {
        true
    }

    fn progress(&self) -> Progress {
        self.store.borrow().progress()
    }

    fn set_stream_bytes(&mut self, bytes: StreamBytes) {
        self.stats.stream = bytes;
    }

    fn set_scene(&mut self, scene: &Scene, palette: &[u32]) {
        let entities = &mut self.entities;
        if scene.match_id != entities.match_id {
            // String and variant set indices belong to their match.
            self.models.clear();
            self.far.clear();
            entities.match_id = scene.match_id;
            entities.strings.clear();
            entities.variant_sets.clear();
            self.camera.borrow_mut().placed = false;
        }
        // The tables only grow within a match, so only the tail is new.
        if scene.strings.len() < entities.strings.len() {
            entities.strings.clear();
        }
        let known = entities.strings.len();
        entities
            .strings
            .extend(scene.strings[known..].iter().cloned());
        if scene.variant_sets.len() < entities.variant_sets.len() {
            entities.variant_sets.clear();
        }
        let known = entities.variant_sets.len();
        entities
            .variant_sets
            .extend(scene.variant_sets[known..].iter().cloned());
        entities.turn = scene.turn;
        entities.map_size = scene.map_size;
        entities.dots.clear();
        entities
            .dots
            .extend(scene.dots.iter().map(|(id, dot)| (*id, *dot)));
        self.palette = palette.to_vec();
        let mut camera = self.camera.borrow_mut();
        if !camera.placed && scene.map_size > 0 {
            camera.placed = true;
            camera.target = start_point(scene);
        }
    }

    fn set_terrain(&mut self, image: Option<&TerrainImage>) {
        match image {
            Some(image) if image.size > 0 => self.upload_ground(image),
            _ => self.ground = None,
        }
    }

    fn draw(&mut self, progress: f32) -> bool {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            _ => return false,
        };
        self.camera.borrow_mut().step(now());
        let (view_proj, eye, right, up) = self.view_projection();
        self.write_view(&view_proj, [right, up, eye]);
        self.eye = eye;
        if let Some(bake) = self.store.borrow().index_name() {
            self.far.open_storage(bake);
        }
        self.far.begin_frame(&self.device);
        self.place_entities(progress, &view_proj);
        self.far.take_loaded(&self.device, &self.queue);
        let mut prepared = Vec::new();
        for (key, request) in self.far.next_bakes(&self.device) {
            match self.prepare_capture(&request) {
                Some(capture) => prepared.push((key, request, capture)),
                None => self.far.defer(key, request),
            }
        }
        assets::pump(&self.store);
        self.poses.upload(&self.device, &self.queue);
        if self.poses.generation != self.pose_generation {
            self.pose_generation = self.poses.generation;
            self.pose_bind_group =
                pose_bind_group(&self.device, &self.pose_layout, &self.poses.view);
        }
        self.bake(prepared);

        // One buffer for all groups, each drawn from its own slice.
        let mut bytes = Vec::new();
        let mut draws = Vec::new();
        for (key, group) in &self.groups {
            if group.is_empty() {
                continue;
            }
            let start = bytes.len() as u64;
            bytes.extend_from_slice(group);
            draws.push((
                *key,
                start,
                bytes.len() as u64,
                (group.len() / INSTANCE_SIZE) as u32,
            ));
        }
        if bytes.len() as u64 > self.instances.size() {
            // Doubling keeps reallocations rare while a match grows.
            self.instances = create_buffer(
                &self.device,
                &self.queue,
                "model instances",
                &vec![0; (bytes.len() as u64).next_power_of_two() as usize],
                wgpu::BufferUsages::VERTEX,
            );
        }
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.instances, 0, &bytes);
        }
        let mut far_bytes = Vec::new();
        let mut far_draws = Vec::new();
        for (key, group) in &self.far_groups {
            if group.is_empty() {
                continue;
            }
            let start = far_bytes.len() as u64;
            far_bytes.extend_from_slice(group);
            far_draws.push((
                *key,
                start,
                far_bytes.len() as u64,
                (group.len() / far::INSTANCE_SIZE) as u32,
            ));
        }
        if far_bytes.len() as u64 > self.far_instances.size() {
            self.far_instances = create_buffer(
                &self.device,
                &self.queue,
                "impostor instances",
                &vec![0; (far_bytes.len() as u64).next_power_of_two() as usize],
                wgpu::BufferUsages::VERTEX,
            );
        }
        if !far_bytes.is_empty() {
            self.queue.write_buffer(&self.far_instances, 0, &far_bytes);
        }

        let target = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("field"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
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
            if let Some(ground) = &self.ground {
                pass.set_pipeline(&self.terrain_pipeline);
                pass.set_bind_group(1, &ground.bind_group, &[]);
                pass.set_vertex_buffer(0, ground.vertices.slice(..));
                pass.set_index_buffer(ground.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..ground.index_count, 0, 0..1);
            }
            if !draws.is_empty() {
                pass.set_pipeline(&self.model_pipeline);
                pass.set_bind_group(1, &self.pose_bind_group, &[]);
                for ((mesh, level, texture), start, end, count) in &draws {
                    let Some(gpu) = self.meshes.get(mesh) else {
                        continue;
                    };
                    let group = self
                        .textures
                        .get(texture)
                        .map_or(&self.white, |(group, _)| group);
                    let (first, index_count) = gpu.levels[*level];
                    pass.set_bind_group(2, group, &[]);
                    pass.set_vertex_buffer(0, gpu.vertices.slice(..));
                    pass.set_vertex_buffer(1, self.instances.slice(*start..*end));
                    pass.set_index_buffer(gpu.indices.slice(..), wgpu::IndexFormat::Uint16);
                    pass.draw_indexed(first..first + index_count, 0, 0..*count);
                }
            }
            if !far_draws.is_empty() {
                pass.set_pipeline(&self.far.pipeline);
                for (key, start, end, count) in &far_draws {
                    let Some(look) = self.far.ready(*key) else {
                        continue;
                    };
                    pass.set_bind_group(1, &look.atlas.bind_group, &[]);
                    pass.set_vertex_buffer(0, self.far_instances.slice(*start..*end));
                    pass.draw(0..4, 0..*count);
                }
            }
            if self.ground.is_some() {
                pass.set_pipeline(&self.water_pipeline);
                pass.draw(0..4, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        self.show_stats(draws.len() + far_draws.len());
        true
    }
}

impl Field {
    fn show_stats(&mut self, draws: usize) {
        let stats = &mut self.stats;
        stats.frames += 1;
        let now = now();
        let span = now - stats.since;
        if span < FPS_INTERVAL_MS {
            return;
        }
        let rate = f64::from(stats.frames) * 1000.0 / span;
        let [high, medium, low, impostors] = stats.levels;
        let assets = self.store.borrow().stats();
        let stream = stats.stream;
        let mb = |bytes: u64| bytes as f64 / 1e6;
        let (held, held_bytes, waiting) = self.far.held();
        let seconds = span / 1000.0;
        let uploads = f64::from(std::mem::take(&mut self.far.uploads)) / seconds;
        let bakes = f64::from(std::mem::take(&mut self.far.bakes)) / seconds;
        stats.element.set_text_content(Some(&format!(
            "{rate:.0} FPS, {} in view: meshes {high}/{medium}/{low}, impostors {impostors}, {} models, {draws} draws\n\
             Impostor atlases: {held} held ({:.0}/{:.0} MB), {waiting} waiting for room; \
             {} from storage, {} drawn; {uploads:.1} uploads/s, {bakes:.1} bakes/s\n\
             Assets: {} files, {:.1} MB in memory; {:.1} MB downloaded, {:.1} MB from cache\n\
             Stream: {:.2} MB (live {:.2}, history {:.2}, terrain {:.2})",
            high + medium + low + impostors,
            stats.models,
            mb(held_bytes),
            mb(self.far.budget()),
            self.far.from_storage,
            self.far.drawn,
            assets.files,
            mb(assets.bytes),
            mb(assets.downloaded),
            mb(assets.cached),
            mb(stream.live + stream.segments + stream.terrain),
            mb(stream.live),
            mb(stream.segments),
            mb(stream.terrain),
        )));
        stats.frames = 0;
        stats.since = now;
    }
}

fn now() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
}

/// What every model of one entity shares this frame.
struct EntityState {
    dot: Dot,
    turn: u32,
    progress: f32,
    /// The animation name, lower case.
    state: String,
    /// Stands in for the engine's `rand`, so a unit keeps its choice from
    /// frame to frame.
    pick: u64,
    /// RGBA8 of the owner's displayed colour.
    color: u32,
    level: usize,
    /// A fixed time in ms for every animation, looping, instead of the
    /// one the stream gives; for impostor captures.
    time: Option<f32>,
}

/// `CObjectEntry::GetRandomAnimation(name, id)`: by frequency among those
/// of the name (and of the root's id, for props). An unknown name falls
/// back to idle, which the engine always has.
fn choose_animation<'a>(
    animations: &'a [ModelAnimation],
    state: &str,
    root_id: Option<&str>,
    pick: u64,
) -> Option<&'a ModelAnimation> {
    let candidates = |name: &str| -> Vec<&'a ModelAnimation> {
        let named: Vec<&ModelAnimation> = animations
            .iter()
            .filter(|animation| animation.name == name)
            .collect();
        let matching: Vec<&ModelAnimation> = named
            .iter()
            .copied()
            .filter(|animation| root_id.is_none_or(|id| id.is_empty() || animation.id == id))
            .collect();
        if matching.is_empty() { named } else { matching }
    };
    let mut list = candidates(state);
    if list.is_empty() {
        list = candidates("idle");
    }
    if list.is_empty() {
        return None;
    }
    let total: i64 = list
        .iter()
        .map(|animation| i64::from(animation.frequency.max(0)))
        .sum();
    if total == 0 {
        return Some(list[(pick % list.len() as u64) as usize]);
    }
    let mut choice = (pick % total as u64) as i64;
    for animation in &list {
        choice -= i64::from(animation.frequency.max(0));
        if choice < 0 {
            return Some(animation);
        }
    }
    list.last().copied()
}

/// `CUnitAnimation::Update` and `SetAnimationSyncOffset` worked out from
/// the turns the stream gives: the animation's time in ms, and whether it
/// loops.
fn animation_time(
    entity: &EntityState,
    chosen: &ModelAnimation,
    data: &AnimationData,
) -> (f32, bool) {
    let dot = &entity.dot;
    let duration = data.frames as f32 * data.frame_ms;
    if let Some(time) = entity.time {
        return (time.rem_euclid(duration.max(f32::EPSILON)), true);
    }
    let since =
        |turn: u32| (entity.turn.saturating_sub(turn) as f32 + entity.progress) * TURN_MS as f32;
    let time = if dot.sync_repeat > 0 && chosen.event >= 0.0 {
        let speed = duration / dot.sync_repeat as f32;
        let action = chosen.event * duration;
        let start =
            (action - dot.sync_offset as f32 * speed).rem_euclid(duration.max(f32::EPSILON));
        start + since(dot.sync_turn) * speed
    } else {
        // In 1/256: m/s for walk and run, else a multiplier.
        let speed = dot.speed as f32 / 256.0 * chosen.speed;
        since(dot.animation_turn) * speed
    };
    if dot.once {
        // "Very nearly the end of the last frame", as the engine holds it.
        (time.min(duration - 1.0).max(0.0), false)
    } else {
        (time.rem_euclid(duration.max(f32::EPSILON)), true)
    }
}

fn owner_color(dot: &Dot, palette: &[u32]) -> u32 {
    let color = usize::try_from(dot.owner)
        .ok()
        .and_then(|owner| palette.get(owner).copied())
        .unwrap_or(0xFFFF_FFFF);
    let [b, g, r, _] = color.to_le_bytes();
    u32::from_le_bytes([r, g, b, 0xFF])
}

/// A 1-based string table index; 0 is none.
fn string(strings: &[String], index: u32) -> Option<&str> {
    (index as usize)
        .checked_sub(1)
        .and_then(|index| strings.get(index))
        .map(String::as_str)
}

fn pose_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("poses"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(view),
        }],
    })
}

fn texture_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("field texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// BC levels go up as they are when the device takes them; otherwise, and
/// for RGBA8, as RGBA8.
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &Texture,
    bc_supported: bool,
) -> wgpu::TextureView {
    let bc_format = texture.format.bc().filter(|_| bc_supported);
    let format = match bc_format {
        Some(bc::Format::Bc1) => wgpu::TextureFormat::Bc1RgbaUnorm,
        Some(bc::Format::Bc2) => wgpu::TextureFormat::Bc2RgbaUnorm,
        Some(bc::Format::Bc3) => wgpu::TextureFormat::Bc3RgbaUnorm,
        None => wgpu::TextureFormat::Rgba8Unorm,
    };
    let gpu_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("skin"),
        size: wgpu::Extent3d {
            width: texture.width,
            height: texture.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: texture.levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, data) in texture.levels.iter().enumerate() {
        let (width, height) = level_extent(texture.width, texture.height, level as u32);
        let (bytes, bytes_per_row, extent) = match (bc_format, texture.format.bc()) {
            // Small mips still take whole 4x4 blocks.
            (Some(format), _) => (
                std::borrow::Cow::Borrowed(data.as_slice()),
                width.div_ceil(4) * format.block_size() as u32,
                (width.next_multiple_of(4), height.next_multiple_of(4)),
            ),
            (None, Some(format)) => match bc::decode(format, width, height, data) {
                Ok(rgba) => (std::borrow::Cow::Owned(rgba), width * 4, (width, height)),
                Err(_) => continue,
            },
            (None, None) => (
                std::borrow::Cow::Borrowed(data.as_slice()),
                width * 4,
                (width, height),
            ),
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &gpu_texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: extent.0,
                height: extent.1,
                depth_or_array_layers: 1,
            },
        );
    }
    gpu_texture.create_view(&Default::default())
}

/// Where the camera starts: the first player's civic centre, else the
/// middle of what the players own, else the map's centre. The centre alone
/// can be open sea.
fn start_point(scene: &Scene) -> [f32; 2] {
    let at = |dot: &Dot| {
        [
            dot.x as f32 / POSITION_UNITS_PER_METRE,
            dot.z as f32 / POSITION_UNITS_PER_METRE,
        ]
    };
    let template = |dot: &Dot| {
        scene
            .templates
            .get(dot.template as usize)
            .map_or("", String::as_str)
    };
    let centre = scene
        .dots
        .values()
        .filter(|dot| dot.owner > 0 && template(dot).contains("civil_centre"))
        .min_by_key(|dot| dot.owner);
    if let Some(dot) = centre {
        return at(dot);
    }
    let owned: Vec<[f32; 2]> = scene
        .dots
        .values()
        .filter(|dot| dot.owner > 0)
        .map(at)
        .collect();
    if owned.is_empty() {
        return [scene.map_size as f32 / 2.0; 2];
    }
    let count = owned.len() as f32;
    let sum = owned.iter().fold([0.0, 0.0], |sum, point| {
        [sum[0] + point[0], sum[1] + point[1]]
    });
    [sum[0] / count, sum[1] / count]
}

impl Camera {
    /// Moves the ground under the pointer along with it, `dx`, `dy` CSS
    /// pixels.
    fn pan(&mut self, dx: f32, dy: f32) {
        // About a pixel's worth of ground at the target per pixel.
        let scale = self.distance * 0.0015;
        let (sin, cos) = self.yaw.sin_cos();
        // look_at's right is (-cos, sin) and the view's forward over the
        // ground (-sin, -cos); the camera moves against the drag.
        self.target[0] += (dx * cos - dy * sin) * scale;
        self.target[1] += (-dx * sin - dy * cos) * scale;
    }

    fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Zooms keeping the ground under `(nx, ny)`, the point in normalized
    /// device coordinates, where it is on screen. Taken as level with the
    /// target, the point is the target plus the distance times a direction
    /// that does not depend on it, so moving the target by the change in
    /// distance along it keeps it still.
    fn zoom_at(&mut self, factor: f32, nx: f32, ny: f32, aspect: f32) {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        // The eye over the target per unit of distance, as view_projection.
        let eye = [cos_pitch * sin_yaw, sin_pitch, cos_pitch * cos_yaw];
        let forward = [-eye[0], -eye[1], -eye[2]];
        // look_at's right and up for this forward.
        let right = {
            let (x, z) = (forward[2], -forward[0]);
            let length = x.hypot(z).max(f32::EPSILON);
            [x / length, 0.0, z / length]
        };
        let up = [
            forward[1] * right[2] - forward[2] * right[1],
            forward[2] * right[0] - forward[0] * right[2],
            forward[0] * right[1] - forward[1] * right[0],
        ];
        let half = (FIELD_OF_VIEW / 2.0).tan();
        let ray: [f32; 3] =
            std::array::from_fn(|i| forward[i] + right[i] * nx * aspect * half + up[i] * ny * half);
        let before = self.distance;
        self.zoom(factor);
        // Above the horizon the cursor is over no ground.
        if ray[1] >= -f32::EPSILON {
            return;
        }
        let t = -eye[1] / ray[1];
        let mut point = [eye[0] + ray[0] * t, eye[2] + ray[2] * t];
        let length = point[0].hypot(point[1]);
        if length > ZOOM_ANCHOR_MAX {
            point = [
                point[0] * ZOOM_ANCHOR_MAX / length,
                point[1] * ZOOM_ANCHOR_MAX / length,
            ];
        }
        let moved = before - self.distance;
        self.target[0] += point[0] * moved;
        self.target[1] += point[1] * moved;
    }

    /// Moves the camera by the pan keys held since the last frame.
    fn step(&mut self, now: f64) {
        let seconds = (((now - self.last_step) / 1000.0) as f32).clamp(0.0, KEY_STEP_MAX_S);
        self.last_step = now;
        if self.keys == 0 {
            return;
        }
        let axis = |plus: u8, minus: u8| {
            f32::from(u8::from(self.keys & plus != 0)) - f32::from(u8::from(self.keys & minus != 0))
        };
        let (ahead, side) = (axis(KEY_FORWARD, KEY_BACK), axis(KEY_RIGHT, KEY_LEFT));
        let metres = self.distance * KEY_PAN_SPEED * seconds;
        let (sin, cos) = self.yaw.sin_cos();
        // As `pan`: forward over the ground is (-sin, -cos), right (-cos, sin).
        self.target[0] += (-ahead * sin - side * cos) * metres;
        self.target[1] += (-ahead * cos + side * sin) * metres;
    }

    /// One pointer pans (or, with a mouse's other buttons, turns and tilts);
    /// two pan by their midpoint, pinch to zoom and twist to turn.
    fn pointer_moved(&mut self, id: i32, x: f32, y: f32) {
        let Some(index) = self.pointers.iter().position(|pointer| pointer.id == id) else {
            return;
        };
        if self.pointers.len() >= 2 {
            let other = &self.pointers[if index == 0 { 1 } else { 0 }];
            let (ox, oy) = (other.x, other.y);
            let before = &self.pointers[index];
            let span = |ax: f32, ay: f32| (ax - ox).hypot(ay - oy).max(1.0);
            let angle = |ax: f32, ay: f32| (ay - oy).atan2(ax - ox);
            let zoom = span(before.x, before.y) / span(x, y);
            let mut twist = angle(x, y) - angle(before.x, before.y);
            if twist > std::f32::consts::PI {
                twist -= std::f32::consts::TAU;
            } else if twist < -std::f32::consts::PI {
                twist += std::f32::consts::TAU;
            }
            // The midpoint moves by half of this pointer's move.
            let (dx, dy) = ((x - before.x) / 2.0, (y - before.y) / 2.0);
            self.zoom(zoom);
            // Turning the yaw up turns the ground anticlockwise on screen;
            // the ground follows the fingers.
            self.yaw -= twist;
            self.pan(dx, dy);
        } else {
            let pointer = &self.pointers[index];
            let (dx, dy) = (x - pointer.x, y - pointer.y);
            if pointer.button == 0 {
                self.pan(dx, dy);
            } else {
                self.yaw -= dx * ROTATE_SPEED;
                self.pitch = (self.pitch + dy * ROTATE_SPEED).clamp(0.2, 1.5);
            }
        }
        let pointer = &mut self.pointers[index];
        pointer.x = x;
        pointer.y = y;
    }
}

/// `pointerId`, read through the base `MouseEvent` a `PointerEvent` is.
fn pointer_id(event: &MouseEvent) -> i32 {
    js_sys::Reflect::get(event, &"pointerId".into())
        .ok()
        .and_then(|id| id.as_f64())
        .map_or(0, |id| id as i32)
}

/// Mouse, touch and pen through pointer events: drag to pan, the right
/// mouse button to turn and tilt, two fingers to pinch and twist, the wheel
/// to zoom. The handlers live as long as the page.
fn wire_camera(canvas: &HtmlCanvasElement, camera: &Rc<RefCell<Camera>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let listen =
        |target: &web_sys::EventTarget, name: &str, handler: Closure<dyn FnMut(MouseEvent)>| {
            let _ = target.add_event_listener_with_callback(name, handler.as_ref().unchecked_ref());
            handler.forget();
        };

    let target = Rc::clone(camera);
    listen(
        canvas,
        "pointerdown",
        Closure::new(move |event: MouseEvent| {
            let camera = &mut *target.borrow_mut();
            let id = pointer_id(&event);
            camera.pointers.retain(|pointer| pointer.id != id);
            camera.pointers.push(Pointer {
                id,
                button: event.button(),
                x: event.client_x() as f32,
                y: event.client_y() as f32,
            });
        }),
    );

    // The right button turns the camera rather than open a menu.
    listen(
        canvas,
        "contextmenu",
        Closure::new(|event: MouseEvent| event.prevent_default()),
    );

    // On the window, so a drag that leaves the canvas still ends.
    let target = Rc::clone(camera);
    listen(
        &window,
        "pointermove",
        Closure::new(move |event: MouseEvent| {
            target.borrow_mut().pointer_moved(
                pointer_id(&event),
                event.client_x() as f32,
                event.client_y() as f32,
            );
        }),
    );
    for name in ["pointerup", "pointercancel"] {
        let target = Rc::clone(camera);
        listen(
            &window,
            name,
            Closure::new(move |event: MouseEvent| {
                let id = pointer_id(&event);
                target
                    .borrow_mut()
                    .pointers
                    .retain(|pointer| pointer.id != id);
            }),
        );
    }

    let target = Rc::clone(camera);
    let wheel_canvas = canvas.clone();
    let on_wheel = Closure::<dyn FnMut(WheelEvent)>::new(move |event: WheelEvent| {
        event.prevent_default();
        let (width, height) = (
            wheel_canvas.client_width().max(1) as f32,
            wheel_canvas.client_height().max(1) as f32,
        );
        let nx = event.offset_x() as f32 / width * 2.0 - 1.0;
        let ny = 1.0 - event.offset_y() as f32 / height * 2.0;
        target.borrow_mut().zoom_at(
            (event.delta_y() * 0.001).exp() as f32,
            nx,
            ny,
            width / height,
        );
    });
    canvas.set_onwheel(Some(on_wheel.as_ref().unchecked_ref()));
    on_wheel.forget();

    // On the window, as the canvas takes no focus; not while a control
    // has it, where the arrows pick an option.
    for (name, down) in [("keydown", true), ("keyup", false)] {
        let target = Rc::clone(camera);
        let handler = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
            let text = |object: &JsValue, name: &str| {
                js_sys::Reflect::get(object, &name.into())
                    .ok()
                    .and_then(|value| value.as_string())
                    .unwrap_or_default()
            };
            let flag = |name: &str| {
                js_sys::Reflect::get(&event, &name.into()).is_ok_and(|value| value.is_truthy())
            };
            let bit = key_bit(&text(&event, "code"));
            if bit == 0 {
                return;
            }
            let camera = &mut *target.borrow_mut();
            if !down {
                camera.keys &= !bit;
                return;
            }
            let control = event.target().is_some_and(|element| {
                matches!(
                    text(&element, "tagName").as_str(),
                    "INPUT" | "SELECT" | "TEXTAREA"
                )
            });
            if control || flag("ctrlKey") || flag("metaKey") || flag("altKey") {
                return;
            }
            event.prevent_default();
            camera.keys |= bit;
        });
        let _ = window.add_event_listener_with_callback(name, handler.as_ref().unchecked_ref());
        handler.forget();
    }
    // A key let go in another window never sends its keyup here.
    let target = Rc::clone(camera);
    let on_blur = Closure::<dyn FnMut(Event)>::new(move |_: Event| {
        target.borrow_mut().keys = 0;
    });
    let _ = window.add_event_listener_with_callback("blur", on_blur.as_ref().unchecked_ref());
    on_blur.forget();
    Ok(())
}
