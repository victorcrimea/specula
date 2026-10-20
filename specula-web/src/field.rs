//! The observer page's 3D view: the map's heightmap in its terrain
//! textures (blend.rs) with a water plane, and every entity with its actor's models, variant and
//! animation as the stream describes them. Pieces stream in on demand
//! (assets.rs); an entity shows once its own pieces have arrived.
//!
//! Models are skinned on the GPU from the pose atlas (pose.rs) and drawn
//! instanced, one draw per mesh, level of detail and texture. Props are
//! placed on the CPU, on their parent's bone at the current time. Meshes
//! and textures start as their coarse files; the detail files are asked
//! for only for entities drawn finer than the coarsest level. Entities
//! smaller still are impostors (far.rs) once their look's atlas is ready.
//! Decals (a field's soil, the dirt under a building) are drawn over the
//! ground's own triangles, at every distance.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::rc::Rc;

use specula_assets::bc;
use specula_assets::files::AnimationData;
use specula_assets::files::Decal;
use specula_assets::files::Emitter;
use specula_assets::files::FileRef;
use specula_assets::files::Mesh;
use specula_assets::files::NO_BONE;
use specula_assets::files::PropPoint;
use specula_assets::files::Sinking;
use specula_assets::files::TEXTURE_TIER_SIDES;
use specula_assets::files::TerrainTexture;
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

use crate::AnimationSound;
use crate::Listener;
use crate::StreamBytes;
use crate::TURN_MS;
use crate::View;
use crate::assets;
use crate::assets::Get;
use crate::assets::Progress;
use crate::assets::Store;
use crate::bench::BenchInfo;
use crate::bench::CameraPose;
use crate::blend;
use crate::blend::Splat;
use crate::far;
use crate::far::Capture;
use crate::far::CaptureDraw;
use crate::far::Far;
use crate::far::LookKey;
use crate::far::Request;
use crate::gpu;
use crate::impostor;
use crate::overlay::Mark;
use crate::overlay::OverlayShape;
use crate::overlay::Overlays;
use crate::particles;
use crate::particles::Owner;
use crate::particles::Particles;
use crate::pick;
use crate::pick::OrientedBox;
use crate::pick::Pickable;
use crate::pose;
use crate::postproc;
use crate::postproc::Post;
use crate::postproc::Upscale;
use crate::projectile;
use crate::projectile::Flight;
use crate::selection::Lookup;
use crate::selection::Selectee;
use crate::selection::Selection;
use crate::shadow;
use crate::shadow::Shadow;
use crate::sky::Sky;
use crate::stream::Dot;
use crate::stream::RangeSet;
use crate::stream::Scene;
use crate::stream::Shot;
use crate::stream::TemplateDetails;
use crate::stream::VariableSet;
use crate::stream::VariantSet;
use crate::stream::turn_towards;
use crate::terrain::Light;
use crate::terrain::TILE_SIZE;
use crate::terrain::TerrainImage;
use crate::territory;
use crate::territory::Borders;
use crate::timing;
use crate::timing::Timer;
use crate::unit::DEPTH_FORMAT;
use crate::unit::create_buffer;
use crate::unit::cross;
use crate::unit::frustum_planes;
use crate::unit::look_at;
use crate::unit::multiply;
use crate::variation;
use crate::variation::Model;
use crate::variation::ModelAnimation;
use crate::variation::Shape;
use crate::water;
use crate::water::Normals;
use crate::water::Water;
use crate::worker::PEAK_WINDOW_S;

/// The stream's positions are in 1/16 m.
const POSITION_UNITS_PER_METRE: f32 = 16.0;
/// Matrix, sun, misc, water, camera right, up and eye; far.wgsl `View`,
/// of which model.wgsl and field.wgsl read the first four. The rest of the
/// light is shadow.rs's.
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
/// The light until the map's arrives.
const AMBIENT: f32 = 0.45;
const SUN_STRENGTH: f32 = 0.75;
/// Vertical, the engine's view.fov of 45 degrees.
const FIELD_OF_VIEW: f32 = 45.0 * std::f32::consts::PI / 180.0;
const NEAR: f32 = 1.0;
const FAR: f32 = 4000.0;
const MIN_DISTANCE: f32 = 10.0;
const MAX_DISTANCE: f32 = 1500.0;
/// The engine's view.zoom.default. Its FocusHeight keeps the eye this far
/// along the view from the ground, as here on level ground.
const START_DISTANCE: f32 = 120.0;
/// The engine's view.rotate.x.default of 35 degrees.
const START_PITCH: f32 = 35.0 * std::f32::consts::PI / 180.0;
const MIN_PITCH: f32 = 0.2;
const MAX_PITCH: f32 = 1.5;
/// The engine's bird's eye view looks straight down; `look_at` has no
/// right for that, so a hair short of it, which keeps the yaw.
const BIRDS_EYE_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.001;
/// The eye south of the target, looking north (+z) as the engine's camera
/// does at its default view.rotate.y.default of 0.
const START_YAW: f32 = std::f32::consts::PI;
/// Ground crossed per second by the pan keys, in camera distances.
const KEY_PAN_SPEED: f32 = 1.0;
/// Radians per second by the turn keys, the engine's view.rotate.y.speed.
const KEY_ROTATE_SPEED: f32 = 2.0;
/// Radians per second by the tilt keys, the engine's view.rotate.x.speed.
const KEY_TILT_SPEED: f32 = 1.2;
/// Radians per Shift+wheel step or X1/X2 press, the engine's
/// view.rotate.y.speed.wheel.
const WHEEL_ROTATE_STEP: f32 = 0.45;
/// The engine's view.rotate.y.smoothness, view.rotate.x.smoothness and
/// view.zoom.smoothness: the share of a stepped change still to go after a
/// tenth of a second, as `CSmoothedValue::Update`.
const YAW_SMOOTHNESS: f32 = 0.3;
const PITCH_SMOOTHNESS: f32 = 0.5;
const DISTANCE_SMOOTHNESS: f32 = 0.4;
/// `MouseEvent.buttons` bit of the middle button, the engine's camera.pan.
const BUTTONS_MIDDLE: u16 = 4;
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
const MESH_ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    10 => Float32x2,
    11 => Snorm8x4,
    3 => Uint8x4,
    4 => Unorm8x4,
];
/// A model material's uniform: `effectSettings`, then 1 when it has a
/// normal map, 1 when the wind sways it and its `windData`; model.wgsl
/// `Material`.
const MATERIAL_SIZE: usize = 32;
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    5 => Float32x4,
    6 => Float32x4,
    7 => Float32x4,
    8 => Uint32x4,
    9 => Float32x4,
];
const TERRAIN_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];
/// The ground's position and normal; decal.wgsl.
const DECAL_GROUND_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
/// `PlacedDecal::place`, two f32x4; decal.wgsl.
const DECAL_INSTANCE_SIZE: usize = 32;
const DECAL_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4];
/// The anisotropy terrain textures are sampled with, as the engine's
/// default for them; seen at a slant, they would blur without it.
const TERRAIN_ANISOTROPY: u16 = 16;

/// (actor, variant set, animation, seed) as the stream indexes them: what
/// decides an entity's variation.
type ModelKey = (u32, u32, u32, u32);
/// The level count and first uploaded level of a texture on the GPU: a
/// tier of detail adds levels, the texture quality moves the first.
type Uploaded = (usize, usize);

/// (mesh, level of detail, material) of one instanced draw.
type GroupKey = (u64, usize, u64);
const SHOOTING_ANIMATION: &str = "attack_ranged";
/// What a shot's model resolves from: its projectile actor, else the
/// shooter's actor, variant set and seed.
type ShotModelKey = (u32, u32, u32, u32);

/// A projectile the view draws, with what it works out once.
struct Flying {
    shot: Shot,
    /// The shooter's ammo prop at the launch, when it could be posed then.
    origin: Option<Vec3>,
    /// Worked out when first drawn: the flight, when it lands and the
    /// ground height there; `Some(None)` for one that cannot fly.
    flight: Option<Option<(Flight, f32, f32)>>,
}

/// A decal laid this frame.
struct PlacedDecal {
    /// The texture's coarse file hash.
    texture: u64,
    /// `CModelDecal`'s transform (cos and sin of its Y rotation, x, z),
    /// then the decal's offset x and z and size x and z.
    place: [f32; 8],
}

/// A texture on the GPU: its view, its bind group for the decal and blend
/// passes, what it was uploaded from, and which upload it was, which the
/// materials made from it compare.
struct GpuTexture {
    view: wgpu::TextureView,
    group: wgpu::BindGroup,
    uploaded: Uploaded,
    upload: u64,
}

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
    base: Vec<Splat>,
    blends: Vec<Splat>,
    /// `water::surface`.
    water_vertices: wgpu::Buffer,
    water_indices: wgpu::Buffer,
    water_index_count: u32,
    shore_vertices: wgpu::Buffer,
    shore_indices: wgpu::Buffer,
    shore_index_count: u32,
    /// `water::waves`.
    wave_vertices: wgpu::Buffer,
    wave_indices: wgpu::Buffer,
    wave_index_count: u32,
    /// The minimap picture, for a terrain whose texture is not in (yet).
    minimap: wgpu::BindGroup,
    /// Per palette entry, its textures' bind group and what its colour,
    /// normal and specular maps were uploaded from (`Uploaded`, `None`
    /// for a map it is drawn without); `None` while the colour map is on
    /// its way or missing.
    textures: Vec<Option<(wgpu::BindGroup, [Option<Uploaded>; 3])>>,
    /// Palette entries whose texture will not come.
    missing: Vec<bool>,
    alpha_maps: Option<wgpu::BindGroup>,
    /// The lowest and highest ground, in metres, for the shadow map's
    /// depth range.
    low: f32,
    high: f32,
}

struct Pointer {
    id: i32,
    /// A mouse pans only with the middle button held, as the left and
    /// right ones select and give orders.
    mouse: bool,
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
    /// The engine's m_BirdsEyeView: looking straight down, the tilt keys
    /// idle.
    birds_eye: bool,
    /// Whether the attack, aura and heal range circles show, as the
    /// engine's `gui.session.attackrange`, `aurasrange` and `healrange`,
    /// all on by default.
    ranges: [bool; 3],
    /// What the stepped controls (Shift+wheel, X1/X2, reset, bird's eye)
    /// have yet to add to `yaw`, `pitch` and `distance`; apart from them, so
    /// held keys and drags still act at once, as the engine's `Add` beside
    /// its `AddSmoothly`.
    yaw_ahead: f32,
    pitch_ahead: f32,
    distance_ahead: f32,
    /// Mice, fingers and pens down on the canvas.
    pointers: Vec<Pointer>,
    /// Set over the map's centre once the first keyframe tells its size.
    placed: bool,
    /// Pan and turn keys held, `KEY_*` bits.
    keys: u8,
    /// When `step` last ran, in page milliseconds.
    last_step: f64,
    /// The left button's press, its pointer id and client point, and the
    /// canvas's client offset then.
    press: Option<(i32, [f32; 2], [f32; 2])>,
    /// Left clicks and drags yet to be picked from.
    picks: Vec<PickInput>,
    /// The mouse over the canvas, 0..1 across it from its top left.
    hover: Option<[f32; 2]>,
    /// The left button's drag past `DRAG_DELTA` while it is held.
    band: Option<Band>,
}

/// A left click or drag on the canvas, points 0..1 across it from its top
/// left, with the engine's selection.add (Shift) and selection.remove
/// (Ctrl) held or not; or selection.cancel (Escape).
enum PickInput {
    Click {
        at: [f32; 2],
        count: u32,
        add: bool,
        remove: bool,
    },
    Rect {
        from: [f32; 2],
        to: [f32; 2],
        add: bool,
        remove: bool,
    },
    Clear,
}

/// A bandbox being drawn: its corners 0..1 across the canvas, and in
/// client pixels for the page's rectangle.
#[derive(Clone, Copy)]
struct Band {
    from: [f32; 2],
    to: [f32; 2],
    client_from: [f32; 2],
    client_to: [f32; 2],
}

/// `CCmpSelectable`'s `RGB_DESATURATION`: how far an unselected always
/// visible overlay's colour moves towards its largest channel.
const RGB_DESATURATION: f32 = 0.333_333;

/// `gui.session.dragdelta`: how far, in pixels, a press may move and still
/// be a click.
const DRAG_DELTA: f32 = 4.0;

const KEY_FORWARD: u8 = 1;
const KEY_BACK: u8 = 2;
const KEY_LEFT: u8 = 4;
const KEY_RIGHT: u8 = 8;
const KEY_TURN_CW: u8 = 16;
const KEY_TURN_CCW: u8 = 32;
const KEY_TILT_UP: u8 = 64;
const KEY_TILT_DOWN: u8 = 128;

/// The camera key bit of a `KeyboardEvent.code`: by position, so WASD sits under
/// the same fingers on any layout.
fn key_bit(code: &str, ctrl: bool) -> u8 {
    match code {
        // The engine's camera.rotate.up and camera.rotate.down; its Ctrl+W
        // would close the tab.
        "ArrowUp" if ctrl => KEY_TILT_UP,
        "ArrowDown" if ctrl => KEY_TILT_DOWN,
        _ if ctrl => 0,
        "KeyW" | "ArrowUp" => KEY_FORWARD,
        "KeyS" | "ArrowDown" => KEY_BACK,
        "KeyA" | "ArrowLeft" => KEY_LEFT,
        "KeyD" | "ArrowRight" => KEY_RIGHT,
        // The engine's camera.rotate.cw and camera.rotate.ccw.
        "KeyQ" => KEY_TURN_CW,
        "KeyE" => KEY_TURN_CCW,
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
    templates: Vec<String>,
    /// By template, as `templates`.
    template_details: Vec<Option<TemplateDetails>>,
    strings: Vec<String>,
    variant_sets: Vec<VariantSet>,
    variable_sets: Vec<VariableSet>,
    range_sets: Vec<RangeSet>,
}

pub struct Field {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// The scene's target and depth when post-processing, and the passes.
    post: Post,
    bc: bool,
    scene_sources: SceneSources,
    /// The ground's and models' pipelines for the view.
    scene: ScenePipelines,
    /// The viewer's material maps setting: normal, specular and ambient
    /// occlusion maps fetched and drawn.
    material_maps: bool,
    /// The viewer's triplanar setting: the terrains whose material asks
    /// for it mapped triplanar rather than planar.
    triplanar: bool,
    /// The viewer's wind setting: `USE_WIND` materials sway.
    wind: bool,
    /// The viewer's post-processing settings: the map's effect at all,
    /// then its bloom and depth of field, FXAA, and CAS with its
    /// sharpness.
    postproc: bool,
    bloom: bool,
    dof: bool,
    antialiasing: bool,
    sharpening: bool,
    sharpness: f32,
    /// `renderer.scale` and `renderer.upscale.technique`, which apply
    /// only with post-processing on.
    render_scale: f32,
    upscale: Upscale,
    /// The viewer's fog setting.
    fog: bool,
    /// The viewer's water effects setting: the engine's fancy water,
    /// where off keeps the flat plane.
    water_effects: bool,
    /// The flat plane.
    water_pipeline: wgpu::RenderPipeline,
    water: Water,
    /// The map's water type's frames, while water effects are on.
    water_normals: Option<Normals>,
    /// `water`'s bind group once `water_normals` is complete.
    water_group: Option<wgpu::BindGroup>,
    /// The viewer's sky setting: the map's sky drawn behind everything,
    /// where off keeps the plain background.
    sky_visible: bool,
    sky: Sky,
    /// The viewer's particles setting, the emitters, the pipelines for
    /// the view and for the water reflection, and this frame's instances.
    particles_visible: bool,
    particles: Particles,
    particle_pipelines: particles::Pipelines,
    reflection_particles: particles::Pipelines,
    particle_instances: wgpu::Buffer,
    /// What `place_model` places the particle props it meets for: the
    /// entity or projectile being drawn, not a caster or a capture.
    emitting: Option<Owner>,
    /// The viewer's water refraction, real water depth and water
    /// reflections settings, under the water effects.
    refraction: bool,
    real_depth: bool,
    reflection: bool,
    /// The ground's and models' pipelines for the refraction and the
    /// reflection passes, made when each is first drawn.
    refraction_scene: Option<ScenePipelines>,
    reflection_scene: Option<ScenePipelines>,
    /// The refraction and the reflection passes' views.
    refraction_view: (wgpu::Buffer, wgpu::BindGroup),
    reflection_view: (wgpu::Buffer, wgpu::BindGroup),
    /// Those passes' textures while either is drawn, and a count of
    /// their changes.
    water_targets: Option<water::Targets>,
    water_targets_generation: u64,
    /// The viewer's high-quality water effects setting, under the water
    /// effects, its pipelines, and its textures while it is drawn.
    fancy_water: bool,
    waves: water::Waves,
    water_effects_targets: Option<water::Effects>,
    /// The waves' group once their textures are in.
    waves_group: Option<wgpu::BindGroup>,
    /// `water`'s environment group, with the sky's and the targets'
    /// generations it was made from; the targets' counts the fancy
    /// effects' changes too.
    water_environment: Option<(u64, u64, wgpu::BindGroup)>,
    decal_pipeline: wgpu::RenderPipeline,
    shadow: Shadow,
    borders: Borders,
    /// Selection rings, stars and outlines.
    overlays: Overlays,
    ground_layout: wgpu::BindGroupLayout,
    /// Repeating, for the terrain textures.
    terrain_sampler: wgpu::Sampler,
    view_buffer: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    poses: pose::Atlas,
    pose_layout: wgpu::BindGroupLayout,
    pose_bind_group: wgpu::BindGroup,
    pose_generation: u32,
    texture_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// What stands in for a material's missing colour, normal, specular
    /// and ambient occlusion maps: white, flat, black and white.
    default_maps: [wgpu::TextureView; 4],
    /// For a material not made yet.
    default_material: wgpu::BindGroup,
    ground: Option<Ground>,
    store: Rc<RefCell<Store>>,
    meshes: HashMap<u64, GpuMesh>,
    /// By coarse file hash.
    textures: HashMap<u64, GpuTexture>,
    /// Textures uploaded so far.
    uploads: u64,
    /// By `material`'s key, with the uploads of its textures it was made
    /// from.
    materials: HashMap<u64, (wgpu::BindGroup, [u64; 4])>,
    /// How many of `TEXTURE_TIER_SIDES` textures are fetched and drawn
    /// with: the viewer's texture quality, at most medium without BC.
    texture_tiers: usize,
    entities: Entities,
    palette: Vec<u32>,
    models: HashMap<ModelKey, Option<Rc<Model>>>,
    /// `tree_bounds`' pieces, once every piece of the tree has arrived.
    bounds: HashMap<ModelKey, Vec<(Vec3, Vec3)>>,
    camera: Rc<RefCell<Camera>>,
    instances: wgpu::Buffer,
    groups: HashMap<GroupKey, Vec<u8>>,
    /// Each cascade's shadow casters, as `CULL_SHADOWS_CASCADE_0` and on
    /// gather them: the models drawn whose bounds touch its box, at their
    /// level, and those out of view or drawn as impostors, at the coarse
    /// one.
    shadow_instances: [wgpu::Buffer; shadow::CASCADES],
    shadow_groups: [HashMap<GroupKey, Vec<u8>>; shadow::CASCADES],
    /// This frame's planes of each cascade's box, empty without shadows.
    shadow_planes: Vec<[[f32; 4]; 6]>,
    /// A bit per cascade the model being placed casts into.
    casting: u8,
    /// Model instances placed this frame, captures included.
    placed: u32,
    far: Far,
    far_instances: wgpu::Buffer,
    far_groups: HashMap<LookKey, Vec<u8>>,
    decals: Vec<PlacedDecal>,
    decal_instances: wgpu::Buffer,
    /// Into the ground's vertices, per frame.
    decal_indices: wgpu::Buffer,
    /// Where the camera is this frame, which impostor cells depend on.
    eye: [f32; 3],
    /// The selectable entities drawn this frame.
    pickables: Vec<Pickable>,
    selection: Selection,
    /// Handed over before the scene they belong to.
    renames: Vec<(u32, u32)>,
    /// The page's bandbox rectangle, if it has one.
    bandbox: Option<HtmlElement>,
    /// When the overlays last faded, in page milliseconds.
    last_fade: f64,
    /// The animation time each entity's sound was last checked at.
    sound_clocks: HashMap<u32, SoundClock>,
    shots: HashMap<u32, Flying>,
    shot_models: HashMap<ShotModelKey, Option<Rc<Model>>>,
    /// By 1-based string index of the impact actor.
    impact_models: HashMap<u32, Option<Rc<Model>>>,
    /// Whether projectiles are drawn at all.
    projectiles: bool,
    device_pixel_ratio: f32,
    stats: Stats,
    timer: Timer,
    /// This frame's model and ground draw calls, over all passes.
    draw_calls: Cell<u32>,
}

/// Where an entity's animation was when its sound point was last looked
/// for, as `CUnitAnimation` keeps `pastSoundPos` per animation.
struct SoundClock {
    /// (animation, animation turn, sync turn, animation file): another
    /// value is another animation, whose sound point is due again.
    key: (u32, u32, u32, u64),
    /// Its time in ms, not wrapped at the end of a loop.
    time: f32,
}

/// What the `#fps` box shows, as on the unit page, and the debug panes
/// under it, which the I key cycles through.
struct Stats {
    element: HtmlElement,
    network: HtmlElement,
    profiling: HtmlElement,
    storage: HtmlElement,
    /// Frames drawn since `since`, in page milliseconds.
    frames: u32,
    since: f64,
    /// This frame's model instances, and its entities drawn at each level
    /// of detail, the full mesh first, impostors last.
    models: u32,
    levels: [u32; LEVELS],
    stream: StreamBytes,
    backend: &'static str,
    adapter: String,
}

impl Field {
    pub async fn new(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let gpu::Gpu {
            surface,
            device,
            queue,
            config,
            backend,
            adapter,
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
        let refraction_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water refraction view"),
            size: VIEW_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let refraction_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water refraction view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: refraction_buffer.as_entire_binding(),
            }],
        });
        let reflection_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water reflection view"),
            size: VIEW_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let reflection_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water reflection view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: reflection_buffer.as_entire_binding(),
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

        let material_texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model material"),
            entries: &[
                material_texture(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                material_texture(2),
                material_texture(3),
                material_texture(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    // The wind sways the vertices.
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let ground_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ground texture"),
            entries: &[
                material_texture(0),
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
                material_texture(3),
                material_texture(4),
            ],
        });
        let terrain_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: TERRAIN_ANISOTROPY,
            ..Default::default()
        });

        let depth_stencil = |write: bool| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        };
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
        let terrain_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: blend::VERTEX_SIZE as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &TERRAIN_ATTRIBUTES,
        })];
        let shadow = Shadow::new(
            &device,
            &view_layout,
            VIEW_SIZE,
            &pose_layout,
            &material_layout,
            &model_shader,
            &model_buffers,
            &field_shader,
            &terrain_buffers,
        );
        let light_layout = &shadow.layout;
        let model_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model"),
            bind_group_layouts: &[
                Some(&view_layout),
                Some(&pose_layout),
                Some(&material_layout),
                Some(light_layout),
            ],
            immediate_size: 0,
        });
        let terrain_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain"),
            // Group 2 is the blend pass's masks.
            bind_group_layouts: &[
                Some(&view_layout),
                Some(&ground_layout),
                None,
                Some(light_layout),
            ],
            immediate_size: 0,
        });
        let blend_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain blend"),
            bind_group_layouts: &[
                Some(&view_layout),
                Some(&ground_layout),
                Some(&texture_layout),
                Some(light_layout),
            ],
            immediate_size: 0,
        });
        let scene_sources = SceneSources {
            field_shader: field_shader.clone(),
            model_shader: model_shader.clone(),
            terrain_layout,
            blend_layout,
            model_layout,
            format: config.format,
        };
        let scene = ScenePipelines::new(&device, &scene_sources, WaterClip::None);
        let water_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water"),
            bind_group_layouts: &[Some(&view_layout), None, None, Some(light_layout)],
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
        let decal_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("decal"),
            source: wgpu::ShaderSource::Wgsl(include_str!("decal.wgsl").into()),
        });
        let decal_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("decal"),
            bind_group_layouts: &[
                Some(&view_layout),
                Some(&texture_layout),
                Some(light_layout),
            ],
            immediate_size: 0,
        });
        let decal_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("decal"),
            layout: Some(&decal_layout),
            vertex: wgpu::VertexState {
                module: &decal_shader,
                entry_point: Some("vs_decal"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: blend::VERTEX_SIZE as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &DECAL_GROUND_ATTRIBUTES,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: DECAL_INSTANCE_SIZE as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &DECAL_ATTRIBUTES,
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState::default(),
            // shaders/effects/terrain_decal.xml: over the ground at its
            // very depth, without writing it.
            depth_stencil: Some(wgpu::DepthStencilState {
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                ..depth_stencil(false)
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &decal_shader,
                entry_point: Some("fs_decal"),
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

        let borders = Borders::new(&device, &queue, config.format, &view_layout)?;
        let overlays = Overlays::new(&device, &queue, config.format, &view_layout);
        let sky = Sky::new(&device, &queue, config.format);
        let particle_pipelines = particles::Pipelines::new(
            &device,
            config.format,
            &view_layout,
            &texture_layout,
            light_layout,
            false,
        );
        let particle_instances = create_buffer(
            &device,
            &queue,
            "particle instances",
            &[0; 4096],
            wgpu::BufferUsages::VERTEX,
        );
        let reflection_particles = particles::Pipelines::new(
            &device,
            config.format,
            &view_layout,
            &texture_layout,
            light_layout,
            true,
        );
        let water = Water::new(&device, config.format, &view_layout, light_layout);
        let waves = water::Waves::new(&device, &view_layout);
        let far = Far::new(
            &device,
            config.format,
            &model_shader,
            &model_buffers,
            &view_layout,
            &pose_layout,
            &material_layout,
            light_layout,
            START_PITCH,
        );
        let poses = pose::Atlas::new(&device);
        let pose_bind_group = pose_bind_group(&device, &pose_layout, &poses.view);
        let pose_generation = poses.generation;
        let default_maps = [
            [0xFF, 0xFF, 0xFF, 0xFF],
            [0x80, 0x80, 0xFF, 0xFF],
            [0, 0, 0, 0],
            [0xFF, 0xFF, 0xFF, 0xFF],
        ]
        .map(|texel| {
            let texture = Texture {
                format: TextureFormat::Rgba8,
                width: 1,
                height: 1,
                levels: vec![texel.to_vec()],
                details: [None; 3],
            };
            upload_texture(&device, &queue, &texture, 0, false)
        });
        let default_material = material_bind_group(
            &device,
            &queue,
            &material_layout,
            [0, 1, 2, 3].map(|slot| &default_maps[slot]),
            &sampler,
            [0.0; 4],
            [0.0; 4],
        );
        let post = Post::new(&device, config.format, config.width, config.height);
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
        let decal_instances = create_buffer(
            &device,
            &queue,
            "decal instances",
            &[],
            wgpu::BufferUsages::VERTEX,
        );
        let decal_indices = create_buffer(
            &device,
            &queue,
            "decal indices",
            &[],
            wgpu::BufferUsages::INDEX,
        );
        let shadow_instances = std::array::from_fn(|_| {
            create_buffer(
                &device,
                &queue,
                "shadow caster instances",
                &[],
                wgpu::BufferUsages::VERTEX,
            )
        });
        let camera = Rc::new(RefCell::new(Camera {
            target: [0.0, 0.0],
            yaw: START_YAW,
            pitch: START_PITCH,
            distance: START_DISTANCE,
            birds_eye: false,
            ranges: [true; 3],
            yaw_ahead: 0.0,
            pitch_ahead: 0.0,
            distance_ahead: 0.0,
            pointers: Vec::new(),
            placed: false,
            keys: 0,
            last_step: now(),
            press: None,
            picks: Vec::new(),
            hover: None,
            band: None,
        }));
        wire_camera(&canvas, &camera)?;
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or("no document")?;
        let timer = Timer::new(&device, &queue, backend == "WebGPU");
        let stats = Stats {
            element: crate::element(&document, "fps")?,
            network: crate::element(&document, "stats-network")?,
            profiling: crate::element(&document, "stats-profiling")?,
            storage: crate::element(&document, "stats-storage")?,
            frames: 0,
            since: now(),
            models: 0,
            levels: [0; LEVELS],
            stream: StreamBytes::default(),
            backend,
            adapter,
        };
        Ok(Field {
            surface,
            device,
            queue,
            config,
            post,
            bc,
            texture_tiers: max_texture_tiers(bc),
            scene_sources,
            scene,
            material_maps: false,
            triplanar: true,
            wind: true,
            postproc: false,
            bloom: false,
            dof: false,
            antialiasing: false,
            sharpening: false,
            sharpness: postproc::DEFAULT_SHARPNESS,
            render_scale: 1.0,
            upscale: Upscale::Fsr,
            fog: false,
            water_effects: false,
            water_pipeline,
            water,
            water_normals: None,
            water_group: None,
            sky_visible: false,
            sky,
            particles_visible: false,
            particles: Particles::new(),
            particle_pipelines,
            reflection_particles,
            particle_instances,
            emitting: None,
            refraction: false,
            real_depth: false,
            reflection: false,
            refraction_scene: None,
            reflection_scene: None,
            refraction_view: (refraction_buffer, refraction_group),
            reflection_view: (reflection_buffer, reflection_group),
            water_targets: None,
            water_targets_generation: 0,
            fancy_water: false,
            waves,
            water_effects_targets: None,
            waves_group: None,
            water_environment: None,
            decal_pipeline,
            shadow,
            borders,
            overlays,
            ground_layout,
            terrain_sampler,
            view_buffer,
            view_bind_group,
            poses,
            pose_layout,
            pose_bind_group,
            pose_generation,
            texture_layout,
            material_layout,
            sampler,
            default_maps,
            default_material,
            ground: None,
            store: {
                let store = assets::new();
                store.borrow_mut().set_texture_tiers(max_texture_tiers(bc));
                store
            },
            meshes: HashMap::new(),
            textures: HashMap::new(),
            uploads: 0,
            materials: HashMap::new(),
            entities: Entities::default(),
            palette: Vec::new(),
            models: HashMap::new(),
            bounds: HashMap::new(),
            camera,
            instances,
            groups: HashMap::new(),
            shadow_instances,
            shadow_groups: Default::default(),
            shadow_planes: Vec::new(),
            casting: 0,
            placed: 0,
            far,
            far_instances,
            far_groups: HashMap::new(),
            decals: Vec::new(),
            decal_instances,
            decal_indices,
            eye: [0.0; 3],
            pickables: Vec::new(),
            selection: Selection::default(),
            renames: Vec::new(),
            bandbox: document
                .get_element_by_id("bandbox")
                .and_then(|element| element.dyn_into::<HtmlElement>().ok()),
            last_fade: now(),
            sound_clocks: HashMap::new(),
            shots: HashMap::new(),
            shot_models: HashMap::new(),
            impact_models: HashMap::new(),
            projectiles: true,
            device_pixel_ratio: 1.0,
            stats,
            timer,
            draw_calls: Cell::new(0),
        })
    }

    /// The view-projection matrix, row-major, left-handed with depth 0..1
    /// like the engine's camera (as `unit.rs`), and the eye, right and up.
    fn view_projection(&self) -> ([[f32; 4]; 4], [f32; 3], [f32; 3], [f32; 3]) {
        let (view, eye, right, up) = self.camera_view();
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

    /// The view matrix, where the camera is, and its right and up.
    fn camera_view(&self) -> ([[f32; 4]; 4], [f32; 3], [f32; 3], [f32; 3]) {
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
        (view, eye, right, up)
    }

    /// `ComputeRefractionCamera`'s camera: the view's, its field of view
    /// 5 % wider so ripples can show what is just off the screen, drawn
    /// square and stretched back to the screen's aspect.
    fn refraction_camera(&self) -> water::WaterCamera {
        let (view, _, right, up) = self.camera_view();
        let f = 1.0 / (FIELD_OF_VIEW * 1.05 / 2.0).tan();
        let stretch = self.config.height as f32 / self.config.width.max(1) as f32;
        let projection = [
            [f * stretch, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, FAR / (FAR - NEAR), -NEAR * FAR / (FAR - NEAR)],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let level = self
            .ground
            .as_ref()
            .map_or(0.0, |ground| ground.image.water_level);
        // The mirror in the water, y to 2 level - y, which the reflection
        // camera's orientation takes on.
        let mirror = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, -1.0, 0.0, 2.0 * level],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let view_proj = multiply(&projection, &view);
        water::WaterCamera {
            view_proj,
            right,
            up,
            forward: cross(right, up),
            scale: [f * stretch, f],
            near: NEAR,
            far: FAR,
            reflection: multiply(&view_proj, &mirror),
        }
    }

    /// The map's light, or a plain one until it arrives.
    fn light(&self) -> Light {
        match &self.ground {
            Some(ground) => ground.image.light,
            None => {
                let sun = Vec3::new(0.4, 0.8, -0.45).normalized();
                Light::plain([sun.x, sun.y, sun.z], [SUN_STRENGTH; 3], [AMBIENT; 3])
            }
        }
    }

    /// The sim time since the match began, in seconds, which the engine's
    /// `CTimeManager` and the timers that follow the sim keep.
    fn sim_time(&self, progress: f32) -> f64 {
        (f64::from(self.entities.turn) + f64::from(progress)) * TURN_MS / 1000.0
    }

    /// The view's wind: model_common.vs' `sim_time`, then 1 when the
    /// viewer's wind setting sways the models.
    fn wind(&self, progress: f32) -> [f32; 2] {
        [
            self.sim_time(progress) as f32,
            if self.wind { 1.0 } else { 0.0 },
        ]
    }

    fn write_view(
        &self,
        buffer: &wgpu::Buffer,
        view_proj: &[[f32; 4]; 4],
        camera: [[f32; 3]; 3],
        [time, wind]: [f32; 2],
    ) {
        let [x, y, z] = self.light().sun;
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
        uniform.extend_from_slice(&[x, y, z, 0.0]);
        uniform.extend_from_slice(&[time, wind, size, circular]);
        uniform.extend_from_slice(&[water, 0.0, 0.0, 0.0]);
        for [x, y, z] in camera {
            uniform.extend_from_slice(&[x, y, z, 0.0]);
        }
        let bytes: Vec<u8> = uniform
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        self.queue.write_buffer(buffer, 0, &bytes);
    }

    /// Fills `groups` and `far_groups` with this frame's instances, and
    /// `shadow_groups` with each cascade's casters, inside `casters`, the
    /// planes of each cascade's box.
    fn place_entities(
        &mut self,
        progress: f32,
        view_proj: &[[f32; 4]; 4],
        casters: Option<Vec<[[f32; 4]; 6]>>,
    ) {
        let mut groups = std::mem::take(&mut self.groups);
        for group in groups
            .values_mut()
            .chain(self.far_groups.values_mut())
            .chain(self.shadow_groups.iter_mut().flat_map(HashMap::values_mut))
        {
            group.clear();
        }
        self.shadow_planes = casters.unwrap_or_default();
        self.placed = 0;
        self.decals.clear();
        self.pickables.clear();
        self.stats.levels = [0; LEVELS];
        let planes = frustum_planes(view_proj);
        let pixel_scale = self.config.height as f32 / (FIELD_OF_VIEW / 2.0).tan();
        if self.particles_visible {
            // CParticleManager's time, which CCmpParticleManager advances
            // by the sim time.
            self.particles.begin(self.sim_time(progress));
        }
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
        if self.projectiles {
            self.place_shots(progress, view_proj, &planes, pixel_scale, &mut groups);
        }
        if self.particles_visible {
            let dots = &self.entities.dots;
            let shots = &self.shots;
            self.particles.finish(
                |position, reach| !outside(&planes, [position.x, position.y, position.z], reach),
                |owner| match owner {
                    Owner::Entity(id) => dots.binary_search_by_key(&id, |(id, _)| *id).is_ok(),
                    Owner::Shot(id) | Owner::Impact(id) => shots.contains_key(&id),
                },
            );
        }
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
        let key = (dot.actor, dot.variants, dot.animation, dot.seed);
        let Some(model) = self.entity_model(dot) else {
            return;
        };

        let [x, y, z] = self.entity_position(dot, progress);
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

        let root_file = match &model.shape {
            Shape::Mesh(file) => *file,
            Shape::Decal(decal) => {
                let radius =
                    decal.size_x.hypot(decal.size_z) / 2.0 + decal.offset_x.hypot(decal.offset_z);
                let Some(level) = screen_level([x, y, z], radius, view_proj, planes, pixel_scale)
                else {
                    return;
                };
                transform.translate(Vec3::new(x, y, z));
                self.add_pickable(id, dot, None, &transform);
                self.place_decal(&model, decal, &transform, level < COARSE_LEVEL);
                return;
            }
            // Culled with the emitters, by their own reach.
            Shape::Particles(emitter) => {
                if self.particles_visible {
                    transform.translate(Vec3::new(x, y, z));
                    self.place_emitter(Owner::Entity(id), &model, emitter, &transform, dot);
                }
                return;
            }
        };
        let root_mesh = match self.store.borrow_mut().mesh(root_file, false) {
            Get::Ready(mesh) => mesh,
            Get::Pending | Get::Missing => return,
        };
        let height = root_mesh.max[1] - root_mesh.min[1];
        // CCmpPosition::GetConstructionProgressOffset, with the root
        // mesh's height for the selection box's.
        let mut sink = if dot.construction == 0 {
            0.0
        } else {
            (f32::from(dot.construction) / 255.0 - 1.0) * height
        };

        // The ground under the entity, in the root's space.
        let ground = self.ground.as_ref().map(|ground| ground.image.ground(x, z));
        let floor = ground.map_or(f32::MIN, |ground| ground - (y + sink));
        let (low, high) = self.tree_bounds(key, &model, floor).unwrap_or((
            Vec3::new(root_mesh.min[0], root_mesh.min[1], root_mesh.min[2]),
            Vec3::new(root_mesh.max[0], root_mesh.max[1], root_mesh.max[2]),
        ));
        if let Some(sinking) = self.sinking(dot) {
            let since = self.entities.turn.saturating_sub(dot.animation_turn) as f32 + progress;
            let Some((depth, tilt)) = decay(
                &sinking,
                since * TURN_MS as f32 / 1000.0,
                high.y - low.y,
                ground.map_or(0.0, |ground| y - ground),
                [x, z],
            ) else {
                return;
            };
            sink -= depth;
            // CCmpPosition::GetInterpolatedTransform: the x and z
            // rotations, then the turn.
            if let Some([rot_x, rot_z]) = tilt {
                transform = Mat4::from_quat(Quat {
                    x: (rot_x / 2.0).sin(),
                    y: 0.0,
                    z: 0.0,
                    w: (rot_x / 2.0).cos(),
                });
                transform.rotate_by(Quat {
                    x: 0.0,
                    y: 0.0,
                    z: (rot_z / 2.0).sin(),
                    w: (rot_z / 2.0).cos(),
                });
                transform.rotate_by(Quat {
                    x: 0.0,
                    y: half.sin(),
                    z: 0.0,
                    w: half.cos(),
                });
            }
        }
        transform.translate(Vec3::new(x, y + sink, z));
        // Around the entity's position, holding the bounds at any turn.
        let reach = [low.x, high.x]
            .into_iter()
            .flat_map(|x| [low.z, high.z].map(|z| x.hypot(z)))
            .fold(0.0, f32::max);
        let radius = reach.hypot((high.y - low.y) / 2.0) * REACH_MARGIN + 1.0;
        let centre_y = (low.y + high.y) / 2.0;
        let centre = [x, y + sink + centre_y, z];
        let casting = self.casting_into(centre, radius);
        let Some(level) = screen_level(centre, radius, view_proj, planes, pixel_scale) else {
            if casting != 0 {
                self.place_caster(id, dot, progress, &model, &transform, casting);
            }
            return;
        };
        self.add_pickable(id, dot, Some((low, high)), &transform);
        if has_decal(&model) {
            self.place_decals(&model, &transform, level < COARSE_LEVEL, 0);
        }

        let entity = self.entity_state(id, dot, progress, level);
        if level == FAR_LEVEL
            && self.place_far(
                &model,
                &root_mesh,
                &entity,
                [x, centre[1], z, radius],
                centre_y,
                rot_y,
            )
        {
            self.stats.levels[FAR_LEVEL] += 1;
            if casting != 0 {
                self.place_caster(id, dot, progress, &model, &transform, casting);
            }
            return;
        }
        let placed = self.placed;
        self.emitting = self.particles_visible.then_some(Owner::Entity(id));
        self.casting = casting;
        self.place_model(&model, &transform, &entity, None, 0, Some(groups));
        self.casting = 0;
        self.emitting = None;
        // Counted once something of it is drawn, not while it loads.
        if self.placed > placed {
            self.stats.levels[level.min(COARSE_LEVEL)] += 1;
        }
    }

    /// Records a drawn entity for picking, by its template's box under
    /// `transform`, the one it is drawn with; `bounds` as `pick::local_box`.
    fn add_pickable(&mut self, id: u32, dot: &Dot, bounds: Option<(Vec3, Vec3)>, transform: &Mat4) {
        let details = self
            .entities
            .template_details
            .get(dot.template as usize)
            .and_then(Option::as_ref);
        let Some((low, high)) = pick::local_box(details, bounds) else {
            return;
        };
        let (x, z) = (transform.m[0][0], transform.m[2][0]);
        let length = x.hypot(z);
        // The transform turns the model by rotY + PI, but the overlay's
        // unitX comes from rotY alone, so it is the model's -x.
        self.pickables.push(Pickable {
            id,
            position: [transform.m[0][3], transform.m[1][3], transform.m[2][3]],
            facing: if length > 0.0 {
                [-x / length, -z / length]
            } else {
                [1.0, 0.0]
            },
            bounds: OrientedBox::new(low, high, transform),
        });
    }

    /// Makes a selection of the clicks and drags since the last frame, as
    /// `handleInputBeforeGui` does, then highlights what the bandbox would
    /// select or else what the mouse is over.
    fn apply_picks(
        &mut self,
        view_proj: &[[f32; 4]; 4],
        eye: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
    ) {
        let (picks, hover, band) = {
            let mut camera = self.camera.borrow_mut();
            (std::mem::take(&mut camera.picks), camera.hover, camera.band)
        };
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let tan_half = (FIELD_OF_VIEW / 2.0).tan();
        let ray = |at| pick::camera_ray(eye, right, up, cross(right, up), tan_half, aspect, at);
        let entities = &self.entities;
        let look: Lookup = &|id| selectee(entities, id);
        for input in picks {
            let (ids, add, remove) = match input {
                PickInput::Click {
                    at,
                    count,
                    add,
                    remove,
                } => {
                    let ids = match pick::pick_at_point(&self.pickables, &ray(at)) {
                        Some(id) if count >= 2 => self.similar(id, count >= 3, view_proj),
                        Some(id) => vec![id],
                        // Ground clicked: the selection stays while adding
                        // or removing, else it clears.
                        None if add || remove => continue,
                        None => Vec::new(),
                    };
                    (ids, add, remove)
                }
                PickInput::Rect {
                    from,
                    to,
                    add,
                    remove,
                } => (self.in_band(view_proj, from, to), add, remove),
                PickInput::Clear => (Vec::new(), false, false),
            };
            if add {
                self.selection.add_list(&ids, look);
            } else if remove {
                self.selection.remove_list(&ids, look);
            } else {
                self.selection.reset(look);
                self.selection.add_list(&ids, look);
            }
        }
        let highlighted = match (band, hover) {
            (Some(band), _) => self.in_band(view_proj, band.from, band.to),
            (None, Some(at)) => pick::pick_at_point(&self.pickables, &ray(at))
                .into_iter()
                .collect(),
            (None, None) => Vec::new(),
        };
        self.selection.set_highlight_list(&highlighted, look);
        if let Some(element) = &self.bandbox {
            match band {
                Some(band) => {
                    let [x0, y0] = [0, 1].map(|i| band.client_from[i].min(band.client_to[i]));
                    let [x1, y1] = [0, 1].map(|i| band.client_from[i].max(band.client_to[i]));
                    let _ = element.set_attribute(
                        "style",
                        &format!(
                            "left: {x0}px; top: {y0}px; width: {}px; height: {}px",
                            x1 - x0,
                            y1 - y0
                        ),
                    );
                    element.set_hidden(false);
                }
                None => element.set_hidden(true),
            }
        }
    }

    /// `PickEntitiesInRect` then `getPreferredEntities`, corners 0..1
    /// across the canvas.
    fn in_band(&self, view_proj: &[[f32; 4]; 4], from: [f32; 2], to: [f32; 2]) -> Vec<u32> {
        let ids = pick::pick_in_rect(&self.pickables, view_proj, from, to);
        pick::preferred(ids, |id, class| {
            self.dot_details(id)
                .is_some_and(|(_, details)| pick::has_class(details, &self.entities.strings, class))
        })
    }

    /// `PickSimilarEntities` for a double click, or with `exact` a triple
    /// one: on screen, of the clicked entity's owner only (CONTROL.md 5),
    /// of its selection group, or of its template when it has none or
    /// `exact`.
    fn similar(&self, clicked: u32, exact: bool, view_proj: &[[f32; 4]; 4]) -> Vec<u32> {
        let Some((dot, details)) = self.dot_details(clicked) else {
            return vec![clicked];
        };
        let (owner, template) = (dot.owner, dot.template);
        let group = if exact { 0 } else { details.group_name };
        pick::pick_similar(&self.pickables, view_proj, |id| {
            self.dot_details(id).is_some_and(|(other, other_details)| {
                other.owner == owner
                    && if group != 0 {
                        other_details.group_name == group
                    } else {
                        other.template == template
                    }
            })
        })
    }

    fn entity_dot(&self, id: u32) -> Option<&Dot> {
        entity_dot(&self.entities, id)
    }

    fn dot_details(&self, id: u32) -> Option<(&Dot, &TemplateDetails)> {
        let dot = self.entity_dot(id)?;
        let details = self
            .entities
            .template_details
            .get(dot.template as usize)?
            .as_ref()?;
        Some((dot, details))
    }

    /// How a corpse or rubble sinks: the `Decay` of the template of a
    /// local entity `Health.js` leaves, whose filters make it active.
    fn sinking(&self, dot: &Dot) -> Option<Sinking> {
        let template = self.entities.templates.get(dot.template as usize)?;
        if !(template.starts_with("corpse|") || template.starts_with("decay|")) {
            return None;
        }
        self.store.borrow().sinking(template)
    }

    /// How `place_model` draws the entity at `level`.
    fn entity_state(&self, id: u32, dot: &Dot, progress: f32, level: usize) -> EntityState {
        let state = string(&self.entities.strings, dot.animation)
            .map_or_else(|| "idle".to_string(), str::to_lowercase);
        let mut pick = DefaultHasher::new();
        (id, dot.animation_turn).hash(&mut pick);
        EntityState {
            dot: *dot,
            turn: self.entities.turn,
            progress,
            state,
            pick: pick.finish(),
            color: owner_color(dot, &self.palette),
            level,
            time: None,
        }
    }

    /// Places an entity that is not drawn as a mesh into the
    /// `shadow_groups` of the cascades in `casting`, at the coarse level:
    /// its shadow can fall in view all the same.
    #[allow(clippy::too_many_arguments)]
    fn place_caster(
        &mut self,
        id: u32,
        dot: &Dot,
        progress: f32,
        model: &Model,
        transform: &Mat4,
        casting: u8,
    ) {
        let entity = self.entity_state(id, dot, progress, COARSE_LEVEL);
        // Not counted among the models drawn.
        let placed = self.placed;
        self.casting = casting;
        self.place_model(model, transform, &entity, None, 0, None);
        self.casting = 0;
        self.placed = placed;
    }

    /// A bit per cascade whose box a sphere touches.
    fn casting_into(&self, centre: [f32; 3], radius: f32) -> u8 {
        self.shadow_planes
            .iter()
            .enumerate()
            .filter(|(_, planes)| !outside(planes, centre, radius))
            .fold(0, |casting, (cascade, _)| casting | 1 << cascade)
    }

    /// Where the entity is drawn this frame, in metres.
    fn entity_position(&self, dot: &Dot, progress: f32) -> [f32; 3] {
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
        [x, y, z]
    }

    /// The model the entity's actor resolves to, through the cache;
    /// `None` while its manifest loads or when it resolves to nothing.
    fn entity_model(&mut self, dot: &Dot) -> Option<Rc<Model>> {
        let actor = string(&self.entities.strings, dot.actor)?;
        let manifest = match self.store.borrow_mut().actor(actor) {
            Get::Ready(manifest) => manifest,
            Get::Pending | Get::Missing => return None,
        };
        let key = (dot.actor, dot.variants, dot.animation, dot.seed);
        if let Some(model) = self.models.get(&key) {
            return model.clone();
        }
        let selections = self.selections(dot);
        let model = variation::resolve(&manifest, &selections, dot.seed).map(Rc::new);
        self.models.insert(key, model.clone());
        model
    }

    /// The sound groups whose point the animations passed since the last
    /// frame, as `CUnitAnimation::Update` plays them: of every entity, on
    /// screen or not, at most once per frame, at the `sound` point or else
    /// the `event` point of the root model's animation.
    fn animation_sounds(&mut self, progress: f32) -> Vec<AnimationSound> {
        let mut sounds = Vec::new();
        let dots = std::mem::take(&mut self.entities.dots);
        let mut clocks = std::mem::take(&mut self.sound_clocks);
        let mut seen = HashMap::with_capacity(clocks.len());
        for (id, dot) in &dots {
            if dot.sound_group == 0 {
                continue;
            }
            let Some((key, time, start, point, duration, once)) =
                self.sound_clock(*id, dot, progress)
            else {
                continue;
            };
            // A clock first seen now (a join, a seek, a model just loaded)
            // starts here rather than replaying what went before.
            let last = match clocks.remove(id) {
                Some(clock) if clock.key == key => clock.time,
                Some(_) => start - f32::EPSILON.max(start.abs() * f32::EPSILON),
                None => time,
            };
            let passed = if once {
                last < point && point <= time
            } else {
                time > last
                    && ((time - point) / duration).floor() > ((last - point) / duration).floor()
            };
            if passed {
                sounds.push(AnimationSound {
                    group: dot.sound_group - 1,
                    owner: dot.owner,
                    visibility: dot.visibility,
                    position: self.entity_position(dot, progress),
                });
            }
            seen.insert(*id, SoundClock { key, time });
        }
        self.entities.dots = dots;
        self.sound_clocks = seen;
        sounds
    }

    /// The entity's animation clock: its key, the time now and at the
    /// animation's start (unwrapped, ms), the sound point and duration in
    /// ms, and whether it plays once. `None` while pieces load or when the
    /// animation has no point.
    #[allow(clippy::type_complexity)]
    fn sound_clock(
        &mut self,
        id: u32,
        dot: &Dot,
        progress: f32,
    ) -> Option<((u32, u32, u32, u64), f32, f32, f32, f32, bool)> {
        let model = self.entity_model(dot)?;
        let Shape::Mesh(root_file) = model.shape else {
            return None;
        };
        let root_mesh = match self.store.borrow_mut().mesh(root_file, false) {
            Get::Ready(mesh) => mesh,
            Get::Pending | Get::Missing => return None,
        };
        let state = string(&self.entities.strings, dot.animation)
            .map_or_else(|| "idle".to_string(), str::to_lowercase);
        let mut pick = DefaultHasher::new();
        (id, dot.animation_turn).hash(&mut pick);
        let (chosen, data) = self.root_animation(&model, &root_mesh, &state, pick.finish())??;
        let fraction = if chosen.sound >= 0.0 {
            chosen.sound
        } else {
            chosen.event
        };
        if fraction < 0.0 {
            return None;
        }
        let duration = data.frames as f32 * data.frame_ms;
        if duration <= 0.0 {
            return None;
        }
        let mut entity = EntityState {
            dot: *dot,
            turn: self.entities.turn,
            progress,
            state,
            pick: 0,
            color: 0,
            level: 0,
            time: None,
        };
        let (time, _) = animation_clock(&entity, chosen, &data);
        let start_turn = if dot.sync_repeat > 0 && chosen.event >= 0.0 {
            dot.sync_turn
        } else {
            dot.animation_turn
        };
        entity.turn = start_turn;
        entity.progress = 0.0;
        entity.dot.speed_turn = dot.animation_turn;
        entity.dot.phase = 0.0;
        let (start, _) = animation_clock(&entity, chosen, &data);
        let key = (
            dot.animation,
            dot.animation_turn,
            dot.sync_turn,
            chosen.file.hash,
        );
        let time = if dot.once {
            time.min(duration - 1.0)
        } else {
            time
        };
        Some((key, time, start, fraction * duration, duration, dot.once))
    }

    /// Draws the entity as its look's impostor; false while the atlas is
    /// not ready, which asks for it.
    fn place_far(
        &mut self,
        model: &Rc<Model>,
        root_mesh: &Mesh,
        entity: &EntityState,
        // The bounding sphere's centre and radius.
        sphere: [f32; 4],
        // Its centre's height in the model's own space.
        centre_y: f32,
        rot_y: f32,
    ) -> bool {
        let [x, y, z, radius] = sphere;
        let centre = [x, y, z];
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
            let request = Request {
                model: Rc::clone(model),
                dot: *dot,
                state: entity.state.clone(),
                pick: FAR_PICK,
                radius,
                centre_y,
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
        // A decal at the root is laid on the ground and never asked for.
        let Shape::Mesh(root_file) = model.shape else {
            return None;
        };
        let root_mesh = match self.store.borrow_mut().mesh(root_file, false) {
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
            if !self.place_model(
                &model,
                &Mat4::identity(),
                &entity,
                None,
                0,
                Some(&mut groups),
            ) {
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
                .filter_map(|(slot, (mesh, level, material), start, end, count)| {
                    let gpu = self.meshes.get(mesh)?;
                    Some(CaptureDraw {
                        slot: *slot,
                        vertices: &gpu.vertices,
                        indices: &gpu.indices,
                        index_range: gpu.levels[*level],
                        material: self
                            .materials
                            .get(material)
                            .map_or(&self.default_material, |(group, _)| group),
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

    /// The bounds of the meshes in the model tree, props in their bind
    /// pose, in the root's space, leaving out those wholly under `floor`,
    /// the ground there, which cannot be seen and would drag the sphere
    /// down to them: a field's root mesh is a small plane buried 27 m down
    /// that only carries the prop points of its crop. Measured from the
    /// ground rather than the root, as a floating root's meshes may hang
    /// under it in the water, a fish's all of them. `None` while a piece
    /// is on its way.
    fn tree_bounds(&mut self, key: ModelKey, model: &Model, floor: f32) -> Option<(Vec3, Vec3)> {
        if !self.bounds.contains_key(&key) {
            let mut pieces = Vec::new();
            if !self.add_bounds(model, &Mat4::identity(), 0, &mut pieces) {
                return None;
            }
            self.bounds.insert(key, pieces);
        }
        let pieces = &self.bounds[&key];
        let seen = || pieces.iter().filter(|(_, high)| high.y >= floor);
        let all = || pieces.iter();
        let join =
            |(min, max): (Vec3, Vec3), (low, high): &(Vec3, Vec3)| (min.min(*low), max.max(*high));
        let empty = (
            Vec3::new(f32::MAX, f32::MAX, f32::MAX),
            Vec3::new(f32::MIN, f32::MIN, f32::MIN),
        );
        // Wholly underground, it is still somewhere.
        let bounds = if seen().next().is_some() {
            seen().fold(empty, join)
        } else {
            all().next()?;
            all().fold(empty, join)
        };
        Some(bounds)
    }

    /// Adds the bounds of `model`'s mesh, moved by `transform`, and its
    /// props' to `pieces`, as `place_model` places them; false while a
    /// piece is on its way.
    fn add_bounds(
        &mut self,
        model: &Model,
        transform: &Mat4,
        depth: usize,
        pieces: &mut Vec<(Vec3, Vec3)>,
    ) -> bool {
        // Decals lie on the ground and are culled on their own.
        let Shape::Mesh(file) = model.shape else {
            return true;
        };
        let mesh = match self.store.borrow_mut().mesh(file, false) {
            Get::Ready(mesh) => mesh,
            Get::Pending => return false,
            Get::Missing => return true,
        };
        let skeleton = match mesh.skeleton {
            Some(file) => match self.store.borrow_mut().skeleton(file) {
                Get::Ready(skeleton) => Some(skeleton),
                Get::Pending => return false,
                Get::Missing => None,
            },
            None => None,
        };
        let mut low = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut high = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
        for corner in 0..8 {
            let pick = |axis: usize| {
                if corner >> axis & 1 == 0 {
                    mesh.min[axis]
                } else {
                    mesh.max[axis]
                }
            };
            let moved = transform.transform(Vec3::new(pick(0), pick(1), pick(2)));
            low = low.min(moved);
            high = high.max(moved);
        }
        pieces.push((low, high));
        if depth >= 8 {
            return true;
        }
        let mut complete = true;
        for (point_name, prop) in &model.props {
            let Some(point) = mesh
                .prop_points
                .iter()
                .find(|point| point.name == *point_name)
            else {
                continue;
            };
            let mut placed = *transform;
            if point.bone != NO_BONE
                && let Some(skeleton) = &skeleton
            {
                placed = placed.mul(&pose::bone_matrix(None, skeleton, usize::from(point.bone)));
            }
            placed = placed.mul(&point_matrix(point));
            complete &= self.add_bounds(prop, &placed, depth + 1, pieces);
        }
        complete
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
        // `None` for a shadow caster only, which goes to the cascades.
        mut groups: Option<&mut HashMap<GroupKey, Vec<u8>>>,
    ) -> bool {
        let file = match &model.shape {
            Shape::Mesh(file) => *file,
            // place_decals lays decals, whatever the level of detail.
            Shape::Decal(_) => return true,
            Shape::Particles(emitter) => {
                if let Some(owner) = self.emitting {
                    self.place_emitter(owner, model, emitter, transform, &entity.dot);
                }
                return true;
            }
        };
        let detail = entity.level < COARSE_LEVEL;
        let mesh = match self.store.borrow_mut().mesh(file, detail) {
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
        let Some(material) = self.material(model, detail) else {
            return false;
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
            file.hash,
            &mesh,
            model,
            transform,
            entity,
            pose,
            blend,
            material,
            groups.as_deref_mut(),
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
            placed = placed.mul(&point_matrix(point));
            complete &= self.place_model(
                prop,
                &placed,
                entity,
                root_id,
                depth + 1,
                groups.as_deref_mut(),
            );
        }
        complete
    }

    #[allow(clippy::too_many_arguments)]
    fn add_instance(
        &mut self,
        mesh_hash: u64,
        mesh: &Mesh,
        model: &Model,
        transform: &Mat4,
        entity: &EntityState,
        pose: [u32; 3],
        blend: f32,
        material: u64,
        groups: Option<&mut HashMap<GroupKey, Vec<u8>>>,
    ) {
        let complete = mesh.detail.is_none();
        let current = self
            .meshes
            .get(&mesh_hash)
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
            self.meshes.insert(mesh_hash, gpu);
        }
        self.placed += 1;
        let level = entity.level.min(mesh.levels.len() - 1);
        let key = (mesh_hash, level, material);
        let mut instance = [0u8; INSTANCE_SIZE];
        let tinted = if model.player_color { 1.0 } else { 0.0 };
        let values = transform.m[..3]
            .iter()
            .flatten()
            .map(|value| value.to_le_bytes())
            .chain([pose[0], pose[1], pose[2], entity.color].map(|value| value.to_le_bytes()))
            .chain([blend, tinted, model.alpha_ref, 0.0f32].map(|value| value.to_le_bytes()));
        for (slot, bytes) in instance.chunks_exact_mut(4).zip(values) {
            slot.copy_from_slice(&bytes);
        }
        if let Some(groups) = groups {
            groups.entry(key).or_default().extend_from_slice(&instance);
        }
        for (cascade, cascade_groups) in self.shadow_groups.iter_mut().enumerate() {
            if self.casting & 1 << cascade != 0 {
                cascade_groups
                    .entry(key)
                    .or_default()
                    .extend_from_slice(&instance);
            }
        }
    }

    /// Lays the decals of `model`'s tree on the ground. Apart from
    /// `place_model`, as an impostor leaves out the ground under it.
    fn place_decals(&mut self, model: &Model, transform: &Mat4, detail: bool, depth: usize) {
        let file = match &model.shape {
            Shape::Mesh(file) => *file,
            Shape::Decal(decal) => {
                self.place_decal(model, decal, transform, detail);
                return;
            }
            Shape::Particles(_) => return,
        };
        if depth >= 8 {
            return;
        }
        let mesh = match self.store.borrow_mut().mesh(file, false) {
            Get::Ready(mesh) => mesh,
            Get::Pending | Get::Missing => return,
        };
        let skeleton = match mesh
            .skeleton
            .map(|file| self.store.borrow_mut().skeleton(file))
        {
            Some(Get::Ready(skeleton)) => Some(skeleton),
            _ => None,
        };
        for (point_name, prop) in &model.props {
            if !has_decal(prop) {
                continue;
            }
            let Some(point) = mesh
                .prop_points
                .iter()
                .find(|point| point.name == *point_name)
            else {
                continue;
            };
            let mut placed = *transform;
            // The bind pose: decals hang off "root" or still points, and
            // keep only their parent's place and heading anyway.
            if point.bone != NO_BONE
                && let Some(skeleton) = &skeleton
            {
                placed = placed.mul(&pose::bone_matrix(None, skeleton, usize::from(point.bone)));
            }
            placed = placed.mul(&point_matrix(point));
            self.place_decals(prop, &placed, detail, depth + 1);
        }
    }

    /// `CModelDecal::SetTransform`: the parent's heading and place only,
    /// as decals lie flat on the ground. Left out until its texture is in;
    /// the engine draws none without one either.
    fn place_decal(&mut self, model: &Model, decal: &Decal, transform: &Mat4, detail: bool) {
        let Some(file) = model.texture else {
            return;
        };
        let Some(Some(texture)) = self.texture(file, detail) else {
            return;
        };
        let m = &transform.m;
        // CMatrix3D::GetYRotation.
        let yaw = -m[2][0].atan2(m[0][0]) + decal.angle;
        let (sin, cos) = yaw.sin_cos();
        self.decals.push(PlacedDecal {
            texture,
            place: [
                cos,
                sin,
                m[0][3],
                m[2][3],
                decal.offset_x,
                decal.offset_z,
                decal.size_x,
                decal.size_z,
            ],
        });
    }

    /// The bind group of a texture: `Some(Some(hash))` when ready,
    /// `Some(None)` to draw it white (it will not come), `None` while it
    /// is on its way. Uploaded again once a tier of its detail has
    /// arrived or the texture quality has changed.
    fn texture(&mut self, file: FileRef, detail: bool) -> Option<Option<u64>> {
        let texture = match self.store.borrow_mut().texture(file, detail) {
            Get::Ready(texture) => texture,
            Get::Pending => return None,
            Get::Missing => return Some(None),
        };
        let uploaded = self.uploaded(&texture);
        let current = self
            .textures
            .get(&file.hash)
            .is_some_and(|held| held.uploaded == uploaded);
        if !current {
            let view = upload_texture(&self.device, &self.queue, &texture, uploaded.1, self.bc);
            let group =
                texture_bind_group(&self.device, &self.texture_layout, &view, &self.sampler);
            self.uploads += 1;
            self.textures.insert(
                file.hash,
                GpuTexture {
                    view,
                    group,
                    uploaded,
                    upload: self.uploads,
                },
            );
        }
        Some(Some(file.hash))
    }

    /// The key of `model`'s material bind group: its colour, normal,
    /// specular and ambient occlusion maps and `effectSettings`, made again
    /// once one of them has been uploaded again. `None` while the colour
    /// map is on its way; a map still on its way is left out until it
    /// arrives, so the model shows meanwhile. With the material maps off
    /// the maps are not even fetched.
    fn material(&mut self, model: &Model, detail: bool) -> Option<u64> {
        let base = match model.texture {
            Some(file) => self.texture(file, detail)?,
            None => None,
        };
        let wanted = if self.material_maps {
            model.maps
        } else {
            [None; 3]
        };
        let maps = wanted.map(|file| file.and_then(|file| self.texture(file, detail).flatten()));
        let hashes = [base, maps[0], maps[1], maps[2]];
        let mut hasher = DefaultHasher::new();
        hashes.hash(&mut hasher);
        model.effect.map(f32::to_bits).hash(&mut hasher);
        model
            .wind
            .map(|wind| wind.map(f32::to_bits))
            .hash(&mut hasher);
        let key = hasher.finish();
        let textures = hashes.map(|hash| hash.and_then(|hash| self.textures.get(&hash)));
        let uploads = textures.map(|texture| texture.map_or(0, |texture| texture.upload));
        if self
            .materials
            .get(&key)
            .is_none_or(|(_, held)| *held != uploads)
        {
            let views = [0, 1, 2, 3].map(|slot| {
                textures[slot].map_or(&self.default_maps[slot], |texture| &texture.view)
            });
            let group = material_bind_group(
                &self.device,
                &self.queue,
                &self.material_layout,
                views,
                &self.sampler,
                model.effect,
                material_flags(maps[0].is_some(), model.wind),
            );
            self.materials.insert(key, (group, uploads));
        }
        Some(key)
    }

    /// What `texture` is uploaded from at the current texture quality.
    fn uploaded(&self, texture: &Texture) -> Uploaded {
        let first = texture.first_level(TEXTURE_TIER_SIDES[self.texture_tiers - 1]);
        (texture.levels.len(), first)
    }

    /// `CUnit::ReloadObject`'s entity selections: the variant selections
    /// and the animation name, lower case as `SetEntitySelection` keeps
    /// them.
    fn selections(&self, dot: &Dot) -> BTreeSet<String> {
        let mut selections = self.variant_selections(dot.variants);
        if let Some(animation) = string(&self.entities.strings, dot.animation) {
            selections.insert(animation.to_lowercase());
        }
        selections
    }

    /// The selections of a 1-based variant set index, lower case.
    fn variant_selections(&self, variants: u32) -> BTreeSet<String> {
        let strings = &self.entities.strings;
        (variants as usize)
            .checked_sub(1)
            .and_then(|index| self.entities.variant_sets.get(index))
            .into_iter()
            .flatten()
            .filter_map(|(_, selection)| strings.get(*selection as usize))
            .map(|selection| selection.to_lowercase())
            .collect()
    }

    /// Where `CModel::FindFirstAmmoProp` has the shooter's ammo as the turn
    /// starts, which is where the engine launches from; `None` when it has
    /// no ammo prop or a piece of it has not arrived.
    fn ammo_point(&mut self, id: u32) -> Option<Vec3> {
        let dots = &self.entities.dots;
        let index = dots.binary_search_by_key(&id, |(id, _)| *id).ok()?;
        let dot = dots[index].1;
        let model = self.entity_model(&dot)?;
        let Shape::Mesh(root_file) = model.shape else {
            return None;
        };
        let root_mesh = match self.store.borrow_mut().mesh(root_file, false) {
            Get::Ready(mesh) => mesh,
            Get::Pending | Get::Missing => return None,
        };
        // As place_entity places it.
        let [x, y, z] = self.entity_position(&dot, 0.0);
        let rot_y = dot.from_angle / 256.0 * std::f32::consts::TAU;
        let half = (rot_y + std::f32::consts::PI) / 2.0;
        let mut transform = Mat4::from_quat(Quat {
            x: 0.0,
            y: half.sin(),
            z: 0.0,
            w: half.cos(),
        });
        let height = root_mesh.max[1] - root_mesh.min[1];
        let sink = if dot.construction == 0 {
            0.0
        } else {
            (f32::from(dot.construction) / 255.0 - 1.0) * height
        };
        transform.translate(Vec3::new(x, y + sink, z));
        let state = string(&self.entities.strings, dot.animation)
            .map_or_else(|| "idle".to_string(), str::to_lowercase);
        let mut pick = DefaultHasher::new();
        (id, dot.animation_turn).hash(&mut pick);
        let entity = EntityState {
            dot,
            turn: self.entities.turn,
            progress: 0.0,
            state,
            pick: pick.finish(),
            color: 0,
            level: 0,
            time: None,
        };
        self.find_ammo(&model, &transform, &entity, None, 0)?
    }

    /// `CModel::FindFirstAmmoProp` over a posed tree: the model's own ammo
    /// point, else the first prop's. `None` while a piece is on its way.
    fn find_ammo(
        &mut self,
        model: &Model,
        transform: &Mat4,
        entity: &EntityState,
        root_id: Option<&str>,
        depth: usize,
    ) -> Option<Option<Vec3>> {
        let Shape::Mesh(file) = model.shape else {
            return Some(None);
        };
        let mesh = match self.store.borrow_mut().mesh(file, false) {
            Get::Ready(mesh) => mesh,
            Get::Pending => return None,
            Get::Missing => return Some(None),
        };
        let skeleton = match mesh.skeleton {
            Some(file) => match self.store.borrow_mut().skeleton(file) {
                Get::Ready(skeleton) => Some(skeleton),
                Get::Pending => return None,
                Get::Missing => return Some(None),
            },
            None => None,
        };
        let chosen = choose_animation(&model.animations, &entity.state, root_id, entity.pick);
        let animation = match (chosen, &skeleton) {
            (Some(chosen), Some(_)) => match self.store.borrow_mut().animation(chosen.file) {
                Get::Ready(data) => Some((chosen, data)),
                Get::Pending => return None,
                Get::Missing => None,
            },
            _ => None,
        };
        let timing = animation.as_ref().map(|(chosen, data)| {
            let (time, looping) = animation_time(entity, chosen, data);
            (data, pose::frames(data, time, looping))
        });
        let place = |point: &PropPoint| {
            let mut placed = *transform;
            if point.bone != NO_BONE
                && let Some(skeleton) = &skeleton
            {
                let bone = pose::bone_matrix(
                    timing.map(|(data, frames)| (data.as_ref(), frames)),
                    skeleton,
                    usize::from(point.bone),
                );
                placed = placed.mul(&bone);
            }
            placed.mul(&point_matrix(point))
        };
        let ammo = model
            .ammo_points
            .iter()
            .rev()
            .find_map(|name| mesh.prop_points.iter().find(|point| point.name == *name));
        if let Some(point) = ammo {
            let m = place(point).m;
            return Some(Some(Vec3::new(m[0][3], m[1][3], m[2][3])));
        }
        if depth >= 8 {
            return Some(None);
        }
        let root_id = root_id.or(animation.as_ref().map(|(chosen, _)| chosen.id.as_str()));
        for (point_name, prop) in &model.props {
            let Some(point) = mesh
                .prop_points
                .iter()
                .find(|point| point.name == *point_name)
            else {
                continue;
            };
            let placed = place(point);
            if let Some(found) = self.find_ammo(prop, &placed, entity, root_id, depth + 1)? {
                return Some(Some(found));
            }
        }
        Some(None)
    }

    /// The model a shot flies as: its projectile actor, else the shooter's
    /// `projectile` prop, as `Attack.js` and `CObjectEntry` pick it.
    fn shot_model(&mut self, shot: &Shot) -> Option<Rc<Model>> {
        let key = (shot.projectile_actor, shot.actor, shot.variants, shot.seed);
        if let Some(model) = self.shot_models.get(&key) {
            return model.clone();
        }
        let named = shot.projectile_actor != 0;
        let actor = if named {
            shot.projectile_actor
        } else {
            shot.actor
        };
        let manifest = match self
            .store
            .borrow_mut()
            .actor(string(&self.entities.strings, actor)?)
        {
            Get::Ready(manifest) => manifest,
            Get::Pending | Get::Missing => return None,
        };
        // The engine seeds a projectile from a counter no other client
        // shares, so any seed is as right.
        let model = if named {
            variation::resolve(&manifest, &BTreeSet::new(), 0)
        } else {
            let mut selections = self.variant_selections(shot.variants);
            // Every attack with a projectile is Ranged, and Attack.js
            // plays "attack_" + type while it shoots: the variant that
            // holds the projectile prop.
            selections.insert(SHOOTING_ANIMATION.to_string());
            variation::resolve(&manifest, &selections, shot.seed)
                .and_then(|shooter| shooter.projectile)
                .and_then(|object| {
                    variation::resolve_object(&manifest, object, &BTreeSet::new(), 0)
                })
        }
        .map(Rc::new);
        self.shot_models.insert(key, model.clone());
        model
    }

    fn impact_model(&mut self, actor: u32) -> Option<Rc<Model>> {
        if let Some(model) = self.impact_models.get(&actor) {
            return model.clone();
        }
        let manifest = match self
            .store
            .borrow_mut()
            .actor(string(&self.entities.strings, actor)?)
        {
            Get::Ready(manifest) => manifest,
            Get::Pending | Get::Missing => return None,
        };
        let model = variation::resolve(&manifest, &BTreeSet::new(), 0).map(Rc::new);
        self.impact_models.insert(actor, model.clone());
        model
    }

    /// Adds this frame's projectiles and impact actors to `groups`, as
    /// `CCmpProjectileManager::Interpolate` keeps them: a projectile that
    /// missed lies on the ground until `DECAY_SECS` after reaching its
    /// target, and the impact actor shows from its landing for its
    /// lifetime.
    fn place_shots(
        &mut self,
        progress: f32,
        view_proj: &[[f32; 4]; 4],
        planes: &[[f32; 4]; 6],
        pixel_scale: f32,
        groups: &mut HashMap<GroupKey, Vec<u8>>,
    ) {
        let Some(ground) = &self.ground else {
            return;
        };
        let image = &ground.image;
        let metres = |point: [i32; 3]| {
            Vec3::new(
                point[0] as f32 / POSITION_UNITS_PER_METRE,
                point[1] as f32 / POSITION_UNITS_PER_METRE,
                point[2] as f32 / POSITION_UNITS_PER_METRE,
            )
        };
        let mut shots = std::mem::take(&mut self.shots);
        let mut placing = Vec::new();
        for (id, flying) in shots.iter_mut() {
            let shot = &flying.shot;
            let flight = *flying.flight.get_or_insert_with(|| {
                let origin = flying.origin.unwrap_or_else(|| metres(shot.launch));
                Flight::new(
                    origin,
                    metres(shot.target),
                    shot.speed as f32 / 256.0,
                    shot.gravity as f32 / 256.0,
                )
                .map(|flight| {
                    let stop = flight.stop_time(|x, z| image.ground(x, z));
                    let at = flight.position(stop);
                    (flight, stop, image.ground(at.x, at.z))
                })
            });
            let Some((flight, stop, rest)) = flight else {
                continue;
            };
            let since = self.entities.turn.saturating_sub(shot.launch_turn) as f32 + progress;
            let t = since * TURN_MS as f32 / 1000.0;
            placing.push((*id, *shot, flight, stop, rest, t));
        }
        self.shots = shots;

        for (id, shot, flight, stop, rest, t) in placing {
            if t - flight.time_hit() <= projectile::DECAY_SECS
                && let Some(model) = self.shot_model(&shot)
            {
                let transform = flight.transform(t, stop, rest);
                let place = ShotPlace {
                    owner: shot.owner,
                    emitter: Owner::Shot(id),
                    time: t * 1000.0,
                    progress,
                };
                self.place_shot_model(
                    &model,
                    &transform,
                    &place,
                    view_proj,
                    planes,
                    pixel_scale,
                    groups,
                );
            }
            let lifetime = shot.impact_lifetime as f32 / 1000.0;
            if shot.impact_actor != 0
                && t >= stop
                && t - stop <= lifetime
                && let Some(model) = self.impact_model(shot.impact_actor)
            {
                let mut transform = Mat4::identity();
                let at = flight.position(stop);
                transform.translate(Vec3::new(at.x, rest, at.z));
                let place = ShotPlace {
                    owner: shot.owner,
                    emitter: Owner::Impact(id),
                    time: (t - stop) * 1000.0,
                    progress,
                };
                self.place_shot_model(
                    &model,
                    &transform,
                    &place,
                    view_proj,
                    planes,
                    pixel_scale,
                    groups,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn place_shot_model(
        &mut self,
        model: &Model,
        transform: &Mat4,
        place: &ShotPlace,
        view_proj: &[[f32; 4]; 4],
        planes: &[[f32; 4]; 6],
        pixel_scale: f32,
        groups: &mut HashMap<GroupKey, Vec<u8>>,
    ) {
        let file = match &model.shape {
            Shape::Mesh(file) => *file,
            Shape::Decal(decal) => {
                self.place_decal(model, decal, transform, true);
                return;
            }
            Shape::Particles(emitter) => {
                if self.particles_visible {
                    self.place_emitter(place.emitter, model, emitter, transform, &Dot::default());
                }
                return;
            }
        };
        let mesh = match self.store.borrow_mut().mesh(file, false) {
            Get::Ready(mesh) => mesh,
            Get::Pending | Get::Missing => return,
        };
        let low = Vec3::new(mesh.min[0], mesh.min[1], mesh.min[2]);
        let high = Vec3::new(mesh.max[0], mesh.max[1], mesh.max[2]);
        let centre = transform.transform(low.lerp(high, 0.5));
        let radius =
            Vec3::new(high.x - low.x, high.y - low.y, high.z - low.z).length() / 2.0 * REACH_MARGIN;
        let Some(level) = screen_level(
            [centre.x, centre.y, centre.z],
            radius,
            view_proj,
            planes,
            pixel_scale,
        ) else {
            return;
        };
        let dot = Dot {
            owner: place.owner,
            ..Dot::default()
        };
        let entity = EntityState {
            dot,
            turn: self.entities.turn,
            progress: place.progress,
            state: "idle".to_string(),
            pick: 0,
            color: owner_color(&dot, &self.palette),
            // Impostors are for entities; a projectile is too small to
            // need one.
            level: level.min(COARSE_LEVEL),
            time: Some(place.time),
        };
        self.emitting = self.particles_visible.then_some(place.emitter);
        self.casting = self.casting_into([centre.x, centre.y, centre.z], radius);
        self.place_model(model, transform, &entity, None, 0, Some(groups));
        self.casting = 0;
        self.emitting = None;
    }

    /// Hands the particle model `model` of `owner` to the emitters, with
    /// `dot`'s entity variables.
    fn place_emitter(
        &mut self,
        owner: Owner,
        model: &Model,
        emitter: &Rc<Emitter>,
        transform: &Mat4,
        dot: &Dot,
    ) {
        let strings = &self.entities.strings;
        let variables = (dot.variables as usize)
            .checked_sub(1)
            .and_then(|index| self.entities.variable_sets.get(index))
            .map_or_else(Vec::new, |set| {
                set.iter()
                    .filter_map(|(name, value)| {
                        Some((strings.get(*name as usize)?.clone(), *value))
                    })
                    .collect()
            });
        self.particles.place(
            owner,
            std::ptr::from_ref(model) as usize,
            emitter,
            transform,
            variables,
        );
    }

    fn upload_ground(&mut self, image: &TerrainImage) {
        let mesh = blend::build(image);
        let water = water::surface(image);
        let waves = water::waves(image);
        let bytes = |indices: &[u32]| -> Vec<u8> {
            indices
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect()
        };
        let buffer = |label: &str, contents: &[u8], usage: wgpu::BufferUsages| {
            create_buffer(&self.device, &self.queue, label, contents, usage)
        };
        let minimap = Texture {
            format: TextureFormat::Rgba8,
            width: image.size,
            height: image.size,
            levels: vec![image.pixels.clone()],
            details: [None; 3],
        };
        let view = upload_texture(&self.device, &self.queue, &minimap, 0, false);
        let minimap = self.ground_bind_group(&view, &self.sampler, None);
        let indices: Vec<u8> = mesh
            .indices
            .iter()
            .flat_map(|index| index.to_le_bytes())
            .collect();
        let (low, high) = image.height_range();
        self.ground = Some(Ground {
            image: image.clone(),
            vertices: create_buffer(
                &self.device,
                &self.queue,
                "terrain",
                &mesh.vertices,
                wgpu::BufferUsages::VERTEX,
            ),
            indices: create_buffer(
                &self.device,
                &self.queue,
                "terrain indices",
                &indices,
                wgpu::BufferUsages::INDEX,
            ),
            base: mesh.base,
            blends: mesh.blends,
            water_vertices: buffer("water", &water.vertices, wgpu::BufferUsages::VERTEX),
            water_indices: buffer(
                "water indices",
                &bytes(&water.indices),
                wgpu::BufferUsages::INDEX,
            ),
            water_index_count: water.indices.len() as u32,
            shore_vertices: buffer(
                "water shore",
                &water.shore_vertices,
                wgpu::BufferUsages::VERTEX,
            ),
            shore_indices: buffer(
                "water shore indices",
                &bytes(&water.shore_indices),
                wgpu::BufferUsages::INDEX,
            ),
            shore_index_count: water.shore_indices.len() as u32,
            wave_vertices: buffer("water waves", &waves.vertices, wgpu::BufferUsages::VERTEX),
            wave_indices: buffer(
                "water wave indices",
                &bytes(&waves.indices),
                wgpu::BufferUsages::INDEX,
            ),
            wave_index_count: waves.indices.len() as u32,
            minimap,
            textures: vec![None; image.names.len()],
            missing: vec![false; image.names.len()],
            alpha_maps: None,
            low,
            high,
        });
    }

    /// A terrain texture's bind group: its view, sampler, mapping and
    /// normal and specular maps; `None` maps the minimap picture over the
    /// map instead.
    fn ground_bind_group(
        &self,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        terrain: Option<(&TerrainTexture, [Option<&wgpu::TextureView>; 2])>,
    ) -> wgpu::BindGroup {
        // CTerrainTextureEntry::GenerateTextureMatrix's _11 and _13.
        let (uniform, [normal, specular]) = match terrain {
            Some((terrain, maps)) => {
                let (sin, cos) = terrain.angle.sin_cos();
                let normal_map = if maps[0].is_some() { 1.0 } else { 0.0 };
                let uniform = [
                    cos / terrain.size,
                    -sin / terrain.size,
                    1.0,
                    normal_map,
                    terrain.normal_strength,
                    terrain.specular_power,
                    if terrain.triplanar { 1.0 } else { 0.0 },
                    0.0,
                ];
                (uniform, maps)
            }
            None => ([0.0; 8], [None; 2]),
        };
        let normal = normal.unwrap_or(&self.default_maps[1]);
        let specular = specular.unwrap_or(&self.default_maps[2]);
        let bytes: Vec<u8> = uniform
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let buffer = create_buffer(
            &self.device,
            &self.queue,
            "terrain mapping",
            &bytes,
            wgpu::BufferUsages::UNIFORM,
        );
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ground texture"),
            layout: &self.ground_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(normal),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(specular),
                },
            ],
        })
    }

    /// Asks for the ground's textures and the blend masks, the detail
    /// straight away since the ground is always close somewhere, and
    /// uploads what has arrived.
    fn refresh_ground(&mut self) {
        let Some(mut ground) = self.ground.take() else {
            return;
        };
        for (at, name) in ground.image.names.iter().enumerate() {
            if ground.missing[at] {
                continue;
            }
            let terrain = match self.store.borrow().terrain(name) {
                Get::Ready(terrain) => terrain,
                Get::Pending => continue,
                Get::Missing => {
                    crate::log(&format!("terrain {name} is not in the asset bake"));
                    ground.missing[at] = true;
                    continue;
                }
            };
            let texture = match self.store.borrow_mut().texture(terrain.texture, true) {
                Get::Ready(texture) => texture,
                Get::Pending => continue,
                Get::Missing => {
                    ground.missing[at] = true;
                    continue;
                }
            };
            // A map on its way is left out until it arrives, and with the
            // material maps off is not fetched.
            let wanted = if self.material_maps {
                [terrain.normal, terrain.specular]
            } else {
                [None; 2]
            };
            let maps = wanted.map(|file| {
                match file.map(|file| self.store.borrow_mut().texture(file, true)) {
                    Some(Get::Ready(texture)) => Some(texture),
                    _ => None,
                }
            });
            let uploaded = [Some(&texture), maps[0].as_ref(), maps[1].as_ref()]
                .map(|texture| texture.map(|texture| self.uploaded(texture)));
            if ground.textures[at]
                .as_ref()
                .is_some_and(|(_, held)| *held == uploaded)
            {
                continue;
            }
            let upload = |texture: &Texture| {
                let first = self.uploaded(texture).1;
                upload_texture(&self.device, &self.queue, texture, first, self.bc)
            };
            let view = upload(&texture);
            let map_views = maps.map(|texture| texture.map(|texture| upload(&texture)));
            let group = self.ground_bind_group(
                &view,
                &self.terrain_sampler,
                Some((&terrain, [map_views[0].as_ref(), map_views[1].as_ref()])),
            );
            ground.textures[at] = Some((group, uploaded));
        }
        if ground.alpha_maps.is_none() {
            let file = self.store.borrow().alpha_maps();
            if let Get::Ready(file) = file
                && let Get::Ready(texture) = self.store.borrow_mut().texture(file, false)
            {
                // Linear and clamped, as the engine's composite.
                let view = upload_texture(&self.device, &self.queue, &texture, 0, false);
                ground.alpha_maps = Some(texture_bind_group(
                    &self.device,
                    &self.texture_layout,
                    &view,
                    &self.sampler,
                ));
            }
        }
        self.ground = Some(ground);
    }
}

impl Field {
    /// Fetches the map's water type's frames while water effects are on,
    /// uploads those in, and once all are, readies what this frame's
    /// water options draw with and writes the water's uniform.
    fn refresh_water(&mut self, progress: f32) {
        let Some(ground) = &self.ground else {
            return;
        };
        if !self.water_effects {
            return;
        }
        let kind = &ground.image.water.kind;
        if self
            .water_normals
            .as_ref()
            .is_none_or(|normals| normals.kind != *kind)
        {
            self.water_group = None;
            self.water_normals = match self.store.borrow().water(kind) {
                Get::Ready(files) => Some(Normals::new(kind.clone(), files)),
                Get::Pending => return,
                Get::Missing => {
                    crate::log(&format!("water type {kind} is not in the asset bake"));
                    Some(Normals::new(kind.clone(), Vec::new()))
                }
            };
        }
        let normals = self.water_normals.as_mut().unwrap();
        if self.water_group.is_none() && normals.files.len() == water::FRAMES {
            let tiers = self.store.borrow().texture_tiers();
            for (frame, file) in normals.files.clone().into_iter().enumerate() {
                let Get::Ready(texture) = self.store.borrow_mut().texture(file, true) else {
                    continue;
                };
                // Whole at the texture quality, or it would be uploaded
                // without the mips still on their way.
                if texture.next_detail(tiers).is_some() {
                    continue;
                }
                if let Err(error) =
                    normals.upload(&self.device, &self.queue, frame, &texture, self.bc)
                {
                    crate::log(&error);
                    normals.files.clear();
                    return;
                }
            }
            if let Some(view) = &normals.view {
                self.water_group = Some(self.water.bind_group(&self.device, view));
            }
        }
        let Some(options) = self.water_options() else {
            return;
        };
        self.water.prepare(&self.device, options);

        let targets = options.refraction || options.reflection;
        if targets != self.water_targets.is_some() {
            self.water_targets = targets
                .then(|| water::Targets::new(&self.device, self.config.format, self.config.height));
            self.water_targets_generation += 1;
        }
        let effects_size = self.scaled_size();
        if options.fancy
            != self
                .water_effects_targets
                .as_ref()
                .is_some_and(|effects| effects.size == effects_size)
        {
            self.water_effects_targets = options
                .fancy
                .then(|| water::Effects::new(&self.device, effects_size.0, effects_size.1));
            self.water_targets_generation += 1;
        }
        // Asked for apart, as the store is borrowed again below.
        let wave_files = self.store.borrow().wave_textures();
        if options.fancy
            && self.waves_group.is_none()
            && let Get::Ready((foam, coastal_wave)) = wave_files
        {
            let textures = [coastal_wave, foam].map(|file| {
                match self.store.borrow_mut().texture(file, true) {
                    Get::Ready(texture) => Some(texture),
                    Get::Pending | Get::Missing => None,
                }
            });
            if let [Some(wave), Some(foam)] = textures {
                let [wave, foam] = [wave, foam].map(|texture| {
                    let first = self.uploaded(&texture).1;
                    upload_texture(&self.device, &self.queue, &texture, first, self.bc)
                });
                self.waves_group =
                    Some(
                        self.waves
                            .bind_group(&self.device, &self.water.buffer, &wave, &foam),
                    );
            }
        }
        if options.reflection && self.reflection_scene.is_none() {
            self.reflection_scene = Some(ScenePipelines::new(
                &self.device,
                &self.scene_sources,
                WaterClip::Above,
            ));
        }
        if options.refraction && self.refraction_scene.is_none() {
            self.refraction_scene = Some(ScenePipelines::new(
                &self.device,
                &self.scene_sources,
                WaterClip::Below,
            ));
        }
        let key = (self.sky.generation(), self.water_targets_generation);
        if self
            .water_environment
            .as_ref()
            .is_none_or(|(sky, targets, _)| (*sky, *targets) != key)
        {
            let group = self.water.environment_group(
                &self.device,
                self.sky.view(),
                self.water_targets.as_ref(),
                self.water_effects_targets.as_ref(),
            );
            self.water_environment = Some((key.0, key.1, group));
        }

        let camera = self.refraction_camera();
        let (_, eye, right, up) = self.camera_view();
        let wind = self.wind(progress);
        if options.refraction {
            self.write_view(
                &self.refraction_view.0,
                &camera.view_proj,
                [right, up, eye],
                wind,
            );
        }
        if options.reflection {
            self.write_view(
                &self.reflection_view.0,
                &camera.reflection,
                [right, up, eye],
                wind,
            );
        }
        let Some(ground) = &self.ground else {
            return;
        };
        // m_WaterTexTimer: the sim time since the match began.
        let uniform = water::uniform(
            &ground.image,
            self.sim_time(progress),
            self.light().rotation,
            &camera,
        );
        self.queue.write_buffer(&self.water.buffer, 0, &uniform);
    }

    /// The resolution scale in effect: `RecalculateSize` keeps 1 without
    /// post-processing.
    fn scale(&self) -> f32 {
        if self.postproc {
            self.render_scale
        } else {
            1.0
        }
    }

    /// The scene's size at the resolution scale.
    fn scaled_size(&self) -> (u32, u32) {
        let max = self.device.limits().max_texture_dimension_2d;
        let scale = |pixels: u32| ((pixels as f32 * self.scale()).round() as u32).clamp(1, max);
        (scale(self.config.width), scale(self.config.height))
    }

    fn resize_post(&mut self) {
        let screen = (self.config.width, self.config.height);
        let scaled = self.scaled_size();
        self.post.resize(&self.device, screen, scaled, self.upscale);
    }

    /// The water options this frame draws with: `None` while the flat
    /// plane stands in, with the water effects off or their frames on
    /// their way, or on a map without water.
    fn water_options(&self) -> Option<water::Options> {
        let drawn = self.water_effects
            && self.water_group.is_some()
            && self
                .ground
                .as_ref()
                .is_some_and(|ground| ground.water_index_count > 0);
        drawn.then_some(water::Options {
            refraction: self.refraction,
            real_depth: self.refraction && self.real_depth,
            reflection: self.reflection,
            fancy: self.fancy_water,
        })
    }
}

impl View for Field {
    fn max_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    fn resize(&mut self, width: u32, height: u32, device_pixel_ratio: f32) {
        self.device_pixel_ratio = device_pixel_ratio;
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.resize_post();
        if self.water_targets.as_ref().is_some_and(|targets| {
            targets.side != water::Targets::side(&self.device, self.config.height)
        }) {
            self.water_targets = Some(water::Targets::new(
                &self.device,
                self.config.format,
                self.config.height,
            ));
            self.water_targets_generation += 1;
        }
    }

    fn animating(&self) -> bool {
        true
    }

    fn set_projectiles(&mut self, on: bool) {
        self.projectiles = on;
    }

    fn set_shadows(&mut self, size: u32) {
        self.shadow.set_size(&self.device, size);
    }

    fn set_shadow_cutoff(&mut self, metres: f32) {
        self.shadow.set_cutoff(metres);
    }

    fn max_texture_tiers(&self) -> usize {
        max_texture_tiers(self.bc)
    }

    fn set_texture_tiers(&mut self, tiers: usize) {
        self.texture_tiers = tiers.clamp(1, max_texture_tiers(self.bc));
        self.store
            .borrow_mut()
            .set_texture_tiers(self.texture_tiers);
    }

    fn set_material_maps(&mut self, on: bool) {
        self.material_maps = on;
        self.store.borrow_mut().set_material_maps(on);
    }

    fn set_triplanar(&mut self, on: bool) {
        self.triplanar = on;
    }

    fn set_wind(&mut self, on: bool) {
        self.wind = on;
    }

    fn set_postproc(&mut self, on: bool) {
        self.postproc = on;
        self.resize_post();
    }

    fn set_bloom(&mut self, on: bool) {
        self.bloom = on;
    }

    fn set_dof(&mut self, on: bool) {
        self.dof = on;
    }

    fn set_antialiasing(&mut self, on: bool) {
        self.antialiasing = on;
    }

    fn set_sharpening(&mut self, on: bool) {
        self.sharpening = on;
    }

    fn set_sharpness(&mut self, sharpness: f32) {
        self.sharpness = sharpness;
    }

    fn set_render_scale(&mut self, scale: f32) {
        self.render_scale = scale;
        self.resize_post();
    }

    fn set_upscale(&mut self, upscale: Upscale) {
        self.upscale = upscale;
        self.resize_post();
    }

    fn set_fog(&mut self, on: bool) {
        self.fog = on;
    }

    fn set_water_effects(&mut self, on: bool) {
        self.water_effects = on;
    }

    fn set_sky(&mut self, on: bool) {
        self.sky_visible = on;
    }

    fn set_particles(&mut self, on: bool) {
        self.particles_visible = on;
        if !on {
            self.particles.clear();
        }
    }

    fn set_water_refraction(&mut self, on: bool) {
        self.refraction = on;
    }

    fn set_water_real_depth(&mut self, on: bool) {
        self.real_depth = on;
    }

    fn set_water_reflection(&mut self, on: bool) {
        self.reflection = on;
    }

    fn set_fancy_water(&mut self, on: bool) {
        self.fancy_water = on;
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
            self.bounds.clear();
            self.far.clear();
            self.shots.clear();
            self.shot_models.clear();
            self.impact_models.clear();
            entities.match_id = scene.match_id;
            entities.templates.clear();
            entities.template_details.clear();
            entities.strings.clear();
            entities.variant_sets.clear();
            entities.variable_sets.clear();
            entities.range_sets.clear();
            self.particles.clear();
            self.camera.borrow_mut().placed = false;
            self.selection = Selection::default();
        }
        // The tables only grow within a match, so only the tail is new.
        if scene.templates.len() < entities.templates.len() {
            entities.templates.clear();
            entities.template_details.clear();
        }
        let known = entities.templates.len();
        entities
            .templates
            .extend(scene.templates[known..].iter().cloned());
        entities
            .template_details
            .extend(scene.template_details[known..].iter().copied());
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
        if scene.variable_sets.len() < entities.variable_sets.len() {
            entities.variable_sets.clear();
        }
        let known = entities.variable_sets.len();
        entities
            .variable_sets
            .extend(scene.variable_sets[known..].iter().cloned());
        if scene.range_sets.len() < entities.range_sets.len() {
            entities.range_sets.clear();
        }
        let known = entities.range_sets.len();
        entities
            .range_sets
            .extend(scene.range_sets[known..].iter().cloned());
        entities.turn = scene.turn;
        entities.map_size = scene.map_size;
        entities.dots.clear();
        entities
            .dots
            .extend(scene.dots.iter().map(|(id, dot)| (*id, *dot)));
        self.shots.retain(|id, _| scene.shots.contains_key(id));
        for (id, shot) in &scene.shots {
            if self.shots.contains_key(id) {
                continue;
            }
            // Only on its launch turn is the shooter where it shot from.
            let origin = if self.projectiles && shot.launch_turn == scene.turn {
                self.ammo_point(shot.shooter)
            } else {
                None
            };
            self.shots.insert(
                *id,
                Flying {
                    shot: *shot,
                    origin,
                    flight: None,
                },
            );
        }
        let renames = std::mem::take(&mut self.renames);
        let entities = &self.entities;
        self.selection
            .update(&renames, &|id| selectee(entities, id));
        self.palette = palette.to_vec();
        self.borders
            .set_territory(&scene.territory, scene.territory_generation, palette);
        let mut camera = self.camera.borrow_mut();
        if !camera.placed && scene.map_size > 0 {
            camera.placed = true;
            camera.target = start_point(scene);
        }
    }

    /// As `CMiniMap::DrawViewRect`: the screen's corners, bottom left
    /// first and anticlockwise, cast onto the plane at the water level.
    fn view_outline(&self) -> Option<[[f32; 2]; 4]> {
        if self.entities.map_size == 0 {
            return None;
        }
        let (_, eye, right, up) = self.view_projection();
        let forward = [
            right[1] * up[2] - right[2] * up[1],
            right[2] * up[0] - right[0] * up[2],
            right[0] * up[1] - right[1] * up[0],
        ];
        let height = self
            .ground
            .as_ref()
            .map_or(0.0, |ground| ground.image.water_level);
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let half = (FIELD_OF_VIEW / 2.0).tan();
        let corner = |nx: f32, ny: f32| {
            let ray: [f32; 3] = std::array::from_fn(|i| {
                forward[i] + right[i] * nx * aspect * half + up[i] * ny * half
            });
            let t = (height - eye[1]) / ray[1];
            // A corner over the horizon (the camera tilts further up than
            // the engine's) meets the plane behind the eye or nowhere; the
            // ground under its far end keeps the outline a quad.
            let length = ray.iter().map(|value| value * value).sum::<f32>().sqrt();
            let t = if ray[1] < 0.0 && t * length <= FAR {
                t
            } else {
                FAR / length
            };
            [eye[0] + ray[0] * t, eye[2] + ray[2] * t]
        };
        Some([
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ])
    }

    fn move_to(&mut self, x: f32, z: f32) {
        self.camera.borrow_mut().target = [x, z];
    }

    fn asset_store(&self) -> Option<Rc<RefCell<Store>>> {
        Some(Rc::clone(&self.store))
    }

    fn camera(&self) -> Option<CameraPose> {
        let camera = self.camera.borrow();
        Some(CameraPose {
            target: camera.target,
            yaw: camera.yaw + camera.yaw_ahead,
            pitch: camera.pitch + camera.pitch_ahead,
            distance: camera.distance + camera.distance_ahead,
            birds_eye: camera.birds_eye,
        })
    }

    fn set_camera(&mut self, pose: CameraPose) {
        let camera = &mut *self.camera.borrow_mut();
        camera.target = pose.target;
        camera.yaw = pose.yaw;
        camera.pitch = pose.pitch;
        camera.distance = pose.distance;
        camera.birds_eye = pose.birds_eye;
        camera.yaw_ahead = 0.0;
        camera.pitch_ahead = 0.0;
        camera.distance_ahead = 0.0;
        camera.keys = 0;
    }

    fn bench_info(&self) -> BenchInfo {
        BenchInfo {
            adapter: self.stats.adapter.clone(),
            backend: self.stats.backend,
            gpu_timed: self.timer.measures_gpu(),
            canvas: [self.config.width, self.config.height],
            device_pixel_ratio: self.device_pixel_ratio,
            entities: self.entities.dots.len() as u32,
            in_view: self.stats.levels.iter().sum(),
            models: self.stats.models,
        }
    }

    fn listener(&self) -> Option<Listener> {
        let (view_projection, eye, _, _) = self.view_projection();
        Some(Listener {
            view_projection,
            eye,
            width: self.config.width as f32 / self.device_pixel_ratio,
            height: self.config.height as f32 / self.device_pixel_ratio,
        })
    }

    fn take_animation_sounds(&mut self, progress: f32) -> Vec<AnimationSound> {
        self.animation_sounds(progress)
    }

    fn set_selection_atlas(&mut self, bytes: &[u8]) {
        if let Err(error) = self.overlays.set_atlas(&self.device, &self.queue, bytes) {
            web_sys::console::error_1(&format!("selection.bin: {error}").into());
        }
    }

    fn set_renames(&mut self, renames: &[(u32, u32)]) {
        self.renames.extend_from_slice(renames);
    }

    fn set_terrain(&mut self, image: Option<&TerrainImage>) {
        match image {
            Some(image) if image.size > 0 => self.upload_ground(image),
            _ => self.ground = None,
        }
        self.borders.ground_changed();
    }

    fn draw(&mut self, progress: f32) -> bool {
        let acquiring = now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            _ => return false,
        };
        let start = now();
        // Counted as they run, so the bench can set apart the frames that
        // baked or uploaded.
        let bakes = self.far.drawn;
        let uploads = self.far.from_storage as u64 + self.uploads;
        self.camera.borrow_mut().step(now());
        let (view_proj, eye, right, up) = self.view_projection();
        let wind = self.wind(progress);
        self.write_view(&self.view_buffer, &view_proj, [right, up, eye], wind);
        self.eye = eye;
        let bounds = self.ground.as_ref().map(|ground| shadow::Bounds {
            size: ground.image.size as f32 * TILE_SIZE,
            low: ground.low,
            high: ground.high,
        });
        let light = self.light();
        let post = postproc::Plan::new(
            &light,
            self.postproc,
            self.bloom,
            self.dof,
            self.antialiasing,
            self.sharpening,
            self.scale(),
        );
        if post.is_some() {
            self.post
                .prepare(&self.queue, &light, self.sharpness, NEAR, FAR);
        }
        let casters = self.shadow.prepare(
            &self.queue,
            &light,
            self.fog,
            &shadow::Eye {
                position: eye,
                forward: [
                    right[1] * up[2] - right[2] * up[1],
                    right[2] * up[0] - right[0] * up[2],
                    right[0] * up[1] - right[1] * up[0],
                ],
                near: NEAR,
                field_of_view: FIELD_OF_VIEW,
                aspect: self.config.width as f32 / self.config.height.max(1) as f32,
            },
            bounds.as_ref(),
            wind,
        );
        if let Some(bake) = self.store.borrow().index_name() {
            self.far.open_storage(bake);
        }
        self.far.begin_frame(&self.device);
        let shadowed = casters.is_some();
        self.place_entities(progress, &view_proj, casters);
        self.apply_picks(&view_proj, eye, right, up);
        let time = now();
        self.selection
            .tick(((time - self.last_fade) / 1000.0) as f32);
        self.last_fade = time;
        let ranges = self.camera.borrow().ranges;
        let marks = overlay_marks(
            &self.pickables,
            &self.entities,
            &self.selection,
            &self.palette,
            ranges,
        );
        self.overlays.prepare(
            &self.device,
            &self.queue,
            &marks,
            self.ground.as_ref().map(|ground| &ground.image),
        );
        self.far.take_loaded(&self.device, &self.queue);
        let mut prepared = Vec::new();
        for (key, request) in self.far.next_bakes(&self.device) {
            match self.prepare_capture(&request) {
                Some(capture) => prepared.push((key, request, capture)),
                None => self.far.defer(key, request),
            }
        }
        self.refresh_ground();
        if let Some(ground) = &self.ground
            && (self.sky_visible || self.water_effects)
        {
            self.sky
                .refresh(&self.device, &self.queue, &self.store, &ground.image.sky);
        }
        if self.sky_visible {
            let tan = (FIELD_OF_VIEW / 2.0).tan();
            let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
            self.sky.prepare(
                &self.queue,
                right.map(|value| value * tan * aspect),
                up.map(|value| value * tan),
                cross(right, up),
                light.rotation,
            );
        }
        self.refresh_water(progress);
        assets::pump(&self.store);
        self.poses.upload(&self.device, &self.queue);
        if self.poses.generation != self.pose_generation {
            self.pose_generation = self.poses.generation;
            self.pose_bind_group =
                pose_bind_group(&self.device, &self.pose_layout, &self.poses.view);
        }
        self.bake(prepared);

        let draws = upload_groups(&self.device, &self.queue, &self.groups, &mut self.instances);
        let shadow_draws: Vec<_> = self
            .shadow_groups
            .iter()
            .zip(&mut self.shadow_instances)
            .take(if shadowed { self.shadow.cascades() } else { 0 })
            .map(|(groups, instances)| upload_groups(&self.device, &self.queue, groups, instances))
            .collect();
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
        // One draw per decal, each reading its own instance: WebGL2 has no
        // base instance to share one draw among them.
        let mut decal_bytes = Vec::new();
        let mut decal_indices: Vec<u32> = Vec::new();
        let mut decal_draws = Vec::new();
        if let Some(ground) = &self.ground {
            for decal in &self.decals {
                let Some([i0, j0, i1, j1]) = decal_extents(ground.image.size, &decal.place) else {
                    continue;
                };
                let first = decal_indices.len() as u32;
                for j in j0..j1 {
                    for i in i0..i1 {
                        decal_indices.extend_from_slice(&blend::tile_triangles(
                            &ground.image,
                            i,
                            j,
                        ));
                    }
                }
                let start = decal_bytes.len() as u64;
                for value in decal.place {
                    decal_bytes.extend_from_slice(&value.to_le_bytes());
                }
                let count = decal_indices.len() as u32 - first;
                decal_draws.push((decal.texture, start, first, count));
            }
        }
        if decal_bytes.len() as u64 > self.decal_instances.size() {
            self.decal_instances = create_buffer(
                &self.device,
                &self.queue,
                "decal instances",
                &vec![0; (decal_bytes.len() as u64).next_power_of_two() as usize],
                wgpu::BufferUsages::VERTEX,
            );
        }
        if !decal_bytes.is_empty() {
            self.queue
                .write_buffer(&self.decal_instances, 0, &decal_bytes);
        }
        let decal_index_bytes: Vec<u8> = decal_indices
            .iter()
            .flat_map(|index| index.to_le_bytes())
            .collect();
        if decal_index_bytes.len() as u64 > self.decal_indices.size() {
            self.decal_indices = create_buffer(
                &self.device,
                &self.queue,
                "decal indices",
                &vec![0; (decal_index_bytes.len() as u64).next_power_of_two() as usize],
                wgpu::BufferUsages::INDEX,
            );
        }
        if !decal_index_bytes.is_empty() {
            self.queue
                .write_buffer(&self.decal_indices, 0, &decal_index_bytes);
        }

        let (particle_bytes, particle_draws) = if self.particles_visible {
            self.particles.instances(eye)
        } else {
            (Vec::new(), Vec::new())
        };
        for draw in &particle_draws {
            // Uploads it once it is in; until then the emitter is skipped.
            let _ = self.texture(draw.texture, true);
        }
        if particle_bytes.len() as u64 > self.particle_instances.size() {
            self.particle_instances = create_buffer(
                &self.device,
                &self.queue,
                "particle instances",
                &vec![0; (particle_bytes.len() as u64).next_power_of_two() as usize],
                wgpu::BufferUsages::VERTEX,
            );
        }
        if !particle_bytes.is_empty() {
            self.queue
                .write_buffer(&self.particle_instances, 0, &particle_bytes);
        }
        let particle_texture =
            |file: FileRef| self.textures.get(&file.hash).map(|texture| &texture.group);

        self.borders.prepare(
            &self.device,
            &self.queue,
            self.ground.as_ref().map(|ground| &ground.image),
            2.0 * (FIELD_OF_VIEW / 2.0).tan() / self.scaled_size().1 as f32,
            self.device_pixel_ratio * self.scaled_size().1 as f32 / self.config.height as f32,
            territory::blink_seconds(now()),
        );
        let screen = frame.texture.create_view(&Default::default());
        let target = if post.is_some() {
            self.post.scene()
        } else {
            &screen
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.timer.begin();
        self.draw_calls.set(0);
        if shadowed
            && let Some(mut pass) = self
                .shadow
                .begin(&mut encoder, self.timer.writes(timing::SHADOW, true, true))
        {
            // Each cascade's own casters; the viewport clips them to its
            // quarter.
            for (cascade, draws) in shadow_draws.iter().enumerate() {
                self.shadow.select(&mut pass, cascade);
                if let Some(ground) = &self.ground {
                    pass.set_pipeline(&self.shadow.terrain_pipeline);
                    pass.set_vertex_buffer(0, ground.vertices.slice(..));
                    pass.set_index_buffer(ground.indices.slice(..), wgpu::IndexFormat::Uint32);
                    for splat in &ground.base {
                        pass.draw_indexed(splat.first..splat.first + splat.count, 0, 0..1);
                    }
                    self.count_draws(ground.base.len());
                }
                pass.set_pipeline(&self.shadow.model_pipeline);
                pass.set_bind_group(1, &self.pose_bind_group, &[]);
                self.draw_models(&mut pass, draws, &self.shadow_instances[cascade]);
            }
        }
        let water_options = self.water_options();
        if water_options.is_some_and(|options| options.refraction)
            && let (Some(targets), Some(scene)) = (&self.water_targets, &self.refraction_scene)
        {
            // RenderRefractions: what is under the water, cleared to an
            // alpha of 0 where nothing is, which the blur leaves out.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water refraction"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.refraction,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.refraction_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.timer.writes(timing::REFRACTION, true, true),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.refraction_view.1, &[]);
            self.draw_ground(&mut pass, scene);
            self.draw_scene_models(&mut pass, scene, &draws);
        }
        if water_options.is_some_and(|options| options.fancy)
            && let (Some(effects), Some(ground)) = (&self.water_effects_targets, &self.ground)
        {
            // RenderWaves, then RenderWaterFoamOccluders over them.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water effects"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &effects.color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &effects.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.timer.writes(timing::WAVES, true, true),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.view_bind_group, &[]);
            if let Some(group) = &self.waves_group
                && ground.wave_index_count > 0
            {
                pass.set_pipeline(&self.waves.wave_pipeline);
                pass.set_bind_group(1, group, &[]);
                pass.set_vertex_buffer(0, ground.wave_vertices.slice(..));
                pass.set_index_buffer(ground.wave_indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..ground.wave_index_count, 0, 0..1);
            }
            if ground.shore_index_count > 0 {
                pass.set_pipeline(&self.waves.occluder_pipeline);
                pass.set_vertex_buffer(0, ground.shore_vertices.slice(..));
                pass.set_index_buffer(ground.shore_indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..ground.shore_index_count, 0, 0..1);
            }
        }
        if water_options.is_some_and(|options| options.reflection)
            && let (Some(targets), Some(scene)) = (&self.water_targets, &self.reflection_scene)
        {
            // RenderReflections: what is over the water, mirrored, cleared
            // to an alpha of 0 where nothing is, which the sky fills.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water reflection"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.reflection,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.5,
                            g: 0.5,
                            b: 1.0,
                            a: 0.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.reflection_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.timer.writes(timing::REFLECTION, true, true),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.reflection_view.1, &[]);
            self.draw_ground(&mut pass, scene);
            self.draw_scene_models(&mut pass, scene, &draws);
            // The engine draws them into the reflection, not the
            // refraction.
            self.reflection_particles.draw(
                &mut pass,
                &particle_draws,
                &self.particle_instances,
                &self.shadow.bind_group,
                particle_texture,
            );
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("field"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BACKGROUND),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self.post.depth(),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: if post.is_some_and(|plan| plan.reads_depth()) {
                            wgpu::StoreOp::Store
                        } else {
                            wgpu::StoreOp::Discard
                        },
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.timer.writes(timing::MAIN, true, true),
                ..Default::default()
            });
            if self.sky_visible && self.ground.is_some() {
                self.sky.draw(&mut pass);
            }
            pass.set_bind_group(0, &self.view_bind_group, &[]);
            self.draw_ground(&mut pass, &self.scene);
            if self.ground.is_some() && !decal_draws.is_empty() {
                pass.set_pipeline(&self.decal_pipeline);
                pass.set_bind_group(2, &self.shadow.bind_group, &[]);
                pass.set_index_buffer(self.decal_indices.slice(..), wgpu::IndexFormat::Uint32);
                for (texture, start, first, count) in &decal_draws {
                    let Some(GpuTexture { group, .. }) = self.textures.get(texture) else {
                        continue;
                    };
                    let end = start + DECAL_INSTANCE_SIZE as u64;
                    pass.set_bind_group(1, group, &[]);
                    pass.set_vertex_buffer(1, self.decal_instances.slice(*start..end));
                    pass.draw_indexed(*first..first + count, 0, 0..1);
                }
            }
            self.draw_scene_models(&mut pass, &self.scene, &draws);
            if !far_draws.is_empty() {
                pass.set_pipeline(&self.far.pipeline);
                pass.set_bind_group(2, &self.shadow.bind_group, &[]);
                for (key, start, end, count) in &far_draws {
                    let Some(look) = self.far.ready(*key) else {
                        continue;
                    };
                    pass.set_bind_group(1, &look.atlas.bind_group, &[]);
                    pass.set_vertex_buffer(0, self.far_instances.slice(*start..*end));
                    pass.draw(0..4, 0..*count);
                }
            }
            if let Some(ground) = &self.ground {
                let fancy = water_options.and_then(|options| {
                    Some((
                        self.water.pipeline(options)?,
                        self.water_group.as_ref()?,
                        &self.water_environment.as_ref()?.2,
                    ))
                });
                match fancy {
                    Some((pipeline, group, environment)) => {
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(1, group, &[]);
                        pass.set_bind_group(2, environment, &[]);
                        pass.set_bind_group(3, &self.shadow.bind_group, &[]);
                        pass.set_vertex_buffer(0, ground.water_vertices.slice(..));
                        pass.set_index_buffer(
                            ground.water_indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..ground.water_index_count, 0, 0..1);
                        // RenderWaterShore, which only the fancy effects
                        // draw.
                        if water_options.is_some_and(|options| options.fancy)
                            && ground.shore_index_count > 0
                        {
                            pass.set_vertex_buffer(0, ground.shore_vertices.slice(..));
                            pass.set_index_buffer(
                                ground.shore_indices.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.draw_indexed(0..ground.shore_index_count, 0, 0..1);
                        }
                    }
                    // A map without water draws none, as the engine.
                    None if water_options.is_some() => {}
                    None => {
                        pass.set_pipeline(&self.water_pipeline);
                        // WebGL2 numbers uniform slots across groups, so
                        // the light's slot here differs from the models'
                        // (no uniforms in groups 1 and 2), yet wgpu keeps
                        // group 3 bound as compatible and drops a repeat
                        // of the same group; clearing it forces a rebind.
                        pass.set_bind_group(3, None, &[]);
                        pass.set_bind_group(3, &self.shadow.bind_group, &[]);
                        pass.draw(0..4, 0..1);
                    }
                }
                // After the water, as the engine draws overlays, so a line
                // floating on it shows.
                self.borders.draw(&mut pass);
                self.overlays.draw(&mut pass);
            }
            // Last, as the engine draws them after the water and the
            // overlays.
            pass.set_bind_group(0, &self.view_bind_group, &[]);
            self.particle_pipelines.draw(
                &mut pass,
                &particle_draws,
                &self.particle_instances,
                &self.shadow.bind_group,
                particle_texture,
            );
        }
        if let Some(plan) = post {
            self.post.draw(&mut encoder, plan, &screen, &self.timer);
        }
        self.timer.resolve(&mut encoder);
        let encoded = now();
        self.queue.submit([encoder.finish()]);
        self.timer.submitted(&self.queue);
        self.queue.present(frame);
        timing::draw(timing::Draw {
            acquire_ms: start - acquiring,
            encode_ms: encoded - start,
            submit_ms: now() - encoded,
            draws: self.draw_calls.get(),
            bakes: self.far.drawn - bakes,
            uploads: (self.far.from_storage as u64 + self.uploads - uploads) as u32,
        });
        self.show_stats(draws.len() + far_draws.len());
        true
    }
}

impl Field {
    /// Instanced model draws, the pipeline and the groups below 2 set.
    /// The ground's base and blend passes with `scene`'s pipelines, the
    /// view's group set.
    fn draw_ground(&self, pass: &mut wgpu::RenderPass, scene: &ScenePipelines) {
        let Some(ground) = &self.ground else {
            return;
        };
        let group = |texture: u16| {
            ground
                .textures
                .get(usize::from(texture))
                .and_then(Option::as_ref)
                .map_or(&ground.minimap, |(group, _)| group)
        };
        let maps = usize::from(self.material_maps);
        let triplanar = usize::from(self.triplanar);
        pass.set_pipeline(&scene.terrain[maps][triplanar]);
        pass.set_bind_group(3, &self.shadow.bind_group, &[]);
        pass.set_vertex_buffer(0, ground.vertices.slice(..));
        pass.set_index_buffer(ground.indices.slice(..), wgpu::IndexFormat::Uint32);
        for splat in &ground.base {
            pass.set_bind_group(1, group(splat.texture), &[]);
            pass.draw_indexed(splat.first..splat.first + splat.count, 0, 0..1);
        }
        self.count_draws(ground.base.len());
        if let Some(alpha_maps) = &ground.alpha_maps {
            pass.set_pipeline(&scene.blend[maps][triplanar]);
            pass.set_bind_group(2, alpha_maps, &[]);
            for splat in &ground.blends {
                pass.set_bind_group(1, group(splat.texture), &[]);
                pass.draw_indexed(splat.first..splat.first + splat.count, 0, 0..1);
            }
            self.count_draws(ground.blends.len());
        }
    }

    /// The near models with `scene`'s pipeline, the view's group set.
    fn draw_scene_models(
        &self,
        pass: &mut wgpu::RenderPass,
        scene: &ScenePipelines,
        draws: &[(GroupKey, u64, u64, u32)],
    ) {
        if draws.is_empty() {
            return;
        }
        pass.set_pipeline(&scene.model[usize::from(self.material_maps)]);
        pass.set_bind_group(1, &self.pose_bind_group, &[]);
        pass.set_bind_group(3, &self.shadow.bind_group, &[]);
        self.draw_models(pass, draws, &self.instances);
    }

    fn draw_models(
        &self,
        pass: &mut wgpu::RenderPass,
        draws: &[(GroupKey, u64, u64, u32)],
        instances: &wgpu::Buffer,
    ) {
        for ((mesh, level, material), start, end, count) in draws {
            let Some(gpu) = self.meshes.get(mesh) else {
                continue;
            };
            let group = self
                .materials
                .get(material)
                .map_or(&self.default_material, |(group, _)| group);
            let (first, index_count) = gpu.levels[*level];
            pass.set_bind_group(2, group, &[]);
            pass.set_vertex_buffer(0, gpu.vertices.slice(..));
            pass.set_vertex_buffer(1, instances.slice(*start..*end));
            pass.set_index_buffer(gpu.indices.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(first..first + index_count, 0, 0..*count);
            self.count_draws(1);
        }
    }

    fn count_draws(&self, draws: usize) {
        self.draw_calls.set(self.draw_calls.get() + draws as u32);
    }

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
        let live_ms = stream.live_ms + (now - stream.received_at);
        // Bytes per millisecond times 8 is kbit/s.
        let average_kbps = if live_ms > 0.0 {
            stream.live as f64 * 8.0 / live_ms
        } else {
            0.0
        };
        let peak_kbps = stream.peak_window as f64 * 8.0 / (PEAK_WINDOW_S as f64 * 1000.0);
        let (held, held_bytes, waiting) = self.far.held();
        let seconds = span / 1000.0;
        let uploads = f64::from(std::mem::take(&mut self.far.uploads)) / seconds;
        let bakes = f64::from(std::mem::take(&mut self.far.bakes)) / seconds;
        let times = timing::take();
        let gpu_measured = self.timer.measures_gpu();
        let done_measured = stats.backend == "WebGPU";
        let ms = |span: timing::Span, measured: bool| {
            if !measured {
                "n/a".to_string()
            } else {
                span.average().map_or("-".into(), |ms| format!("{ms:.1}"))
            }
        };
        stats.element.set_text_content(Some(&format!(
            "FPS {rate:>8.0}\nCPU {:>5} ms\nGPU {:>5} ms\n{:>12}",
            ms(times.frame, true),
            ms(times.gpu, gpu_measured),
            stats.backend,
        )));

        let mut pane = Pane::new("Profiling, ms", "avg", "max");
        pane.span("CPU frame", times.frame, true);
        pane.span("  acquire", times.acquire, true);
        pane.span("  encode", times.encode, true);
        pane.span("  submit", times.submit, true);
        pane.row("  idle frames", times.idle, "");
        pane.span("GPU frame", times.gpu, gpu_measured);
        for (label, span) in ["  shadow", "  water", "  main", "  post"]
            .into_iter()
            .zip(times.groups)
        {
            pane.span(label, span, gpu_measured);
        }
        pane.span("Submit to done", times.done, done_measured);
        pane.row("In view", high + medium + low + impostors, "");
        pane.row("  mesh high", high, "");
        pane.row("  mesh medium", medium, "");
        pane.row("  mesh low", low, "");
        pane.row("  impostors", impostors, "");
        pane.row("Models", stats.models, "");
        pane.row("Draws, main pass", draws, "");
        pane.row(
            "Model+ground draws",
            times
                .draws
                .average()
                .map_or("-".into(), |draws| format!("{draws:.0}")),
            "",
        );
        pane.row("Impostor uploads", format!("{uploads:.1}"), "/s");
        pane.row("Impostor bakes", format!("{bakes:.1}"), "/s");
        stats.profiling.set_text_content(Some(&pane.0));

        let mut pane = Pane::new("Network", "", "");
        pane.row(
            "Stream",
            format!("{:.2}", mb(stream.live + stream.segments + stream.terrain)),
            "MB",
        );
        pane.row("  live", format!("{:.2}", mb(stream.live)), "MB");
        pane.row("  history", format!("{:.2}", mb(stream.segments)), "MB");
        pane.row("  terrain", format!("{:.2}", mb(stream.terrain)), "MB");
        pane.row("Live average", format!("{average_kbps:.0}"), "kbit/s");
        pane.row(
            &format!("Live peak, {PEAK_WINDOW_S} s"),
            format!("{peak_kbps:.0}"),
            "kbit/s",
        );
        pane.row(
            "Assets downloaded",
            format!("{:.1}", mb(assets.downloaded)),
            "MB",
        );
        stats.network.set_text_content(Some(&pane.0));

        let mut pane = Pane::new("Memory & storage", "", "");
        pane.row("Asset files", assets.files, "");
        pane.row("Assets in memory", format!("{:.1}", mb(assets.bytes)), "MB");
        pane.row(
            "Assets from cache",
            format!("{:.1}", mb(assets.cached)),
            "MB",
        );
        pane.row("Impostor atlases", held, "");
        pane.row("  in memory", format!("{:.0}", mb(held_bytes)), "MB");
        pane.row("  budget", format!("{:.0}", mb(self.far.budget())), "MB");
        pane.row("  waiting for room", waiting, "");
        pane.row("  from storage", self.far.from_storage, "");
        pane.row("  drawn", self.far.drawn, "");
        stats.storage.set_text_content(Some(&pane.0));
        stats.frames = 0;
        stats.since = now;
    }
}

/// A debug pane's text in fixed columns, so the numbers stay put as they
/// change in a monospace box.
struct Pane(String);

impl Pane {
    const LABEL: usize = 20;
    const VALUE: usize = 8;

    fn new(title: &str, first: &str, second: &str) -> Self {
        let mut text = format!("{title:<width$}", width = Self::LABEL);
        if !first.is_empty() {
            text += &format!("{first:>width$}{second:>width$}", width = Self::VALUE);
        }
        Pane(text)
    }

    fn row(&mut self, label: &str, value: impl std::fmt::Display, unit: &str) {
        self.0 += &format!(
            "\n{label:<label_width$}{value:>value_width$} {unit}",
            label_width = Self::LABEL,
            value_width = Self::VALUE,
        );
    }

    /// Average and max, "n/a" where nothing measures it.
    fn span(&mut self, label: &str, span: timing::Span, measured: bool) {
        let (average, max) = match span.average() {
            _ if !measured => ("n/a".to_string(), String::new()),
            Some(average) => (format!("{average:.1}"), format!("{:.1}", span.max())),
            None => ("-".into(), "-".into()),
        };
        self.0 += &format!(
            "\n{label:<label_width$}{average:>value_width$}{max:>value_width$}",
            label_width = Self::LABEL,
            value_width = Self::VALUE,
        );
    }
}

/// Packs the groups into one buffer, grown as needed, each drawn from its
/// own slice: (key, start, end, instance count) per draw.
fn upload_groups(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    groups: &HashMap<GroupKey, Vec<u8>>,
    buffer: &mut wgpu::Buffer,
) -> Vec<(GroupKey, u64, u64, u32)> {
    let mut bytes = Vec::new();
    let mut draws = Vec::new();
    for (key, group) in groups {
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
    if bytes.len() as u64 > buffer.size() {
        // Doubling keeps reallocations rare while a match grows.
        *buffer = create_buffer(
            device,
            queue,
            "model instances",
            &vec![0; (bytes.len() as u64).next_power_of_two() as usize],
            wgpu::BufferUsages::VERTEX,
        );
    }
    if !bytes.is_empty() {
        queue.write_buffer(buffer, 0, &bytes);
    }
    draws
}

/// Whether a sphere is wholly outside the planes.
fn outside(planes: &[[f32; 4]; 6], centre: [f32; 3], radius: f32) -> bool {
    planes.iter().any(|plane| {
        plane[0] * centre[0] + plane[1] * centre[1] + plane[2] * centre[2] + plane[3] < -radius
    })
}

/// The level of detail a sphere is drawn at, `None` when it is out of
/// view.
fn screen_level(
    centre: [f32; 3],
    radius: f32,
    view_proj: &[[f32; 4]; 4],
    planes: &[[f32; 4]; 6],
    pixel_scale: f32,
) -> Option<usize> {
    if outside(planes, centre, radius) {
        return None;
    }
    let depth = view_proj[3][0] * centre[0]
        + view_proj[3][1] * centre[1]
        + view_proj[3][2] * centre[2]
        + view_proj[3][3];
    let diameter = 2.0 * radius * pixel_scale / depth.max(NEAR);
    Some(
        LEVEL_PIXELS
            .iter()
            .position(|pixels| diameter >= *pixels)
            .unwrap_or(LEVEL_PIXELS.len()),
    )
}

fn has_decal(model: &Model) -> bool {
    matches!(model.shape, Shape::Decal(_)) || model.props.iter().any(|(_, prop)| has_decal(prop))
}

/// A prop point's place on its bone or the model's origin.
fn point_matrix(point: &PropPoint) -> Mat4 {
    let [px, py, pz] = point.position;
    let [x, y, z, w] = point.rotation;
    Mat4::from_bone(Vec3::new(px, py, pz), Quat { x, y, z, w })
}

/// `CModelDecal::CalcVertexExtents`: the ground vertices i0, j0, i1, j1
/// around the decal's square, clamped to the map; `None` when empty.
fn decal_extents(tiles: u32, place: &[f32; 8]) -> Option<[usize; 4]> {
    let [cos, sin, x, z, offset_x, offset_z, size_x, size_z] = *place;
    let mut min = [f32::MAX; 2];
    let mut max = [f32::MIN; 2];
    for (sx, sz) in [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)] {
        let cx = offset_x + sx * size_x / 2.0;
        let cz = offset_z + sz * size_z / 2.0;
        // CMatrix3D::SetYRotation, then the translation.
        let corner = [cos * cx + sin * cz + x, -sin * cx + cos * cz + z];
        for axis in 0..2 {
            min[axis] = min[axis].min(corner[axis]);
            max[axis] = max[axis].max(corner[axis]);
        }
    }
    let clamp = |vertex: f32| (vertex as i64).clamp(0, i64::from(tiles)) as usize;
    let i0 = clamp((min[0] / TILE_SIZE).floor());
    let j0 = clamp((min[1] / TILE_SIZE).floor());
    let i1 = clamp((max[0] / TILE_SIZE).ceil());
    let j1 = clamp((max[1] / TILE_SIZE).ceil());
    (i1 > i0 && j1 > j0).then_some([i0, j0, i1, j1])
}

fn now() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
}

/// What every model of one entity shares this frame.
/// Who a projectile or impact actor belongs to, and its animation time
/// in ms.
struct ShotPlace {
    owner: i32,
    /// What its particle props are kept by.
    emitter: Owner,
    time: f32,
    progress: f32,
}

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
    let (time, duration) = animation_clock(entity, chosen, data);
    if dot.once {
        // "Very nearly the end of the last frame", as the engine holds it.
        (time.min(duration - 1.0).max(0.0), false)
    } else {
        (time.rem_euclid(duration.max(f32::EPSILON)), true)
    }
}

/// The animation's time in ms as `animation_time` works it out, before it
/// wraps or holds at the end, and its duration.
fn animation_clock(
    entity: &EntityState,
    chosen: &ModelAnimation,
    data: &AnimationData,
) -> (f32, f32) {
    let dot = &entity.dot;
    let duration = data.frames as f32 * data.frame_ms;
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
        let played = dot.phase + since(dot.speed_turn) * dot.speed as f32 / 256.0;
        played * chosen.speed
    };
    (time, duration)
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

/// The second half of `MATERIAL_SIZE`.
fn material_flags(normal_map: bool, wind: Option<[f32; 2]>) -> [f32; 4] {
    let [x, z] = wind.unwrap_or_default();
    [
        if normal_map { 1.0 } else { 0.0 },
        if wind.is_some() { 1.0 } else { 0.0 },
        x,
        z,
    ]
}

fn material_bind_group(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    views: [&wgpu::TextureView; 4],
    sampler: &wgpu::Sampler,
    effect: [f32; 4],
    flags: [f32; 4],
) -> wgpu::BindGroup {
    let mut bytes = Vec::with_capacity(MATERIAL_SIZE);
    for value in effect.iter().chain(&flags) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let buffer = create_buffer(
        device,
        queue,
        "model material",
        &bytes,
        wgpu::BufferUsages::UNIFORM,
    );
    let [base, normal, specular, ao] = views;
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model material"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(base),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(normal),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(specular),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(ao),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: buffer.as_entire_binding(),
            },
        ],
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

/// The texture tiers a device can be given: without BC the levels are
/// decoded to RGBA8, four to eight times the memory, so the largest tier
/// is left out.
fn max_texture_tiers(bc_supported: bool) -> usize {
    if bc_supported {
        TEXTURE_TIER_SIDES.len()
    } else {
        TEXTURE_TIER_SIDES.len() - 1
    }
}

/// Which side of the water a pass keeps, as the engine's water cameras
/// clip; field.wgsl's and model.wgsl's `WATER_CLIP`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WaterClip {
    None = 0,
    /// The refraction: what is under the water.
    Below = 1,
    /// The reflection, mirrored: what is over it.
    Above = 2,
}

/// What the ground and model pipelines are made from, kept to make the
/// water passes' when they are first drawn.
struct SceneSources {
    field_shader: wgpu::ShaderModule,
    model_shader: wgpu::ShaderModule,
    terrain_layout: wgpu::PipelineLayout,
    blend_layout: wgpu::PipelineLayout,
    model_layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
}

/// The ground's base and blend passes and the models, for one view or
/// water pass, by the material maps setting and then the triplanar one.
struct ScenePipelines {
    terrain: [[wgpu::RenderPipeline; 2]; 2],
    blend: [[wgpu::RenderPipeline; 2]; 2],
    model: [wgpu::RenderPipeline; 2],
}

impl ScenePipelines {
    fn new(device: &wgpu::Device, sources: &SceneSources, clip: WaterClip) -> Self {
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        let clip_value = f64::from(clip as u8);
        let depth_stencil = |write: bool| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        };
        // The engine draws models double-sided where the material asks,
        // and without culling every model looks right; so does every
        // pass here, the mirrored reflection too, whose mirror turns the
        // winding round.
        let primitive = wgpu::PrimitiveState::default();
        let terrain_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: blend::VERTEX_SIZE as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &TERRAIN_ATTRIBUTES,
        })];
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
        let ground = |blend: bool, maps: bool, triplanar: bool| {
            let constants = [
                ("MAPS", flag(maps)),
                ("TRIPLANAR", flag(triplanar)),
                ("WATER_CLIP", clip_value),
            ];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if blend { "terrain blend" } else { "terrain" }),
                layout: Some(if blend {
                    &sources.blend_layout
                } else {
                    &sources.terrain_layout
                }),
                vertex: wgpu::VertexState {
                    module: &sources.field_shader,
                    entry_point: Some("vs_terrain"),
                    compilation_options: Default::default(),
                    buffers: &terrain_buffers,
                },
                primitive,
                // The blend pass goes over the base pass's surface, at the
                // very same depth.
                depth_stencil: Some(if blend {
                    wgpu::DepthStencilState {
                        depth_compare: Some(wgpu::CompareFunction::LessEqual),
                        ..depth_stencil(false)
                    }
                } else {
                    depth_stencil(true)
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &sources.field_shader,
                    entry_point: Some(if blend { "fs_blend" } else { "fs_terrain" }),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &constants,
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format: sources.format,
                        blend: blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let model = |maps: bool| {
            let constants = [("MAPS", flag(maps)), ("WATER_CLIP", clip_value)];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("model"),
                layout: Some(&sources.model_layout),
                vertex: wgpu::VertexState {
                    module: &sources.model_shader,
                    entry_point: Some("vs_model"),
                    compilation_options: Default::default(),
                    buffers: &model_buffers,
                },
                primitive,
                depth_stencil: Some(depth_stencil(true)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &sources.model_shader,
                    entry_point: Some("fs_model"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &constants,
                        ..Default::default()
                    },
                    targets: &[Some(sources.format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        ScenePipelines {
            terrain: [false, true]
                .map(|maps| [false, true].map(|triplanar| ground(false, maps, triplanar))),
            blend: [false, true]
                .map(|maps| [false, true].map(|triplanar| ground(true, maps, triplanar))),
            model: [false, true].map(model),
        }
    }
}

/// BC levels from `first` on go up as they are when the device takes
/// them; otherwise, and for RGBA8, as RGBA8.
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &Texture,
    first: usize,
    bc_supported: bool,
) -> wgpu::TextureView {
    let first = first.min(texture.levels.len() - 1);
    let (top_width, top_height) = level_extent(texture.width, texture.height, first as u32);
    let gpu_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("skin"),
        size: wgpu::Extent3d {
            width: top_width,
            height: top_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: (texture.levels.len() - first) as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: gpu_format(texture, bc_supported),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_levels(queue, &gpu_texture, texture, first, 0, bc_supported);
    gpu_texture.create_view(&Default::default())
}

/// The format `texture` goes up as: its BC blocks when the device takes
/// them, otherwise RGBA8.
pub fn gpu_format(texture: &Texture, bc_supported: bool) -> wgpu::TextureFormat {
    match texture.format.bc().filter(|_| bc_supported) {
        Some(bc::Format::Bc1) => wgpu::TextureFormat::Bc1RgbaUnorm,
        Some(bc::Format::Bc2) => wgpu::TextureFormat::Bc2RgbaUnorm,
        Some(bc::Format::Bc3) => wgpu::TextureFormat::Bc3RgbaUnorm,
        None => wgpu::TextureFormat::Rgba8Unorm,
    }
}

/// Writes `texture`'s levels from `first` on into `layer` of
/// `gpu_texture`, whose format is `gpu_format`'s.
pub fn write_levels(
    queue: &wgpu::Queue,
    gpu_texture: &wgpu::Texture,
    texture: &Texture,
    first: usize,
    layer: u32,
    bc_supported: bool,
) {
    let first = first.min(texture.levels.len() - 1);
    let (top_width, top_height) = level_extent(texture.width, texture.height, first as u32);
    let bc_format = texture.format.bc().filter(|_| bc_supported);
    for (level, data) in texture.levels[first..].iter().enumerate() {
        let (width, height) = level_extent(top_width, top_height, level as u32);
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
                texture: gpu_texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
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
}

/// `CCmpDecay::Interpolate`: how far a corpse or rubble has sunk `time`
/// seconds after it appeared, and its x and z rotations when it sinks like
/// a ship; `None` once it is under, where the engine destroys it.
/// `height` is its model's, `above` how far it stands over the ground,
/// `at` its x and z. The engine sinks at random frames, which only makes
/// its steps uneven; this sinks every frame.
fn decay(
    sinking: &Sinking,
    time: f32,
    height: f32,
    above: f32,
    at: [f32; 2],
) -> Option<(f32, Option<[f32; 2]>)> {
    if time < sinking.delay {
        return Some((0.0, None));
    }
    let mut total = height.max(1.0) + above.max(0.0);
    let t = time - sinking.delay;
    let (depth, tilt) = if sinking.ship {
        // Further, as it turns over.
        total += 10.0;
        let tilt_time = t.min(5.0);
        let tilt_sink = tilt_time * tilt_time / 5.0;
        // The engine's stand-in for randomness, from the position.
        let angle_x = ((at[0].round() as i32) % 30 - 15) as f32 / 15.0;
        let angle_z = ((at[1].round() as i32) % 30) as f32 / 40.0;
        let depth = sinking.rate * ((t - 1.0).exp() - 0.548_811_6)
            + (sinking.acceleration * (t - 4.0).exp() - 0.018_315_64);
        (
            depth.max(0.0),
            Some([angle_x * tilt_sink / 5.0, angle_z * tilt_time / 3.0]),
        )
    } else {
        (sinking.rate * t + sinking.acceleration * t * t, None)
    };
    (depth <= total).then_some((depth, tilt))
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

    /// The engine's camera.reset, `ResetCameraAngleZoom`: the default zoom,
    /// tilt and turn over the same ground point.
    fn reset_angle_zoom(&mut self) {
        self.distance_ahead = START_DISTANCE - self.distance;
        self.pitch_ahead = START_PITCH - self.pitch;
        // The short way round.
        let turn = std::f32::consts::TAU;
        self.yaw_ahead = (START_YAW - self.yaw + turn / 2.0).rem_euclid(turn) - turn / 2.0;
        self.birds_eye = false;
    }

    /// The engine's camera.togglebirdseyeview, `ToggleBirdsEyeView`; back
    /// at the default tilt, not the one left.
    fn toggle_birds_eye(&mut self) {
        self.birds_eye = !self.birds_eye;
        let goal = if self.birds_eye {
            BIRDS_EYE_PITCH
        } else {
            START_PITCH
        };
        self.pitch_ahead = goal - self.pitch;
    }

    /// Moves and turns the camera by the keys held since the last frame.
    fn step(&mut self, now: f64) {
        let seconds = (((now - self.last_step) / 1000.0) as f32).clamp(0.0, KEY_STEP_MAX_S);
        self.last_step = now;
        let ease = |ahead: &mut f32, smoothness: f32| {
            let share = if ahead.abs() < 1e-4 {
                1.0
            } else {
                1.0 - smoothness.powf(10.0 * seconds)
            };
            let delta = *ahead * share;
            *ahead -= delta;
            delta
        };
        self.yaw += ease(&mut self.yaw_ahead, YAW_SMOOTHNESS);
        self.pitch += ease(&mut self.pitch_ahead, PITCH_SMOOTHNESS);
        self.distance += ease(&mut self.distance_ahead, DISTANCE_SMOOTHNESS);
        if self.keys == 0 {
            return;
        }
        let axis = |plus: u8, minus: u8| {
            f32::from(u8::from(self.keys & plus != 0)) - f32::from(u8::from(self.keys & minus != 0))
        };
        let (ahead, side) = (axis(KEY_FORWARD, KEY_BACK), axis(KEY_RIGHT, KEY_LEFT));
        // The camera orbits clockwise seen from above, so the ground turns
        // anticlockwise on screen, as the yaw going up does.
        self.yaw += axis(KEY_TURN_CW, KEY_TURN_CCW) * KEY_ROTATE_SPEED * seconds;
        // Looking up lowers the eye towards the horizon.
        if !self.birds_eye {
            self.pitch = (self.pitch - axis(KEY_TILT_UP, KEY_TILT_DOWN) * KEY_TILT_SPEED * seconds)
                .clamp(MIN_PITCH, MAX_PITCH);
        }
        let metres = self.distance * KEY_PAN_SPEED * seconds;
        let (sin, cos) = self.yaw.sin_cos();
        // As `pan`: forward over the ground is (-sin, -cos), right (-cos, sin).
        self.target[0] += (-ahead * sin - side * cos) * metres;
        self.target[1] += (-ahead * cos + side * sin) * metres;
    }

    /// A finger or pen pans, as does a mouse with the middle button held
    /// (`buttons`, the event's); two fingers pan by their midpoint and
    /// pinch to zoom. They do not turn: no pinch is free of twist, so
    /// every zoom would turn the view a little.
    fn pointer_moved(&mut self, id: i32, buttons: u16, x: f32, y: f32) {
        let Some(index) = self.pointers.iter().position(|pointer| pointer.id == id) else {
            return;
        };
        let touches: Vec<usize> = (0..self.pointers.len())
            .filter(|&i| !self.pointers[i].mouse)
            .collect();
        let pointer = &self.pointers[index];
        if !pointer.mouse && touches.len() >= 2 {
            let other = &self.pointers[if touches[0] == index {
                touches[1]
            } else {
                touches[0]
            }];
            let (ox, oy) = (other.x, other.y);
            let before = &self.pointers[index];
            let span = |ax: f32, ay: f32| (ax - ox).hypot(ay - oy).max(1.0);
            let zoom = span(before.x, before.y) / span(x, y);
            // The midpoint moves by half of this pointer's move.
            let (dx, dy) = ((x - before.x) / 2.0, (y - before.y) / 2.0);
            self.zoom(zoom);
            self.pan(dx, dy);
        } else {
            let (dx, dy) = (x - pointer.x, y - pointer.y);
            if !pointer.mouse || buttons & BUTTONS_MIDDLE != 0 {
                self.pan(dx, dy);
            }
        }
        let pointer = &mut self.pointers[index];
        pointer.x = x;
        pointer.y = y;
    }
}

/// The overlay of each drawn entity it shows for, coloured as
/// `CCmpSelectable::UpdateColor` and `SetSelectionHighlight`.
fn overlay_marks<'a>(
    pickables: &[Pickable],
    entities: &'a Entities,
    selection: &Selection,
    palette: &[u32],
    ranges: [bool; 3],
) -> Vec<Mark<'a>> {
    let mut marks = Vec::new();
    // White without an owner.
    let owner_rgb = |owner: i32| {
        usize::try_from(owner)
            .ok()
            .and_then(|owner| palette.get(owner))
            .map_or([1.0; 3], |color| {
                let argb = color.to_be_bytes();
                [argb[1], argb[2], argb[3]].map(|value| f32::from(value) / 255.0)
            })
    };
    let packed = |rgb: [f32; 3], alpha: f32| {
        let [r, g, b, a] = [rgb[0], rgb[1], rgb[2], alpha]
            .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u32);
        a << 24 | r << 16 | g << 8 | b
    };
    for pickable in pickables {
        let Some(dot) = entity_dot(entities, pickable.id) else {
            continue;
        };
        // The circles of `RangeOverlayRenderer`, in the owner's colour and
        // not faded.
        if dot.ranges != 0
            && ranges.contains(&true)
            && selection.shows_ranges(pickable.id)
            && let Some(set) = (dot.ranges as usize)
                .checked_sub(1)
                .and_then(|index| entities.range_sets.get(index))
        {
            for circle in set {
                let Some(shown) = ranges.get(usize::from(circle.kind)) else {
                    continue;
                };
                let Some(texture) = (circle.texture as usize)
                    .checked_sub(1)
                    .and_then(|index| entities.strings.get(index))
                else {
                    continue;
                };
                if !shown {
                    continue;
                }
                marks.push(Mark {
                    texture,
                    outline: Some(circle.thickness),
                    shape: OverlayShape::Circle {
                        radius: circle.radius,
                    },
                    centre: [pickable.position[0], pickable.position[2]],
                    facing: [1.0, 0.0],
                    color: packed(owner_rgb(dot.owner), 1.0),
                });
            }
        }
        let Some(details) = entities
            .template_details
            .get(dot.template as usize)
            .and_then(Option::as_ref)
        else {
            continue;
        };
        let always_visible = details.flags & pick::FLAG_ALWAYS_VISIBLE != 0;
        let overlay = selection.overlay(pickable.id, always_visible);
        if overlay.alpha <= 0.0 {
            continue;
        }
        let Some(shape) = overlay_shape(details) else {
            continue;
        };
        let Some(texture) = (details.overlay_texture as usize)
            .checked_sub(1)
            .and_then(|index| entities.strings.get(index))
        else {
            continue;
        };
        let mut rgb = owner_rgb(dot.owner);
        if always_visible && !overlay.selected {
            let max = rgb[0].max(rgb[1]).max(rgb[2]);
            rgb = rgb.map(|value| value + (max - value) * RGB_DESATURATION);
        }
        marks.push(Mark {
            texture,
            outline: (details.flags & pick::FLAG_OUTLINE != 0).then_some(details.line_thickness),
            shape,
            centre: [pickable.position[0], pickable.position[2]],
            facing: pickable.facing,
            color: packed(rgb, overlay.alpha),
        });
    }
    marks
}

fn entity_dot(entities: &Entities, id: u32) -> Option<&Dot> {
    let dots = &entities.dots;
    let index = dots.binary_search_by_key(&id, |(id, _)| *id).ok()?;
    Some(&dots[index].1)
}

/// `CCmpSelectable`'s overlay shape: its own, else the footprint's, else
/// none, as the engine then draws nothing.
fn overlay_shape(details: &TemplateDetails) -> Option<OverlayShape> {
    let flags = details.flags;
    if flags & pick::FLAG_OVERLAY_SQUARE != 0 {
        Some(OverlayShape::Square {
            width: details.overlay_width,
            depth: details.overlay_depth,
        })
    } else if flags & pick::FLAG_OVERLAY_CIRCLE != 0 {
        Some(OverlayShape::Circle {
            radius: details.overlay_width,
        })
    } else if flags & pick::FLAG_FOOTPRINT == 0 {
        None
    } else if flags & pick::FLAG_SQUARE_FOOTPRINT != 0 {
        Some(OverlayShape::Square {
            width: details.footprint_width,
            depth: details.footprint_depth,
        })
    } else {
        Some(OverlayShape::Circle {
            radius: details.footprint_width,
        })
    }
}

/// What the selection needs of entity `id`, `None` when it does not exist.
fn selectee(entities: &Entities, id: u32) -> Option<Selectee> {
    let dot = entity_dot(entities, id)?;
    let flags = entities
        .template_details
        .get(dot.template as usize)
        .and_then(Option::as_ref)
        .map_or(0, |details| details.flags);
    Some(Selectee {
        owner: dot.owner,
        always_visible: flags & pick::FLAG_ALWAYS_VISIBLE != 0,
    })
}

/// Whether a press moved far enough to be a drag.
fn dragged(from: [f32; 2], to: [f32; 2]) -> bool {
    (to[0] - from[0]).hypot(to[1] - from[1]) >= DRAG_DELTA
}

/// A client point 0..1 across the canvas, whose client offset is `offset`.
fn on_canvas(canvas: &HtmlCanvasElement, at: [f32; 2], offset: [f32; 2]) -> [f32; 2] {
    [
        (at[0] - offset[0]) / canvas.client_width().max(1) as f32,
        (at[1] - offset[1]) / canvas.client_height().max(1) as f32,
    ]
}

/// `pointerId`, read through the base `MouseEvent` a `PointerEvent` is.
pub fn pointer_id(event: &MouseEvent) -> i32 {
    js_sys::Reflect::get(event, &"pointerId".into())
        .ok()
        .and_then(|id| id.as_f64())
        .map_or(0, |id| id as i32)
}

/// Mouse, touch and pen through pointer events, with the engine's mouse
/// bindings: middle drag to pan, the wheel to zoom, Shift+wheel and X1/X2
/// to turn; the left and right buttons are left to selection and orders.
/// Fingers and pens drag to pan and pinch to zoom. The handlers live as
/// long as the page.
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
            let mouse = js_sys::Reflect::get(&event, &"pointerType".into())
                .ok()
                .and_then(|kind| kind.as_string())
                .is_none_or(|kind| kind == "mouse");
            camera.pointers.push(Pointer {
                id,
                mouse,
                x: event.client_x() as f32,
                y: event.client_y() as f32,
            });
            if mouse && event.button() == 0 {
                let at = [event.client_x() as f32, event.client_y() as f32];
                let offset = [
                    at[0] - event.offset_x() as f32,
                    at[1] - event.offset_y() as f32,
                ];
                camera.press = Some((id, at, offset));
            }
        }),
    );

    // A drag ends on pointerup, on the window as it may leave the canvas;
    // a click ends on click, which counts the clicks.
    let target = Rc::clone(camera);
    let up_canvas = canvas.clone();
    listen(
        &window,
        "pointerup",
        Closure::new(move |event: MouseEvent| {
            let camera = &mut *target.borrow_mut();
            let Some((id, from, offset)) = camera.press else {
                return;
            };
            let to = [event.client_x() as f32, event.client_y() as f32];
            if id != pointer_id(&event) || !dragged(from, to) {
                return;
            }
            camera.press = None;
            camera.band = None;
            camera.picks.push(PickInput::Rect {
                from: on_canvas(&up_canvas, from, offset),
                to: on_canvas(&up_canvas, to, offset),
                add: event.shift_key(),
                remove: event.ctrl_key(),
            });
        }),
    );
    let target = Rc::clone(camera);
    let click_canvas = canvas.clone();
    listen(
        canvas,
        "click",
        Closure::new(move |event: MouseEvent| {
            let camera = &mut *target.borrow_mut();
            let Some((_, from, offset)) = camera.press.take() else {
                return;
            };
            let at = [event.client_x() as f32, event.client_y() as f32];
            if dragged(from, at) {
                return;
            }
            let count = js_sys::Reflect::get(&event, &"detail".into())
                .ok()
                .and_then(|detail| detail.as_f64())
                .map_or(1, |detail| detail as u32);
            camera.picks.push(PickInput::Click {
                at: on_canvas(&click_canvas, at, offset),
                count,
                add: event.shift_key(),
                remove: event.ctrl_key(),
            });
        }),
    );

    // The right button gives orders rather than open a menu.
    listen(
        canvas,
        "contextmenu",
        Closure::new(|event: MouseEvent| event.prevent_default()),
    );

    // On mousedown, which fires for every button, where pointerdown fires
    // only for the first one held. The middle button would start the
    // browser's autoscroll, X1/X2 would go back and forward (on release).
    let target = Rc::clone(camera);
    listen(
        canvas,
        "mousedown",
        Closure::new(move |event: MouseEvent| {
            // The right button cancels a bandbox, as the engine's.
            if event.button() == 2 {
                let camera = &mut *target.borrow_mut();
                if camera.band.take().is_some() {
                    camera.press = None;
                }
                return;
            }
            let turn = match event.button() {
                1 => 0.0,
                3 => WHEEL_ROTATE_STEP,
                4 => -WHEEL_ROTATE_STEP,
                _ => return,
            };
            event.prevent_default();
            target.borrow_mut().yaw_ahead += turn;
        }),
    );
    for name in ["mouseup", "auxclick"] {
        listen(
            canvas,
            name,
            Closure::new(|event: MouseEvent| {
                if matches!(event.button(), 1 | 3 | 4) {
                    event.prevent_default();
                }
            }),
        );
    }

    // On the window, so a drag that leaves the canvas still ends.
    let target = Rc::clone(camera);
    let move_canvas = canvas.clone();
    listen(
        &window,
        "pointermove",
        Closure::new(move |event: MouseEvent| {
            let camera = &mut *target.borrow_mut();
            let id = pointer_id(&event);
            let at = [event.client_x() as f32, event.client_y() as f32];
            camera.pointer_moved(id, event.buttons(), at[0], at[1]);
            if let Some((pressed, from, offset)) = camera.press
                && pressed == id
                && (camera.band.is_some() || dragged(from, at))
            {
                camera.band = Some(Band {
                    from: on_canvas(&move_canvas, from, offset),
                    to: on_canvas(&move_canvas, at, offset),
                    client_from: from,
                    client_to: at,
                });
            }
        }),
    );
    // Hover is the mouse's only; a finger is never over anything.
    let target = Rc::clone(camera);
    let hover_canvas = canvas.clone();
    listen(
        canvas,
        "pointermove",
        Closure::new(move |event: MouseEvent| {
            let mouse = js_sys::Reflect::get(&event, &"pointerType".into())
                .ok()
                .and_then(|kind| kind.as_string())
                .is_none_or(|kind| kind == "mouse");
            target.borrow_mut().hover = mouse.then(|| {
                [
                    event.offset_x() as f32 / hover_canvas.client_width().max(1) as f32,
                    event.offset_y() as f32 / hover_canvas.client_height().max(1) as f32,
                ]
            });
        }),
    );
    let target = Rc::clone(camera);
    listen(
        canvas,
        "pointerleave",
        Closure::new(move |_: MouseEvent| target.borrow_mut().hover = None),
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
        // Turning clockwise on the wheel going up, as Q. Some browsers turn
        // Shift+wheel into a sideways scroll.
        if event.shift_key() {
            let delta = if event.delta_y() != 0.0 {
                event.delta_y()
            } else {
                event.delta_x()
            };
            if delta != 0.0 {
                let step = -delta.signum() as f32;
                target.borrow_mut().yaw_ahead += step * WHEEL_ROTATE_STEP;
            }
            return;
        }
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
            let code = text(&event, "code");
            let camera = &mut *target.borrow_mut();
            // Ctrl may be let go before the arrow, or after.
            if !down {
                camera.keys &= !(key_bit(&code, false) | key_bit(&code, true));
                return;
            }
            let tag = event
                .target()
                .map(|element| text(&element, "tagName"))
                .unwrap_or_default();
            let control = matches!(tag.as_str(), "INPUT" | "SELECT" | "TEXTAREA");
            // The engine's session.toggleattackrange, toggleaurasrange and
            // togglehealrange.
            if !control && flag("altKey") && !flag("ctrlKey") && !flag("metaKey") {
                let kind = match code.as_str() {
                    "KeyC" => Some(0),
                    "KeyV" => Some(1),
                    "KeyB" => Some(2),
                    _ => None,
                };
                if let Some(kind) = kind {
                    event.prevent_default();
                    if !flag("repeat") {
                        camera.ranges[kind] = !camera.ranges[kind];
                    }
                    return;
                }
            }
            if control || flag("metaKey") || flag("altKey") {
                return;
            }
            let ctrl = flag("ctrlKey");
            let shift = flag("shiftKey");
            // Once, as the engine's hotkey press, not on each repeat.
            let press = !flag("repeat");
            match code.as_str() {
                "KeyR" if !ctrl && !shift => {
                    event.prevent_default();
                    if press {
                        camera.reset_angle_zoom();
                    }
                    return;
                }
                // The engine's selection.cancel.
                "Escape" if !ctrl && !shift => {
                    if press {
                        camera.picks.push(PickInput::Clear);
                    }
                    return;
                }
                // Shift+Tab still steps back through the page's controls
                // once one has the focus.
                "Tab" if !ctrl && shift && tag != "BUTTON" => {
                    event.prevent_default();
                    if press {
                        camera.toggle_birds_eye();
                    }
                    return;
                }
                _ => {}
            }
            let bit = key_bit(&code, ctrl);
            if bit == 0 {
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
