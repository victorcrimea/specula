mod assets;
mod audio;
mod barter;
mod bench;
mod blend;
mod command_row;
mod detail;
mod diplomacy;
mod diplomacy_window;
mod far;
mod field;
mod game_menu;
mod gpu;
mod icons;
mod idle;
mod impostor;
mod lang;
mod left;
mod los;
mod march;
mod message_box;
mod minimap;
mod opfs;
mod orders;
mod overlay;
mod panel;
mod particles;
mod pause;
mod pick;
mod placement;
mod pose;
mod postproc;
mod predict;
mod projectile;
mod render;
mod research;
mod selection;
mod shadow;
mod silhouette;
mod sky;
mod stored;
mod stream;
mod summary;
mod terrain;
mod territory;
mod timing;
mod tips;
mod tooltip;
mod top;
mod trade;
mod trade_window;
mod unit;
mod uplink;
mod usage;
mod variation;
mod warmup;
mod water;
mod worker;

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use specula_assets::command::Command;
use specula_assets::files::MusicKind;
use specula_assets::files::TEXTURE_TIER_SIDES;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
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
use web_sys::MouseEvent;
use web_sys::Response;
use web_sys::Worker;
use web_sys::WorkerOptions;
use web_sys::WorkerType;

use crate::assets::Progress;
use crate::assets::Store;
use crate::audio::Audio;
use crate::audio::DEFAULT_VOLUME;
use crate::audio::Settings;
use crate::bench::Bench;
use crate::bench::BenchInfo;
use crate::bench::CameraPose;
use crate::detail::Details;
use crate::diplomacy::Perspective;
use crate::diplomacy_window::DiplomacyWindow;
use crate::field::Field;
use crate::game_menu::GameMenu;
use crate::icons::IconAtlas;
use crate::message_box::MessageBox;
use crate::message_box::Pending;
use crate::minimap::Minimap;
use crate::orders::Preselect;
use crate::panel::DetailsPanel;
use crate::panel::Request;
use crate::pause::PauseOverlay;
use crate::placement::Placement;
use crate::postproc::Upscale;
use crate::predict::Predictions;
use crate::render::Renderer;
use crate::research::ResearchProgress;
use crate::stream::Ground;
use crate::stream::PlayerState;
use crate::stream::Scene;
use crate::stream::Sound;
use crate::stream::SoundTarget;
use crate::summary::Source;
use crate::summary::SummaryWindow;
use crate::terrain::TerrainImage;
use crate::top::TopPanel;
use crate::trade_window::TradeWindow;
use crate::uplink::Uplink;
use crate::warmup::Warmup;
use crate::worker::COMMAND_CURRENT_MATCH;
use crate::worker::COMMAND_DETAIL;
use crate::worker::COMMAND_LIVE;
use crate::worker::COMMAND_PAUSE;
use crate::worker::COMMAND_PLAY;
use crate::worker::COMMAND_SCRUB;
use crate::worker::COMMAND_SEEK;
use crate::worker::COMMAND_SPEED;
use crate::worker::POST_DETAIL;
use crate::worker::POST_SCENE;
use crate::worker::POST_STARTS;
use crate::worker::POST_STATUS;
use crate::worker::POST_TERRAIN;
use crate::worker::STATUS_ARCHIVE;
use crate::worker::STATUS_ENDED;
use crate::worker::STATUS_LIVE;
use crate::worker::STATUS_NEXT_READY;
use crate::worker::STATUS_PAUSED;

type FrameCallback = Closure<dyn FnMut(f64)>;

/// A multiplayer turn is 200 ms of simulation time (DEFAULT_TURN_LENGTH);
/// the real interval depends on game speed and is measured.
/// The page is a seat's own view on `/play`, whose files come from the
/// seat's routes, rather than the observers'.
static SEATED: AtomicBool = AtomicBool::new(false);

pub fn seated() -> bool {
    SEATED.load(Ordering::Relaxed)
}

const TURN_MS: f64 = 200.0;
const TURN_MS_MIN: f64 = 50.0;
const TURN_MS_MAX: f64 = 1000.0;
/// How often a seat's growing terrain rebuilds the view: the terrain mesh
/// and blends are made for the whole map at once.
const TERRAIN_REBUILD_MS: f64 = 1000.0;
/// Low enough that network jitter does not make the pace wobble.
const TURN_SMOOTHING: f64 = 0.1;
/// Playback speeds in game time; the match's own speed is added if missing.
const SPEEDS: [f32; 5] = [0.5, 1.0, 2.0, 4.0, 8.0];
const POSITION_UNITS_PER_METRE: f32 = 16.0;
/// localStorage key of the sound settings: "on,master,effects,music".
const SOUND_SETTINGS_KEY: &str = "specula.sound";
/// localStorage key of the shadow map size, 0 for no shadows.
const SHADOW_SETTINGS_KEY: &str = "specula.shadows";
/// localStorage key of the shadow cutoff distance in metres.
const SHADOW_CUTOFF_SETTINGS_KEY: &str = "specula.shadowscutoff";
/// localStorage key of the texture quality: 1 low, 2 medium, 3 high.
const TEXTURE_SETTINGS_KEY: &str = "specula.textures";
const LANGUAGE_SETTINGS_KEY: &str = lang::SETTING_KEY;
/// localStorage key of the material maps setting: 0 off, 1 on.
const MAP_SETTINGS_KEY: &str = "specula.maps";
/// localStorage key of the triplanar setting: 0 off, 1 on.
const TRIPLANAR_SETTINGS_KEY: &str = "specula.triplanar";
/// localStorage key of the wind setting: 0 off, 1 on.
const WIND_SETTINGS_KEY: &str = "specula.wind";
/// localStorage keys of the post-processing, bloom, depth of field and fog
/// settings: 0 off, 1 on.
const POSTPROC_SETTINGS_KEY: &str = "specula.postproc";
const BLOOM_SETTINGS_KEY: &str = "specula.bloom";
const DOF_SETTINGS_KEY: &str = "specula.dof";
/// localStorage keys of the antialiasing and sharpening settings, 0 off
/// and 1 FXAA or CAS, and of the sharpness factor, from 0 to 1.
const ANTIALIASING_SETTINGS_KEY: &str = "specula.antialiasing";
const SHARPENING_SETTINGS_KEY: &str = "specula.sharpening";
const SHARPNESS_SETTINGS_KEY: &str = "specula.sharpness";
/// localStorage keys of the resolution scale, a factor of the canvas's
/// size, and of the upscale technique, "fsr", "bilinear" or "pixelated".
const RENDER_SCALE_SETTINGS_KEY: &str = "specula.renderscale";
const UPSCALE_SETTINGS_KEY: &str = "specula.upscale";
const FOG_SETTINGS_KEY: &str = "specula.fog";
const WATER_SETTINGS_KEY: &str = "specula.water";
const SKY_SETTINGS_KEY: &str = "specula.sky";
const WORLD_SUPPORT_SETTINGS_KEY: &str = "specula.worldsupport";
const PARTICLES_SETTINGS_KEY: &str = "specula.particles";
const SILHOUETTES_SETTINGS_KEY: &str = "specula.silhouettes";
const PROJECTILES_SETTINGS_KEY: &str = "specula.projectiles";
/// localStorage key of the detailed tooltips setting, the engine's
/// showdetailedtooltips: 0 off, 1 on.
const DETAILED_TOOLTIPS_SETTINGS_KEY: &str = "specula.detailedtooltips";
/// localStorage key of the status bars setting, the engine's
/// `g_ShowAllStatusBars`: 0 off, 1 on.
const STATUS_BARS_SETTINGS_KEY: &str = "specula.statusbars";
/// How long the copy button says the link was copied.
const COPIED_MS: i32 = 2000;
/// localStorage keys of the attack, aura and heal range overlays, in that
/// order: 0 off, 1 on.
const RANGES_SETTINGS_KEYS: [&str; 3] = [
    "specula.rangesattack",
    "specula.rangesaura",
    "specula.rangesheal",
];
const REFRACTION_SETTINGS_KEY: &str = "specula.waterrefraction";
const REAL_DEPTH_SETTINGS_KEY: &str = "specula.waterrealdepth";
const REFLECTION_SETTINGS_KEY: &str = "specula.waterreflection";
const FANCY_WATER_SETTINGS_KEY: &str = "specula.waterfancyeffects";
/// localStorage key of when the storage notice was last shown, in ms since
/// the epoch.
const STORAGE_NOTICE_KEY: &str = "specula.storage-notice";
const STORAGE_NOTICE_INTERVAL_MS: f64 = 7.0 * 24.0 * 60.0 * 60.0 * 1000.0;

/// The camera a sound is placed against, as the engine's listener sits at
/// it: the view-projection matrix (row-major), the eye in metres and the
/// viewport in CSS pixels.
pub struct Listener {
    pub view_projection: [[f32; 4]; 4],
    pub eye: [f32; 3],
    pub width: f32,
    pub height: f32,
}

/// A sound group an entity's animation reached this frame.
pub struct AnimationSound {
    /// Index into `Scene::strings`.
    pub group: u32,
    pub owner: i32,
    pub visibility: u32,
    /// In metres.
    pub position: [f32; 3],
}

/// The select sound of an entity a selection just added, or an order's.
pub struct EntitySound {
    /// The sound group, relative to audio/.
    pub group: String,
    pub owner: i32,
    /// In metres.
    pub position: [f32; 3],
}

/// What draws the match: the minimap view of `/map.html` or the 3D view
/// of `/`. The stream, playback and controls are the same.
trait View {
    fn max_size(&self) -> u32;
    fn resize(&mut self, width: u32, height: u32, device_pixel_ratio: f32);
    /// Returns false when no frame could be drawn, so the caller retries.
    fn draw(&mut self, progress: f32) -> bool;
    /// `palette` is the displayed colour of each player id.
    fn set_scene(&mut self, scene: &Scene, palette: &[u32]);
    /// The renames of every message applied since the last `set_scene`,
    /// in order, before it.
    fn set_renames(&mut self, _renames: &[(u32, u32)]) {}
    /// `None` for a new match whose terrain has not arrived yet.
    fn set_terrain(&mut self, image: Option<&TerrainImage>);
    fn set_icon_atlas(&mut self, _atlas: IconAtlas) {}
    /// `/selection.bin`, where it draws selection overlays.
    fn set_selection_atlas(&mut self, _bytes: &[u8]) {}
    /// Whether it moves between turns on its own, and so needs every frame.
    fn animating(&self) -> bool {
        false
    }
    /// Whether to draw projectiles, where it draws any.
    fn set_projectiles(&mut self, _on: bool) {}
    /// Whose on-screen entities show their status bars too, where it draws
    /// any; `None` for only the selected and highlighted ones.
    fn set_all_status_bars(&mut self, _shown: Option<Perspective>) {}
    /// The shown perspective, which picking, the status bars and the
    /// selection follow, where it picks any; `controls` on `/play`, whose
    /// player is not an observer.
    fn set_perspective(&mut self, _perspective: Perspective, _controls: bool) {}
    /// Whether the attack, aura and heal range overlays are drawn, where it
    /// draws any. Its own keys toggle them too, hence the getter.
    fn set_ranges(&mut self, _ranges: [bool; 3]) {}
    fn ranges(&self) -> [bool; 3] {
        [false; 3]
    }
    /// The shadow map's size, 0 for none, where it draws shadows.
    fn set_shadows(&mut self, _size: u32) {}
    /// How far ahead shadows reach, in metres, likewise.
    fn set_shadow_cutoff(&mut self, _metres: f32) {}
    /// The texture quality, as a count of `TEXTURE_TIER_SIDES`, where it
    /// draws textures.
    fn set_texture_tiers(&mut self, _tiers: usize) {}
    /// Whether normal, specular and ambient occlusion maps are fetched and
    /// drawn, where it draws them.
    fn set_material_maps(&mut self, _on: bool) {}
    /// Whether the terrains whose material asks for it are mapped
    /// triplanar, where it draws terrain textures.
    fn set_triplanar(&mut self, _on: bool) {}
    /// Whether the wind sways the models whose material asks for it,
    /// where it draws models.
    fn set_wind(&mut self, _on: bool) {}
    /// Whether the map's post effect runs, where it draws the match.
    fn set_postproc(&mut self, _on: bool) {}
    /// Whether the post effect blooms, likewise.
    fn set_bloom(&mut self, _on: bool) {}
    /// Whether a map's DOF effect blurs by depth, likewise.
    fn set_dof(&mut self, _on: bool) {}
    /// Whether FXAA runs after the map's effect, likewise.
    fn set_antialiasing(&mut self, _on: bool) {}
    /// Whether CAS sharpens after FXAA, likewise.
    fn set_sharpening(&mut self, _on: bool) {}
    /// CAS's strength, from 0 to 1, likewise.
    fn set_sharpness(&mut self, _sharpness: f32) {}
    /// The factor of the canvas's size the scene is drawn at, likewise.
    fn set_render_scale(&mut self, _scale: f32) {}
    /// How a scene drawn below the canvas's size is brought up to it.
    fn set_upscale(&mut self, _upscale: Upscale) {}
    /// Whether the map's fog is drawn, likewise.
    fn set_fog(&mut self, _on: bool) {}
    /// Whether the water is drawn as the engine's fancy water, likewise.
    fn set_water_effects(&mut self, _on: bool) {}
    /// Whether the map's sky is drawn behind everything, likewise.
    fn set_sky(&mut self, _on: bool) {}
    /// Whether the elephants and the turtle under the map are drawn, likewise.
    fn set_world_support(&mut self, _on: bool) {}
    /// Whether the actors' particles are drawn, likewise.
    fn set_particles(&mut self, _on: bool) {}
    /// Whether units behind structures, gaia and the ground are drawn as
    /// silhouettes, likewise.
    fn set_silhouettes(&mut self, _on: bool) {}
    /// Whether the water refracts what is under it, and takes its depth
    /// from that, likewise.
    fn set_water_refraction(&mut self, _on: bool) {}
    fn set_water_real_depth(&mut self, _on: bool) {}
    /// Whether the water mirrors what is over it, likewise.
    fn set_water_reflection(&mut self, _on: bool) {}
    /// Whether the water has shore waves and foam, likewise.
    fn set_fancy_water(&mut self, _on: bool) {}
    /// The highest texture quality the device is given.
    fn max_texture_tiers(&self) -> usize {
        TEXTURE_TIER_SIDES.len()
    }
    /// Downloads to show on the loading bar.
    fn progress(&self) -> Progress {
        Progress::Idle
    }
    /// What the stream has brought for the shown match, for statistics.
    fn set_stream_bytes(&mut self, _bytes: StreamBytes) {}
    /// The ground the camera sees, x and z in metres, for the minimap.
    fn view_outline(&self) -> Option<[[f32; 2]; 4]> {
        None
    }
    /// Looks at the ground point `x`, `z` in metres.
    fn move_to(&mut self, _x: f32, _z: f32) {}
    /// Keeps the camera over entity `id`, as `setCameraFollow`.
    fn follow(&mut self, _id: u32) {}
    /// Selects only `ids`, or with `remove` leaves them out of the
    /// selection: a selection panel group button.
    fn select_group(&mut self, _ids: &[u32], _remove: bool) {}
    /// Selects only `id`, or with `add` adds it: a hero button.
    fn select_entity(&mut self, _id: u32, _add: bool) {}
    /// The asset bake it draws from, which also holds the sounds.
    fn asset_store(&self) -> Option<Rc<RefCell<Store>>> {
        None
    }
    fn listener(&self) -> Option<Listener> {
        None
    }
    /// The animation sounds due since the last call.
    fn take_animation_sounds(&mut self, _progress: f32) -> Vec<AnimationSound> {
        Vec::new()
    }
    /// The entities selected, in the order added.
    fn selected(&self) -> &[u32] {
        &[]
    }
    /// The select sounds of the selections made since the last call, and
    /// the sounds `queue_sound` queued.
    fn take_entity_sounds(&mut self) -> Vec<EntitySound> {
        Vec::new()
    }
    /// Plays entity `id`'s sound group `name` (e.g. `order_walk`), as the
    /// session GUI's `PlaySound` does.
    fn queue_sound(&mut self, _id: u32, _name: &str) {}
    /// Where the right clicks since the last call landed, where it takes
    /// orders.
    fn take_order_targets(&mut self) -> Vec<orders::Target> {
        Vec::new()
    }
    /// Whether the left and right buttons place a structure rather than
    /// select and order.
    fn set_placing(&mut self, _on: bool) {}
    /// Whether a left click gives the preselected action's order rather
    /// than select.
    fn set_preselecting(&mut self, _on: bool) {}
    /// The `orders::HOTKEY_*` held now.
    fn hotkeys(&self) -> u8 {
        0
    }
    /// The placement presses, releases and cancels since the last call.
    fn take_place_inputs(&mut self) -> Vec<placement::Input> {
        Vec::new()
    }
    /// The mouse over the ground while placing.
    fn place_pointer(&self) -> Option<placement::Pointer> {
        None
    }
    /// The entity the mouse is over, if any, while the mouse is over the
    /// view and neither places nor drags a bandbox: the input states the
    /// engine picks a right click's cursor in.
    fn hovered(&self) -> Option<Option<u32>> {
        None
    }
    /// The placement previews to draw.
    fn set_ghosts(&mut self, _ghosts: Vec<placement::Ghost>) {}
    /// Shows a walk order's target marker at x and z in metres.
    fn add_target_marker(&mut self, _x: f32, _z: f32) {}
    /// Where the camera is, for the benchmark to hold it there.
    fn camera(&self) -> Option<CameraPose> {
        None
    }
    fn set_camera(&mut self, _pose: CameraPose) {}
    /// What the benchmark report describes the view with.
    fn bench_info(&self) -> BenchInfo {
        BenchInfo::default()
    }
}

/// Bytes for the shown match, as the worker counts them: live messages,
/// history segments and the terrain, and the detail stream's apart.
#[derive(Clone, Copy, Default)]
struct StreamBytes {
    live: u64,
    segments: u64,
    terrain: u64,
    /// Since the counts began, as of `received_at` on the page's clock; the
    /// worker's clock has another origin.
    live_ms: f64,
    received_at: f64,
    /// The most live bytes in `worker::PEAK_WINDOW_S` seconds.
    peak_window: u64,
    /// The detail stream's live and history bytes, and how long it was
    /// followed as of `received_at`, which goes on while `detail_on`.
    detail_live: u64,
    detail_segments: u64,
    detail_ms: f64,
    detail_on: bool,
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

    fn set_perspective(&mut self, perspective: Perspective, _controls: bool) {
        Renderer::set_perspective(self, perspective);
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
            Progress::Starting => (lang::translate("Loading the model index"), 0.0, true),
            Progress::Bytes { done, total } => (
                lang::format(
                    &lang::translate("Loading models %(done)s / %(total)s MB"),
                    &[
                        ("done", &format!("{:.1}", done as f64 / 1e6)),
                        ("total", &format!("{:.1}", total as f64 / 1e6)),
                    ],
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
    page: Page,
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
    /// `Scene::renames` of the messages applied since the view last took
    /// the scene; each message clears the scene's own.
    renames: Vec<(u32, u32)>,
    dirty: bool,
    /// The terrain shown; 0 for none.
    terrain_id: u64,
    /// The terrain the worker handed over: until a keyframe shows it, then
    /// for a seat's, to add the reveals to.
    terrain_image: Option<(u64, TerrainImage)>,
    /// A seat's terrain has reveals the view has not been rebuilt with,
    /// and when it last was, by the frame clock.
    terrain_revealed: bool,
    terrain_rebuilt_at: f64,
    diplomacy_colors: bool,
    perspective: Perspective,
    /// The status bars setting, which follows the perspective.
    all_status_bars: bool,
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
    /// `/` only.
    live_delay: Option<HtmlElement>,
    /// What `live_delay` shows; empty while hidden.
    live_delay_text: String,
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
    /// The turn the timeline shows, for the copied link.
    playhead: u32,
    /// `/` only.
    copy_link: Option<HtmlButtonElement>,
    /// Puts the button's label back after "Link copied".
    copy_link_timer: Option<i32>,
    stream_bytes: StreamBytes,
    match_over: MatchOver,
    stream_start: StreamStart,
    /// Only the 3D page plays sound.
    sound: Option<SoundControls>,
    /// Only the 3D page has assets to warm up.
    warmup: Option<Warmup>,
    /// The worker was last told that something is selected.
    detail_wanted: bool,
    /// Only the 3D page has the selection details panel.
    details: Option<DetailsPanel>,
    /// What the panel was last filled from changed: a turn, the palette or
    /// the perspective; the selection and the portrait index are compared.
    details_dirty: bool,
    details_selected: Vec<u32>,
    details_index: bool,
    /// Only the 3D page has the top panel.
    top: Option<TopPanel>,
    /// Only the 3D page has the research overlay; it shows on `/play`.
    research: Option<ResearchProgress>,
    /// Only the 3D page has the diplomacy window.
    diplomacy_window: Option<DiplomacyWindow>,
    /// Only the 3D page has the Barter & Trade dialog.
    trade_window: Option<TradeWindow>,
    /// session.massbarter (Shift) is held.
    mass_barter: bool,
    /// Only `/play` asks before deleting.
    message_box: Option<MessageBox>,
    /// On `/play`: Resign and Exit.
    game_menu: Option<GameMenu>,
    /// Shown whenever a player has the game paused, on `/` and `/play`.
    pause_overlay: Option<PauseOverlay>,
    /// On `/play`: the seat was reported resigned, beaten or victorious.
    finish_noticed: bool,
    /// Only the 3D page has the match summary.
    summary: Option<Rc<RefCell<SummaryWindow>>>,
    /// On `/play`: the visitor's seat while the game is theirs, running or
    /// not, whose own copy of the summary the page reads.
    owner_seat: Option<usize>,
    /// The outcome of the result offer that View summary was pressed on,
    /// which closing the summary returns to.
    offer_return: Option<bool>,
    /// The toggles `/play` ordered that the stream has not reported yet.
    predictions: Predictions,
    /// A turn, the palette or the perspective changed since it was filled;
    /// the portrait index is compared.
    top_dirty: bool,
    top_index: bool,
    /// Only the 3D page draws range overlays.
    ranges: Option<RangeControls>,
    /// Only the 3D page has panels with tooltips.
    tooltips: Option<tooltip::Tooltips>,
    /// The JSON the tooltips read, fetched as they ask for it.
    data_files: tips::DataFiles,
    detailed_tooltips: bool,
    /// On `/play`: where the player's orders go, and who they command.
    uplink: Option<Uplink>,
    /// The seat the perspective was last moved to.
    seat_shown: Option<i32>,
    /// The view's `data-tooltip`, the hovered action's.
    action_tooltip: String,
    /// The `data-cursor` of the view, empty for the default arrow.
    cursor: String,
    /// The structure being placed on `/play`.
    placement: Placement,
    /// The command button action waiting for a left click on `/play`.
    preselected: Option<Preselect>,
    /// On `/play`: the minimap's idle worker button.
    idle_worker: Option<IdleWorker>,
    idle_finder: idle::Finder,
}

/// The minimap's idle worker button and its count.
struct IdleWorker {
    button: HtmlButtonElement,
    count: HtmlElement,
    /// The count shown, `None` before the first.
    shown: Option<usize>,
}

impl IdleWorker {
    fn new(document: &Document) -> Result<Self, String> {
        let button: HtmlButtonElement = element(document, "idle-worker")?;
        let count: HtmlElement = element(document, "idle-worker-count")?;
        let _ = button.set_attribute(
            tooltip::ATTRIBUTE,
            &format!(
                "{}{}",
                command_row::hotkey_prefix("selection.idleworker"),
                lang::translate("Find idle worker\nNumber of idle workers.")
            ),
        );
        button.set_hidden(false);
        count.set_hidden(false);
        Ok(IdleWorker {
            button,
            count,
            shown: None,
        })
    }

    /// `MiniMapIdleWorkerButton.rebuild`.
    fn show(&mut self, count: usize) {
        if self.shown == Some(count) {
            return;
        }
        self.shown = Some(count);
        self.button.set_disabled(count == 0);
        self.count.set_text_content(Some(&if count == 0 {
            String::new()
        } else {
            count.to_string()
        }));
    }
}

/// The range overlay lists, and the values they and the storage last got,
/// which the view's keys may have changed since.
struct RangeControls {
    selects: [HtmlSelectElement; 3],
    shown: [bool; 3],
}

impl RangeControls {
    fn show(&mut self, ranges: [bool; 3]) {
        if ranges == self.shown {
            return;
        }
        self.shown = ranges;
        let storage = web_sys::window().and_then(|window| window.local_storage().ok().flatten());
        for ((select, key), on) in self.selects.iter().zip(RANGES_SETTINGS_KEYS).zip(ranges) {
            select.set_value(if on { "1" } else { "0" });
            // Best effort: without storage the choice lasts only this visit.
            if let Some(storage) = &storage {
                let _ = storage.set_item(key, if on { "1" } else { "0" });
            }
        }
    }
}

/// The sound toggle and volumes, and what they play.
struct SoundControls {
    audio: Audio,
    button: HtmlButtonElement,
    volumes: HtmlElement,
    master: HtmlInputElement,
    effects: HtmlInputElement,
    music: HtmlInputElement,
}

impl SoundControls {
    fn show(&self) {
        let settings = self.audio.settings();
        let _ = self
            .button
            .set_attribute("aria-pressed", if settings.on { "true" } else { "false" });
        lang::set_text(
            &self.button,
            if settings.on { "Sound on" } else { "Sound off" },
        );
        self.volumes.set_hidden(!settings.on);
        self.master.set_value(&settings.master.to_string());
        self.effects.set_value(&settings.effects.to_string());
        self.music.set_value(&settings.music.to_string());
    }

    /// From a user gesture on the controls.
    fn changed(&mut self, on: bool) {
        let volume = |input: &HtmlInputElement| {
            input
                .value()
                .parse::<f32>()
                .map_or(DEFAULT_VOLUME, |value| value.clamp(0.0, 1.0))
        };
        let settings = Settings {
            on,
            master: volume(&self.master),
            effects: volume(&self.effects),
            music: volume(&self.music),
        };
        self.audio.set_settings(settings);
        save_sound_settings(settings);
        self.show();
    }
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
    /// A stored match opened by its link: it is played from its segments,
    /// and the button leaves it for the live stream.
    archived: bool,
    /// When the worker moves on, on the frame clock.
    switch_at: f64,
    /// Closed by the viewer; it opens again when another match ends.
    closed: bool,
    shown: String,
    /// On `/play`, where the next match is one the player starts: New
    /// match, and Rematch if the server keeps the last setup. The bar's
    /// button is then New match too.
    owner: Option<OwnerButtons>,
}

struct OwnerButtons {
    new_match: HtmlButtonElement,
    rematch: HtmlButtonElement,
    can_rematch: bool,
}

impl MatchOver {
    fn show_status(&mut self, flags: u8, switch_in_ms: u32, now: f64) {
        self.archived = flags & STATUS_ARCHIVE != 0;
        if self.archived {
            self.ended = false;
            self.button.set_hidden(false);
            self.button.set_disabled(false);
            self.popup.set_hidden(true);
            return;
        }
        let ended = flags & STATUS_ENDED != 0;
        if ended && !self.ended {
            self.closed = false;
        }
        self.ended = ended;
        self.next_ready = flags & STATUS_NEXT_READY != 0;
        self.switch_at = now + f64::from(switch_in_ms);
        self.button.set_hidden(!ended);
        if let Some(owner) = &self.owner {
            self.button.set_disabled(false);
            self.popup_button.set_hidden(true);
            owner.new_match.set_hidden(false);
            owner.rematch.set_hidden(!owner.can_rematch);
        } else {
            self.button.set_disabled(!self.next_ready);
            self.popup_button.set_disabled(!self.next_ready);
        }
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
        let text = if self.owner.is_some() {
            lang::translate("Match over")
        } else if left > 0.0 {
            let seconds = (left / 1000.0).ceil() as u64;
            lang::format(
                &lang::translate("Match over. Next match in %(time)s"),
                &[("time", &format!("{}:{:02}", seconds / 60, seconds % 60))],
            )
        } else {
            lang::translate("Match over. Waiting for the next match")
        };
        if text != self.shown {
            self.text.set_text_content(Some(&text));
            self.shown = text;
        }
    }
}

/// The match over popup's New match and Rematch, and the bar's button
/// relabelled New match; an older page has none.
fn owner_buttons(document: &Document, status: Option<&game_menu::Status>) -> Option<OwnerButtons> {
    let new_match = element(document, "match-over-new").ok()?;
    let rematch = element(document, "match-over-rematch").ok()?;
    let bar: HtmlElement = element(document, "current").ok()?;
    lang::set_text(&bar, "New match");
    Some(OwnerButtons {
        new_match,
        rematch,
        can_rematch: status.is_some_and(|status| status.rematch),
    })
}

/// The countdown shown while the server holds back the match's first
/// turns, so that a viewer joining then does not face an empty view.
struct StreamStart {
    /// `/` only.
    element: Option<HtmlElement>,
    /// When the first turn is due, on the frame clock; `None` once one has
    /// arrived.
    at: Option<f64>,
    shown: String,
}

impl StreamStart {
    fn frame(&mut self, now: f64) {
        let Some(element) = &self.element else {
            return;
        };
        let text = self.at.map_or_else(String::new, |at| {
            let seconds = ((at - now).max(0.0) / 1000.0).ceil() as u64;
            lang::format(
                &lang::translate("Live stream starts in %(time)s"),
                &[("time", &format!("{}:{:02}", seconds / 60, seconds % 60))],
            )
        });
        if text != self.shown {
            element.set_hidden(text.is_empty());
            element.set_text_content(Some(&text));
            self.shown = text;
        }
    }
}

impl App {
    /// `now` is the requestAnimationFrame timestamp in milliseconds.
    fn frame(&mut self, now: f64) {
        let start = page_now();
        usage::frame(
            now,
            match (self.page, self.live) {
                (Page::Map, _) => "map",
                (Page::Observer, true) => "live",
                (Page::Observer, false) => "replay",
            },
        );
        let mut drew = false;
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
        // A seat's terrain grows a little most turns; the view is rebuilt
        // with it at most once a second (CONTROL-IMPL V48).
        if self.terrain_revealed
            && now - self.terrain_rebuilt_at >= TERRAIN_REBUILD_MS
            && let Some((id, image)) = &self.terrain_image
            && *id == self.terrain_id
        {
            self.terrain_revealed = false;
            self.terrain_rebuilt_at = now;
            self.renderer.set_terrain(Some(image));
            if let Some(minimap) = &mut self.minimap {
                minimap.set_terrain(Some(image));
            }
            if let Some(warmup) = &mut self.warmup {
                warmup.set_terrains(image.names.clone());
            }
            self.dirty = true;
        }
        if self.scene_changed {
            self.scene_changed = false;
            self.renderer.set_renames(&self.renames);
            self.renames.clear();
            self.renderer.set_scene(&self.scene, &self.palette);
            self.details_dirty = true;
            self.top_dirty = true;
            if let Some(minimap) = &mut self.minimap {
                minimap.set_scene(&self.scene, &self.palette);
            }
            if let Some(idle_worker) = &mut self.idle_worker {
                let count = self
                    .uplink
                    .as_ref()
                    .and_then(Uplink::seat)
                    .map_or(0, |player| idle::count(&self.scene, player, idle::WORKERS));
                idle_worker.show(count);
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
            drew = !self.dirty;
            if !self.dirty {
                self.progress = progress;
            }
        }
        if let Some(minimap) = &mut self.minimap {
            minimap.frame(progress, self.renderer.view_outline());
        }
        self.match_over.frame(now);
        self.stream_start.frame(now);
        if let Some(warmup) = &mut self.warmup {
            let index_loaded = self
                .renderer
                .asset_store()
                .is_some_and(|store| store.borrow().index_name().is_some());
            warmup.frame(index_loaded);
            if warmup.take_request() {
                self.start_warmup();
            }
        }
        self.play_sounds(now, progress);
        self.give_orders();
        if let Some(ranges) = &mut self.ranges {
            ranges.show(self.renderer.ranges());
        }
        self.follow_selection();
        self.update_top(now);
        if let Some(diplomacy_window) = &mut self.diplomacy_window {
            diplomacy_window.update(&diplomacy_window::View {
                scene: &self.scene,
                palette: &self.palette,
                perspective: self.perspective,
            });
        }
        if let Some(trade_window) = &mut self.trade_window {
            trade_window.update(&trade_window::View {
                scene: &self.scene,
                perspective: self.perspective,
                player: self.uplink.as_ref().and_then(Uplink::seat),
                mass_barter: self.mass_barter,
            });
        }
        // The frame moves from the turn before the scene's to the scene's.
        if let Some(details) = &mut self.details
            && !self.renderer.selected().is_empty()
        {
            details.tick(f64::from(self.scene.turn) - 1.0 + f64::from(progress));
        }
        if let Some(research) = &mut self.research {
            research.tick(f64::from(self.scene.turn) - 1.0 + f64::from(progress));
        }
        if let Some(tooltips) = &self.tooltips {
            tooltips.frame(now);
        }
        timing::frame(page_now() - start, drew);
    }

    /// `findIdleUnit(classes)` on `/play`, with selection.add and
    /// selection.offscreen as `append` and `offscreen`.
    fn find_idle(&mut self, classes: &[&str], append: bool, offscreen: bool) {
        let Some(player) = self.uplink.as_ref().and_then(Uplink::seat) else {
            return;
        };
        let selected = self.renderer.selected().to_vec();
        let found =
            self.idle_finder
                .find(&self.scene, player, classes, &selected, append, offscreen);
        let Some(found) = found else {
            if let Some(sound) = &mut self.sound {
                sound
                    .audio
                    .play_group(idle::NO_IDLE_UNIT_SOUND, [0.0; 3], true, page_now());
            }
            return;
        };
        if append {
            for id in found.ids {
                self.renderer.select_entity(id, true);
            }
        } else {
            self.renderer.select_group(&found.ids, false);
        }
        if let Some([x, z]) = found.focus {
            self.renderer.move_to(x, z);
        }
    }

    /// On `/play`: turns the right clicks into the seat's orders, and shows
    /// the match from the seat once it is known. Elsewhere the clicks are
    /// dropped.
    fn toggle_trade(&mut self, open: Option<bool>) {
        toggle_trade(
            &mut self.trade_window,
            &mut self.diplomacy_window,
            self.perspective,
            open,
        );
    }

    /// An order from the trade dialog, for the player of `/play`.
    fn send_trade_order(&self, order: Option<Command>) {
        if let (Some(order), Some(uplink)) = (order, &self.uplink) {
            uplink.send(&order);
        }
    }

    fn give_orders(&mut self) {
        let targets = self.renderer.take_order_targets();
        let Some(seat) = self.uplink.as_ref().and_then(Uplink::seat) else {
            return;
        };
        let players = self.scene.players.len();
        if self.seat_shown != Some(seat) && usize::try_from(seat).is_ok_and(|seat| seat < players) {
            self.seat_shown = Some(seat);
            self.perspective = Perspective::Player(seat as usize);
            self.perspective_select.set_value(&seat.to_string());
            self.update_palette();
            // The panels' buttons work for the seat's own from now on.
            self.details_dirty = true;
        }
        // A player watches as its player until it has won or lost, when
        // vanilla makes it an observer (`isPlayerObserver`); its stream
        // carries no more than that.
        let playing = usize::try_from(seat)
            .ok()
            .and_then(|seat| self.scene.players.get(seat))
            .is_some_and(|player| player.state == PlayerState::Active);
        if self.perspective_select.disabled() != playing {
            self.perspective_select.set_disabled(playing);
        }
        let finished = usize::try_from(seat)
            .ok()
            .and_then(|seat| self.scene.players.get(seat))
            .map(|player| player.state)
            .filter(|state| *state != PlayerState::Active);
        if let Some(state) = finished
            && !self.finish_noticed
        {
            self.finish_noticed = true;
            let won = state == PlayerState::Won;
            let summary_open = self
                .summary
                .as_ref()
                .is_some_and(|summary| summary.try_borrow().is_ok_and(|summary| summary.is_open()));
            // Without the summary window there is nothing to offer, so the
            // page keeps its old behavior.
            let offer = self.summary.is_some() && !self.match_over.ended;
            if offer && summary_open {
                // The summary already stands in for the offer; closing it
                // shows the offer.
                self.offer_return = Some(won);
            } else if offer && self.message_box.is_some() {
                if let Some(message_box) = &mut self.message_box {
                    message_box.offer_result(won);
                }
            } else if !won {
                game_menu::after_defeat();
            }
        }
        if playing && self.perspective != Perspective::Player(seat as usize) {
            self.perspective = Perspective::Player(seat as usize);
            self.perspective_select.set_value(&seat.to_string());
            self.update_palette();
        }
        let formation = self
            .details
            .as_ref()
            .and_then(DetailsPanel::default_formation)
            .unwrap_or_default();
        let selected = self.renderer.selected();
        // The engine's "slight hack": no selection, no preselected action.
        if selected.is_empty() || self.placement.active() {
            self.preselected = None;
        }
        let mut sounds = Vec::new();
        let mut markers = Vec::new();
        for target in targets {
            let action = match (self.preselected, target.left) {
                (Some(preselect), true) => {
                    let action = orders::preselected(
                        &self.scene,
                        selected,
                        seat,
                        &target,
                        &formation,
                        preselect,
                    );
                    // The mode stays for more with session.queue (Shift).
                    if action.is_some() && !target.queued {
                        self.preselected = None;
                    }
                    action
                }
                // The right button only ends the mode.
                (Some(_), false) => {
                    self.preselected = None;
                    None
                }
                (None, true) => None,
                (None, false) => orders::action(
                    &self.scene,
                    selected,
                    seat,
                    &target,
                    &formation,
                    &self.data_files,
                ),
            };
            if let Some(action) = action
                && let Some(order) = action.command
                && let Some(uplink) = &self.uplink
            {
                uplink.send(&order);
                if let Command::Walk { x, z, .. }
                | Command::Patrol { x, z, .. }
                | Command::AttackWalk { x, z, .. } = order
                {
                    markers.push((x, z));
                }
                sounds.extend(action.sound);
            }
        }
        self.renderer.set_preselecting(self.preselected.is_some());
        for (name, id) in sounds {
            self.renderer.queue_sound(id, name);
        }
        for (x, z) in markers {
            self.renderer.add_target_marker(x, z);
        }
        let selected = self.renderer.selected();
        let hotkeys = self.renderer.hotkeys();
        // As `updateCursorAndTooltip`: the preselected action's or the
        // right click's cursor, else the default arrow.
        let cursor = self
            .renderer
            .hovered()
            .and_then(|entity| {
                let target = orders::Target {
                    entity,
                    point: None,
                    queued: false,
                    hotkeys,
                    left: false,
                };
                match self.preselected {
                    Some(preselect) => orders::preselected(
                        &self.scene,
                        selected,
                        seat,
                        &target,
                        &formation,
                        preselect,
                    ),
                    None => orders::action(
                        &self.scene,
                        selected,
                        seat,
                        &target,
                        &formation,
                        &self.data_files,
                    ),
                }
            })
            .map(|action| (action.cursor, action.tooltip))
            .unwrap_or_default();
        let (cursor, tooltip) = cursor;
        if tooltip != self.action_tooltip {
            let _ = self.canvas.set_attribute(tooltip::ATTRIBUTE, &tooltip);
            self.action_tooltip = tooltip;
        }
        if cursor != self.cursor {
            let _ = if cursor.is_empty() {
                self.canvas.remove_attribute("data-cursor")
            } else {
                self.canvas.set_attribute("data-cursor", &cursor)
            };
            self.cursor = cursor;
        }
        if let Some(uplink) = &self.uplink {
            self.placement
                .update(&mut *self.renderer, uplink, &self.scene, seat);
        }
    }

    /// Asks for the detail stream while something is selected.
    fn follow_selection(&mut self) {
        let selected = self.renderer.selected();
        // The player's idle units are counted and found from it. Not before
        // the worker's first scene: until its wasm is loaded it has no
        // handler, and the command would be lost.
        let wanted = !selected.is_empty() || (self.uplink.is_some() && self.scene.match_id != 0);
        if wanted != self.detail_wanted {
            self.detail_wanted = wanted;
            self.command(COMMAND_DETAIL, f64::from(u8::from(wanted)));
        }
        let Some(details) = &mut self.details else {
            return;
        };
        for request in details.take_requests() {
            match request {
                Request::Order(order) => {
                    if let Some(uplink) = &self.uplink {
                        uplink.send(&order);
                        self.predictions.record(&order, &self.scene, page_now());
                        self.details_dirty = true;
                    }
                }
                Request::Orders(orders) => {
                    if let Some(uplink) = &self.uplink {
                        for order in &orders {
                            uplink.send(order);
                            self.predictions.record(order, &self.scene, page_now());
                        }
                        self.details_dirty = true;
                    }
                }
                Request::Preselect(preselect) => {
                    if self.uplink.is_some() {
                        self.preselected = Some(preselect);
                    }
                }
                Request::Place(place) => {
                    if let Some(uplink) = &self.uplink {
                        self.placement.start(place, uplink);
                    }
                }
                Request::ConfirmDelete(entities) => {
                    if let Some(message_box) = &mut self.message_box {
                        // `closeOpenDialogs`.
                        if let Some(diplomacy_window) = &mut self.diplomacy_window {
                            diplomacy_window.set_open(false);
                        }
                        if let Some(trade_window) = &mut self.trade_window {
                            trade_window.set_open(false, self.perspective);
                        }
                        message_box.confirm_delete(entities);
                    }
                }
                Request::ToggleTrade => toggle_trade(
                    &mut self.trade_window,
                    &mut self.diplomacy_window,
                    self.perspective,
                    None,
                ),
                request => apply_request(&mut *self.renderer, request, &self.scene),
            }
        }
        if details.take_redraw() {
            self.details_dirty = true;
        }
        let store = self.renderer.asset_store();
        let index = store
            .as_ref()
            .is_some_and(|store| store.borrow().index_name().is_some());
        // A tooltip's data that arrived is shown at once.
        if self.data_files.take_loaded() {
            self.details_dirty = true;
        }
        if self.predictions.reconcile(&self.scene, page_now()) {
            self.details_dirty = true;
        }
        let selected = self.renderer.selected();
        if !self.details_dirty && index == self.details_index && selected == self.details_selected {
            return;
        }
        self.details_dirty = false;
        self.details_index = index;
        self.details_selected = selected.to_vec();
        let portrait = |icon: &str| store.as_ref()?.borrow().portrait_url(icon);
        let technology_portrait =
            |technology: &str| store.as_ref()?.borrow().technology_portrait_url(technology);
        let template_portrait =
            |template: &str| store.as_ref()?.borrow().template_portrait_url(template);
        let view = panel::View {
            scene: &self.scene,
            selected,
            palette: &self.palette,
            perspective: self.perspective,
            portrait: &portrait,
            technology_portrait: &technology_portrait,
            template_portrait: &template_portrait,
            data: &self.data_files,
            detailed: self.detailed_tooltips,
            player: self.uplink.as_ref().and_then(Uplink::seat),
            mass_barter: self.mass_barter,
            predicted: &self.predictions,
        };
        details.update(&view);
        if let Some(research) = &mut self.research {
            research.update(&view);
        }
    }

    /// Fills the top panel when its sources changed, and runs its fades.
    fn update_top(&mut self, now: f64) {
        if let Some(research) = &self.research {
            for request in research.take_requests() {
                apply_request(&mut *self.renderer, request, &self.scene);
            }
        }
        let Some(top) = &mut self.top else {
            return;
        };
        for request in top.take_requests() {
            apply_request(&mut *self.renderer, request, &self.scene);
        }
        let store = self.renderer.asset_store();
        let index = store
            .as_ref()
            .is_some_and(|store| store.borrow().index_name().is_some());
        if self.top_dirty || index != self.top_index {
            self.top_dirty = false;
            self.top_index = index;
            let portrait = |icon: &str| store.as_ref()?.borrow().portrait_url(icon);
            top.update(
                &top::View {
                    scene: &self.scene,
                    palette: &self.palette,
                    perspective: self.perspective,
                    portrait: &portrait,
                },
                now,
            );
        }
        top.tick(now);
    }

    /// Covers the civs, the terrains and the entities on the map now.
    fn start_warmup(&mut self) {
        let (Some(store), Some(warmup)) = (self.renderer.asset_store(), &mut self.warmup) else {
            return;
        };
        let actors: BTreeSet<&str> = self
            .scene
            .dots
            .values()
            .filter_map(|dot| {
                let index = dot.actor.checked_sub(1)?;
                self.scene.strings.get(index as usize).map(String::as_str)
            })
            .collect();
        let actors: Vec<&str> = actors.into_iter().collect();
        warmup.start(&store.borrow(), &actors);
    }

    fn play_sounds(&mut self, now: f64, progress: f32) {
        // Taken even when nothing plays them, so they do not pile up.
        let entity_sounds = self.renderer.take_entity_sounds();
        let Some(sound) = &mut self.sound else {
            return;
        };
        if sound.audio.settings().on {
            for animation in self.renderer.take_animation_sounds(progress) {
                let Some(group) = self.scene.strings.get(animation.group as usize) else {
                    continue;
                };
                if let Some(owned) = hears(self.perspective, animation.owner, animation.visibility)
                {
                    sound
                        .audio
                        .play_group(group, animation.position, owned, now);
                }
            }
        }
        // `CCmpSoundManager::PlaySoundGroup` with no LOS test: selection
        // under a player perspective works as under Observer, and an order
        // goes to the player's own, which it always sees.
        for entity_sound in entity_sounds {
            let owned = matches!(self.perspective, Perspective::Player(shown) if usize::try_from(entity_sound.owner) == Ok(shown));
            sound
                .audio
                .play_group(&entity_sound.group, entity_sound.position, owned, now);
        }
        let (civ, kind) = music_for(&self.scene, self.perspective);
        sound.audio.set_music(civ, kind);
        sound.audio.frame(now, self.renderer.listener().as_ref());
    }

    /// The sounds of the delta just applied.
    fn play_turn_sounds(&mut self) {
        let Some(sound) = &mut self.sound else {
            return;
        };
        let now = page_now();
        for event in &self.scene.sounds {
            let Some(group) = self.scene.strings.get(event.group as usize) else {
                continue;
            };
            if let Some((position, owned)) = heard_event(self.perspective, event) {
                sound.audio.play_group(group, position, owned, now);
            }
        }
    }

    /// Whether the viewer's own seat has the game paused.
    fn own_pause(&self) -> bool {
        self.owner_seat
            .is_some_and(|seat| self.scene.paused_by.contains(&(seat as u32)))
    }

    /// Shows the pause veil and the Menu's button as the scene has them.
    fn sync_pause(&mut self) {
        let own = self.owner_seat;
        if let Some(overlay) = &mut self.pause_overlay {
            overlay.update(&self.scene.paused_by, &self.scene.players, own);
        }
        if let Some(menu) = &self.game_menu {
            menu.update(
                !seated() || self.finish_noticed,
                self.match_over.ended,
                self.own_pause(),
            );
        }
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
            self.seat_shown = None;
        }
        if let Some(warmup) = &mut self.warmup {
            warmup.set_civs(
                players
                    .iter()
                    .skip(1)
                    .map(|player| player.civ.clone())
                    .collect(),
            );
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
        self.show_live_delay();
    }

    /// The engine's `NetworkDelayOverlay`, here for the server's delay.
    /// Retranslated on every call, since a formatted text is not one i18n.js
    /// can redo on a language change.
    fn show_live_delay(&mut self) {
        let Some(live_delay) = &self.live_delay else {
            return;
        };
        let text = if self.scene.delay_ms == 0 || self.match_over.ended || self.match_over.archived
        {
            String::new()
        } else {
            let seconds = f64::from(self.scene.delay_ms) / 1000.0;
            let delay = format!("{seconds:.*}", if seconds < 5.0 { 1 } else { 0 });
            lang::format(
                &lang::translate("Delay to live stream: %(delay)ss"),
                &[("delay", &delay)],
            )
        };
        if text != self.live_delay_text {
            live_delay.set_hidden(text.is_empty());
            live_delay.set_text_content(Some(&text));
            self.live_delay_text = text;
        }
    }

    fn show_status(&mut self, first: u32, newest: u32, playhead: u32, flags: u8, speed: f32) {
        self.live = flags & STATUS_LIVE != 0;
        self.paused = flags & STATUS_PAUSED != 0;
        self.playhead = playhead;
        if let Some(copy_link) = &self.copy_link {
            copy_link.set_disabled(false);
        }
        // A stored match has no live edge to go to.
        self.live_button.set_hidden(flags & STATUS_ARCHIVE != 0);
        self.play_button.set_disabled(false);
        lang::set_text(
            &self.play_button,
            if self.paused { "Play" } else { "Pause" },
        );
        // For the phone layout's icon, which CSS cannot take from the text.
        let paused = if self.paused { "true" } else { "false" };
        let _ = self.play_button.set_attribute("data-paused", paused);
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
        // A capture covers 30 s; while the window is open, ask only once
        // the playhead leaves what it can stand for.
        let at_end = playhead >= newest && (self.match_over.ended || self.match_over.archived);
        if let (Some(summary), Some(source)) = (self.summary.clone(), summary_source(self)) {
            summary::follow(&summary, source, playhead, at_end);
        }
    }

    fn update_palette(&mut self) {
        self.palette =
            diplomacy::palette(&self.scene.players, self.diplomacy_colors, self.perspective);
        self.renderer
            .set_all_status_bars(self.all_status_bars.then_some(self.perspective));
        self.renderer
            .set_perspective(self.perspective, self.uplink.is_some());
        if let Some(minimap) = &mut self.minimap {
            minimap.set_perspective(self.perspective);
        }
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
/// `glue_url`, `wasm_url` and `icons_url` as for `observer_main`.
#[wasm_bindgen]
pub fn page_main(glue_url: String, wasm_url: String, icons_url: Option<String>) {
    set_panic_hook();
    lang::start_page();
    let icons_url = icons_url.unwrap_or_else(|| "/icons.bin".to_string());
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = run(Page::Map, glue_url, wasm_url, icons_url, None).await {
            usage::fail(&error);
            web_sys::console::error_1(&error.into());
        }
    });
}

/// Entry point of index.html: the same stream and controls as the
/// page's, drawn in 3D. `glue_url` and `wasm_url` are the URLs the page loaded
/// this module from; the stream worker loads the same ones, so a cached
/// older worker can never pair an old glue with this wasm.
/// `selection_url` and `icons_url` name the selection and minimap icon
/// atlases by their versions; a page from before them passes none.
#[wasm_bindgen]
pub fn observer_main(
    glue_url: String,
    wasm_url: String,
    selection_url: Option<String>,
    icons_url: Option<String>,
) {
    set_panic_hook();
    let selection_url = selection_url.unwrap_or_else(|| "/selection.bin".to_string());
    let icons_url = icons_url.unwrap_or_else(|| "/icons.bin".to_string());
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = run(
            Page::Observer,
            glue_url,
            wasm_url,
            icons_url,
            Some(selection_url),
        )
        .await
        {
            usage::fail(&error);
            web_sys::console::error_1(&error.into());
        }
    });
}

/// Entry point of play.html, the setup of a game the server hosts: only
/// the language, for the page's own script to translate with.
#[wasm_bindgen]
pub fn play_setup_main() {
    set_panic_hook();
    lang::start_page();
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

/// `icons_url` is the minimap icon atlas, `selection_url` the selection
/// atlas to draw selections with, if any.
async fn run(
    page: Page,
    glue_url: String,
    wasm_url: String,
    icons_url: String,
    selection_url: Option<String>,
) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    usage::start();
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
    let sound = match (page, renderer.asset_store()) {
        (Page::Observer, Some(store)) => Some(SoundControls {
            audio: Audio::new(store, load_sound_settings()),
            button: element(&document, "sound")?,
            volumes: element(&document, "volumes")?,
            master: element(&document, "volume-master")?,
            effects: element(&document, "volume-effects")?,
            music: element(&document, "volume-music")?,
        }),
        _ => None,
    };
    let details = match page {
        Page::Map => None,
        // An older page has no panel; the match still shows.
        Page::Observer => DetailsPanel::new(&document)
            .inspect_err(|error| web_sys::console::error_1(&format!("details: {error}").into()))
            .ok(),
    };
    let top = match page {
        Page::Map => None,
        // An older page has no top panel; the match still shows.
        Page::Observer => TopPanel::new(&document)
            .inspect_err(|error| web_sys::console::error_1(&format!("top panel: {error}").into()))
            .ok(),
    };
    let research = match page {
        Page::Map => None,
        // An older page has no research overlay; the match still shows.
        Page::Observer => ResearchProgress::new(&document)
            .inspect_err(|error| web_sys::console::error_1(&format!("research: {error}").into()))
            .ok(),
    };
    let diplomacy_window = match page {
        Page::Map => None,
        // An older page has no diplomacy window; the match still shows.
        Page::Observer => DiplomacyWindow::new(&document)
            .inspect_err(|error| {
                web_sys::console::error_1(&format!("diplomacy window: {error}").into())
            })
            .ok(),
    };
    let summary = match page {
        Page::Map => None,
        // An older page has no summary window; the match still shows.
        Page::Observer => SummaryWindow::new(&document)
            .inspect_err(|error| web_sys::console::error_1(&format!("summary: {error}").into()))
            .ok(),
    };
    let trade_window = match page {
        Page::Map => None,
        // An older page has no trade dialog; the match still shows.
        Page::Observer => TradeWindow::new(&document)
            .inspect_err(|error| {
                web_sys::console::error_1(&format!("trade window: {error}").into())
            })
            .ok(),
    };
    let tooltips = match page {
        Page::Map => None,
        // An older page has no tooltip box; the panels still show.
        Page::Observer => tooltip::Tooltips::new(&document).ok(),
    };
    let warmup = match page {
        Page::Map => None,
        Page::Observer => Some(Warmup::new(element(&document, "warmup")?)),
    };
    // `/play` is the 3D page for the player of a game the server hosts: the
    // live stream, and orders. The server gives it only to that player.
    let owner = page == Page::Observer && window.location().pathname().as_deref() == Ok("/play");
    let status = if owner {
        game_menu::status()
            .await
            .inspect_err(|error| web_sys::console::error_1(&error.into()))
            .ok()
    } else {
        None
    };
    // A player who resigned or was beaten watches the rest as the observers
    // do, with their delay, and gives no more orders.
    let seated = owner && !status.as_ref().is_some_and(|status| status.defeated);
    SEATED.store(seated, Ordering::Relaxed);
    // Always there, even empty, so the tooltips follow the view from the
    // first move and show an action's text as soon as it has one.
    if seated && page == Page::Observer {
        let _ = canvas.set_attribute(tooltip::ATTRIBUTE, "");
        let _ = canvas.set_attribute(tooltip::STYLE_ATTRIBUTE, tooltip::INFORMATION);
    }
    // An older page has no message box; Delete then does nothing without
    // Shift.
    let message_box = if owner {
        MessageBox::new(&document)
            .inspect_err(|error| web_sys::console::error_1(&format!("message box: {error}").into()))
            .ok()
    } else {
        None
    };
    // An older page has no menu; the game then ends only by itself.
    let game_menu = if owner {
        GameMenu::new(&document)
            .inspect_err(|error| web_sys::console::error_1(&format!("game menu: {error}").into()))
            .ok()
    } else {
        None
    };
    // An older page has no overlay; the game then pauses without a sign.
    let pause_overlay = if page == Page::Observer {
        PauseOverlay::new(&document)
            .inspect_err(|error| {
                web_sys::console::error_1(&format!("pause overlay: {error}").into())
            })
            .ok()
    } else {
        None
    };
    // An older page has no button; the hotkeys still work.
    let idle_worker = if seated {
        IdleWorker::new(&document)
            .inspect_err(|error| web_sys::console::error_1(&format!("idle worker: {error}").into()))
            .ok()
    } else {
        None
    };
    let live_delay = match page {
        Page::Map => None,
        // The player's stream has no delay, whatever the observers' is.
        Page::Observer if seated => None,
        // A cached older page has no label; the match still shows.
        Page::Observer => element(&document, "live-delay").ok(),
    };
    let stream_start = match page {
        Page::Map => None,
        // A cached older page has no countdown; the match still shows.
        Page::Observer => element(&document, "stream-start").ok(),
    };
    let options = WorkerOptions::new();
    options.set_type(WorkerType::Module);
    let mut worker_url = format!(
        "worker.js?glue={}&wasm={}",
        js_sys::encode_uri_component(&glue_url),
        js_sys::encode_uri_component(&wasm_url)
    );
    if seated {
        worker_url.push_str("&play=1");
    }
    // A shared link names a match and a moment in it; the worker opens it.
    let search = window.location().search().unwrap_or_default();
    for name in ["match", "t"] {
        if let Some(value) = query_param(&search, name) {
            worker_url.push_str(&format!("&{name}={}", js_sys::encode_uri_component(&value)));
        }
    }
    let worker = Worker::new_with_options(&worker_url, &options)
        .map_err(|_| "cannot start the stream worker")?;

    let app = Rc::new(RefCell::new(App {
        page,
        canvas,
        renderer,
        minimap,
        loading,
        scene: Scene::for_page(),
        progress: 1.0,
        turn_start: 0.0,
        turn_interval: TURN_MS,
        measure_turns: true,
        turn_arrived: false,
        scene_changed: false,
        renames: Vec::new(),
        dirty: true,
        terrain_id: 0,
        terrain_image: None,
        terrain_revealed: false,
        terrain_rebuilt_at: 0.0,
        diplomacy_colors: false,
        perspective: Perspective::Observer,
        all_status_bars: false,
        palette: Vec::new(),
        diplomacy_button,
        perspective_select,
        player_names: Vec::new(),
        worker,
        clock: element(&document, "clock")?,
        clock_turn: None,
        live_delay,
        live_delay_text: String::new(),
        play_button: element(&document, "play")?,
        speed_select: element(&document, "speed")?,
        speeds_for: 0.0,
        timeline: element(&document, "timeline")?,
        dragging: false,
        live_button: element(&document, "live")?,
        live: true,
        paused: false,
        playhead: 0,
        copy_link: match page {
            Page::Map => None,
            Page::Observer => element(&document, "copy-link").ok(),
        },
        copy_link_timer: None,
        stream_bytes: StreamBytes::default(),
        match_over: MatchOver {
            button: element(&document, "current")?,
            popup: element(&document, "match-over")?,
            text: element(&document, "match-over-text")?,
            popup_button: element(&document, "match-over-current")?,
            close_button: element(&document, "match-over-close")?,
            ended: false,
            next_ready: false,
            archived: false,
            switch_at: 0.0,
            closed: false,
            shown: String::new(),
            owner: if owner {
                owner_buttons(&document, status.as_ref())
            } else {
                None
            },
        },
        stream_start: StreamStart {
            element: stream_start,
            at: None,
            shown: String::new(),
        },
        sound,
        warmup,
        detail_wanted: false,
        details,
        details_dirty: true,
        details_selected: Vec::new(),
        details_index: false,
        top,
        research,
        diplomacy_window,
        trade_window,
        mass_barter: false,
        message_box,
        game_menu,
        pause_overlay,
        finish_noticed: false,
        summary,
        owner_seat: status
            .as_ref()
            .map(|status| status.seat)
            .filter(|seat| owner && *seat != 0),
        offer_return: None,
        predictions: Predictions::default(),
        top_dirty: true,
        top_index: false,
        ranges: None,
        tooltips,
        data_files: tips::DataFiles::default(),
        detailed_tooltips: false,
        uplink: seated.then(Uplink::open),
        idle_worker,
        idle_finder: idle::Finder::default(),
        seat_shown: None,
        cursor: String::new(),
        action_tooltip: String::new(),
        placement: Placement::default(),
        preselected: None,
    }));
    wire_controls(&app);
    wire_details(&app);
    wire_top(&app);
    wire_research(&app);
    wire_diplomacy_window(&app, &window);
    wire_trade_window(&app, &window);
    wire_message_box(&app, &window);
    wire_game_menu(&app);
    wire_pause(&app, &window);
    wire_summary(&app, &window);
    wire_command_hotkeys(&app, &window);
    wire_idle(&app, &window);
    if page == Page::Observer {
        let shadows: HtmlSelectElement = element(&document, "shadows")?;
        wire_shadows(&app, shadows.clone());
        wire_shadow_cutoff(
            &app,
            element(&document, "shadowscutoff")?,
            element(&document, "shadowscutoff-value")?,
            &shadows,
        );
        wire_language(&app, element(&document, "language")?);
        wire_textures(&app, element(&document, "textures")?);
        wire_maps(&app, element(&document, "maps")?);
        wire_triplanar(&app, element(&document, "triplanar")?);
        wire_wind(&app, element(&document, "wind")?);
        let postproc: HtmlSelectElement = element(&document, "postproc")?;
        let bloom: HtmlSelectElement = element(&document, "bloom")?;
        let dof: HtmlSelectElement = element(&document, "dof")?;
        let antialiasing: HtmlSelectElement = element(&document, "antialiasing")?;
        let sharpening: HtmlSelectElement = element(&document, "sharpening")?;
        let render_scale: HtmlSelectElement = element(&document, "renderscale")?;
        wire_switch(
            &app,
            postproc.clone(),
            POSTPROC_SETTINGS_KEY,
            |view, on| view.set_postproc(on),
            vec![
                bloom.clone(),
                dof.clone(),
                antialiasing.clone(),
                sharpening.clone(),
                render_scale.clone(),
            ],
        );
        wire_render_scale(
            &app,
            render_scale,
            element(&document, "upscale")?,
            postproc.clone(),
        );
        wire_switch(
            &app,
            antialiasing,
            ANTIALIASING_SETTINGS_KEY,
            |view, on| view.set_antialiasing(on),
            Vec::new(),
        );
        wire_switch(
            &app,
            sharpening.clone(),
            SHARPENING_SETTINGS_KEY,
            |view, on| view.set_sharpening(on),
            Vec::new(),
        );
        wire_sharpness(
            &app,
            element(&document, "sharpness")?,
            element(&document, "sharpness-value")?,
            vec![postproc, sharpening],
        );
        wire_switch(
            &app,
            bloom,
            BLOOM_SETTINGS_KEY,
            |view, on| view.set_bloom(on),
            Vec::new(),
        );
        wire_switch(
            &app,
            dof,
            DOF_SETTINGS_KEY,
            |view, on| view.set_dof(on),
            Vec::new(),
        );
        wire_switch(
            &app,
            element(&document, "fog")?,
            FOG_SETTINGS_KEY,
            |view, on| view.set_fog(on),
            Vec::new(),
        );
        let water: HtmlSelectElement = element(&document, "water")?;
        let fancy: HtmlSelectElement = element(&document, "waterfancyeffects")?;
        let reflection: HtmlSelectElement = element(&document, "waterreflection")?;
        let refraction: HtmlSelectElement = element(&document, "waterrefraction")?;
        let real_depth: HtmlSelectElement = element(&document, "waterrealdepth")?;
        wire_switch(
            &app,
            water.clone(),
            WATER_SETTINGS_KEY,
            |view, on| view.set_water_effects(on),
            vec![fancy.clone(), reflection.clone(), refraction.clone()],
        );
        wire_switch(
            &app,
            fancy,
            FANCY_WATER_SETTINGS_KEY,
            |view, on| view.set_fancy_water(on),
            Vec::new(),
        );
        wire_switch(
            &app,
            reflection,
            REFLECTION_SETTINGS_KEY,
            |view, on| view.set_water_reflection(on),
            Vec::new(),
        );
        wire_switch(
            &app,
            refraction.clone(),
            REFRACTION_SETTINGS_KEY,
            |view, on| view.set_water_refraction(on),
            Vec::new(),
        );
        wire_switch(
            &app,
            real_depth.clone(),
            REAL_DEPTH_SETTINGS_KEY,
            |view, on| view.set_water_real_depth(on),
            Vec::new(),
        );
        wire_depends_on_all(&real_depth, vec![water, refraction]);
        wire_switch(
            &app,
            element(&document, "sky")?,
            SKY_SETTINGS_KEY,
            |view, on| view.set_sky(on),
            Vec::new(),
        );
        wire_switch_default(
            &app,
            element(&document, "worldsupport")?,
            WORLD_SUPPORT_SETTINGS_KEY,
            |view, on| view.set_world_support(on),
            Vec::new(),
            true,
        );
        wire_switch(
            &app,
            element(&document, "particles")?,
            PARTICLES_SETTINGS_KEY,
            |view, on| view.set_particles(on),
            Vec::new(),
        );
        wire_switch(
            &app,
            element(&document, "silhouettes")?,
            SILHOUETTES_SETTINGS_KEY,
            |view, on| view.set_silhouettes(on),
            Vec::new(),
        );
        wire_projectiles(&app, element(&document, "projectiles")?);
        wire_detailed_tooltips(&app, element(&document, "detailed-tooltips")?);
        wire_status_bars(&app, element(&document, "status-bars")?);
        wire_ranges(
            &app,
            [
                element(&document, "ranges-attack")?,
                element(&document, "ranges-aura")?,
                element(&document, "ranges-heal")?,
            ],
        );
    }
    wire_sound(&app, &window);
    wire_warmup(&app);
    if page == Page::Observer {
        wire_storage_notice(&app, &document)?;
    }
    wire_playback(&app);
    let bench = match page {
        Page::Map => None,
        Page::Observer => {
            let bench = Rc::new(RefCell::new(Bench::new(&document)?));
            bench::wire(&bench, &app);
            Some(bench)
        }
    };
    connect(&app);
    wasm_bindgen_futures::spawn_local(load_icons(Rc::clone(&app), icons_url));
    if let Some(url) = selection_url {
        wasm_bindgen_futures::spawn_local(load_selection_atlas(Rc::clone(&app), url));
    }
    animate(app, bench);
    Ok(())
}

fn element<T: JsCast>(document: &Document, id: &str) -> Result<T, String> {
    document
        .get_element_by_id(id)
        .and_then(|element| element.dyn_into().ok())
        .ok_or_else(|| format!("no element #{id} of the right kind"))
}

/// The details panel's portrait follows its entity; a group button
/// selects its group, or with Ctrl (session.deselectgroup) or the right
/// button removes it.
fn wire_details(app: &Rc<RefCell<App>>) {
    let (portrait, buttons, (sells, buys), commands, formations) = {
        let app = app.borrow();
        let Some(details) = &app.details else {
            return;
        };
        (
            details.portrait_button().clone(),
            details.group_buttons().cloned().collect::<Vec<_>>(),
            details.barter_buttons(),
            details.command_buttons(),
            details.formation_buttons(),
        )
    };
    // On `/play` they give the player's orders; for an observer they do
    // nothing, as in the game.
    for (command, button) in commands {
        let target = Rc::clone(app);
        let on_click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            if let Ok(mut app) = target.try_borrow_mut() {
                if let Some(details) = &mut app.details {
                    details.command_clicked(command, event.shift_key(), event.ctrl_key());
                }
                app.details_dirty = true;
            }
        });
        button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
        on_click.forget();
    }
    let target = Rc::clone(app);
    let on_click = Closure::<dyn FnMut()>::new(move || {
        if let Some(details) = &target.borrow().details {
            details.portrait_clicked();
        }
    });
    portrait.set_onclick(Some(on_click.as_ref().unchecked_ref()));
    // The panel lives as long as the page, so its handlers do too.
    on_click.forget();
    for (index, button) in sells.into_iter().enumerate() {
        let target = Rc::clone(app);
        let on_click = Closure::<dyn FnMut()>::new(move || {
            if let Some(details) = &target.borrow().details {
                details.barter_clicked(index);
            }
        });
        button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
        on_click.forget();
    }
    for (index, button) in buys.into_iter().enumerate() {
        let target = Rc::clone(app);
        let on_click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            if let Some(details) = &target.borrow().details {
                details.barter_buy_clicked(index, event.shift_key());
            }
        });
        button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
        on_click.forget();
    }
    for (index, button) in formations.into_iter().enumerate() {
        let target = Rc::clone(app);
        let on_menu = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            event.prevent_default();
            if let Some(details) = &target.borrow().details {
                details.formation_right_clicked(index);
            }
        });
        button.set_oncontextmenu(Some(on_menu.as_ref().unchecked_ref()));
        on_menu.forget();
    }
    for (index, button) in buttons.into_iter().enumerate() {
        let target = Rc::clone(app);
        let on_click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            if let Some(details) = &target.borrow().details {
                details.group_clicked(index, event.ctrl_key());
            }
        });
        button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
        on_click.forget();
        let target = Rc::clone(app);
        let on_menu = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            event.prevent_default();
            if let Some(details) = &target.borrow().details {
                details.group_clicked(index, true);
            }
        });
        button.set_oncontextmenu(Some(on_menu.as_ref().unchecked_ref()));
        on_menu.forget();
    }
}

/// `TradeDialog.toggle`, or with `open` set to it; opening closes the
/// diplomacy window, as `closeOpenDialogs`.
fn toggle_trade(
    trade_window: &mut Option<TradeWindow>,
    diplomacy_window: &mut Option<DiplomacyWindow>,
    perspective: Perspective,
    open: Option<bool>,
) {
    let Some(trade_window) = trade_window else {
        return;
    };
    let open = open.unwrap_or(!trade_window.is_open());
    if open && let Some(diplomacy_window) = diplomacy_window {
        diplomacy_window.set_open(false);
    }
    trade_window.set_open(open, perspective);
}

/// What a panel's button asked for.
fn apply_request(renderer: &mut dyn View, request: Request, scene: &Scene) {
    match request {
        Request::Follow(id) => renderer.follow(id),
        Request::Group { ids, remove } => renderer.select_group(&ids, remove),
        Request::Select { id, add } => renderer.select_entity(id, add),
        // `selectAndMoveTo` does nothing without a position.
        Request::SelectAndFollow(id) => {
            if scene.dots.contains_key(&id) {
                renderer.select_entity(id, false);
                renderer.follow(id);
            }
        }
        Request::Focus { x, z } => renderer.move_to(x, z),
        // The caller sends it, as it holds the uplink.
        Request::Order(_)
        | Request::Orders(_)
        | Request::Preselect(_)
        | Request::Place(_)
        | Request::ConfirmDelete(_)
        | Request::ToggleTrade => {}
    }
}

/// A hero button selects its entity, with Shift (selection.add) adds it,
/// and on a double click also follows it with the camera.
/// The buttons that open the window (`#diplomacy-open` in the controls,
/// `#top-diplomacy` in the top bar, as `diplomacyButton`), its Close, and
/// the engine's session.gui.diplomacy.toggle (Ctrl+H); Escape closes it,
/// as `closeOpenDialogs`.
fn wire_diplomacy_window(app: &Rc<RefCell<App>>, window: &web_sys::Window) {
    let Some(close) = app
        .borrow()
        .diplomacy_window
        .as_ref()
        .map(|diplomacy_window| diplomacy_window.close_button().clone())
    else {
        return;
    };
    let set = |app: &Rc<RefCell<App>>, open: Option<bool>| {
        let app = &mut *app.borrow_mut();
        if let Some(diplomacy_window) = &mut app.diplomacy_window {
            let open = open.unwrap_or(!diplomacy_window.is_open());
            // `closeOpenDialogs`.
            if open && let Some(trade_window) = &mut app.trade_window {
                trade_window.set_open(false, app.perspective);
            }
            diplomacy_window.set_open(open);
        }
    };
    let target = Rc::clone(app);
    on_click(&close, move || set(&target, Some(false)));
    let Some(document) = window.document() else {
        return;
    };
    for id in ["diplomacy-open", "top-diplomacy"] {
        // An older page lacks either; the window still opens by key.
        if let Ok(button) = element::<HtmlElement>(&document, id) {
            let target = Rc::clone(app);
            on_click(&button, move || set(&target, None));
        }
    }
    let target = Rc::clone(app);
    let on_key = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let text = |object: &JsValue, name: &str| {
            js_sys::Reflect::get(object, &name.into())
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_default()
        };
        let flag = |name: &str| {
            js_sys::Reflect::get(&event, &name.into()).is_ok_and(|value| value.is_truthy())
        };
        let tag = event
            .target()
            .map(|element| text(&element, "tagName"))
            .unwrap_or_default();
        if matches!(tag.as_str(), "INPUT" | "SELECT" | "TEXTAREA")
            || flag("altKey")
            || flag("metaKey")
        {
            return;
        }
        match text(&event, "code").as_str() {
            "KeyH" if flag("ctrlKey") && !flag("shiftKey") => {
                // The browser's history panel would take it otherwise.
                event.prevent_default();
                if !flag("repeat") {
                    set(&target, None);
                }
            }
            "Escape" if !flag("ctrlKey") && !flag("shiftKey") => set(&target, Some(false)),
            _ => {}
        }
    });
    let _ = window.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
    // The window lives as long as the page, so its handler does too.
    on_key.forget();
}

/// The Barter & Trade dialog: `#top-trade` (`tradeButton`), its Close, its
/// barter and trade buttons, and the engine's session.gui.barter.toggle
/// (Ctrl+B); Escape closes it, as `closeOpenDialogs`. Shift is also
/// session.massbarter and session.fulltradeswap, held while pressing, and
/// the barter captions follow it as the engine's do.
fn wire_trade_window(app: &Rc<RefCell<App>>, window: &web_sys::Window) {
    let Some((close, sells, buys, trades)) =
        app.borrow().trade_window.as_ref().map(|trade_window| {
            let barter = trade_window.barter();
            (
                trade_window.close_button().clone(),
                barter.sell_buttons().cloned().collect::<Vec<_>>(),
                barter.buy_buttons().cloned().collect::<Vec<_>>(),
                trade_window.trade_buttons(),
            )
        })
    else {
        return;
    };
    let target = Rc::clone(app);
    on_click(&close, move || {
        target.borrow_mut().toggle_trade(Some(false))
    });
    if let Some(button) = window
        .document()
        .and_then(|document| element::<HtmlElement>(&document, "top-trade").ok())
    {
        let target = Rc::clone(app);
        on_click(&button, move || target.borrow_mut().toggle_trade(None));
    }
    for (index, button) in sells.into_iter().enumerate() {
        let target = Rc::clone(app);
        on_click(&button, move || {
            if let Some(trade_window) = &target.borrow().trade_window {
                trade_window.barter().sell_clicked(index);
            }
        });
    }
    for (index, button) in buys.iter().enumerate() {
        on_shift_click(app, button, move |app, shift| {
            let order = app
                .trade_window
                .as_ref()
                .and_then(|trade_window| trade_window.barter().buy_clicked(index, shift));
            app.send_trade_order(order);
        });
    }
    for (index, (button, up, down)) in trades.iter().enumerate() {
        on_shift_click(app, button, move |app, shift| {
            let order = app
                .trade_window
                .as_mut()
                .and_then(|trade_window| trade_window.trade_clicked(index, shift));
            app.send_trade_order(order);
        });
        for (arrow, is_up) in [(up, true), (down, false)] {
            on_shift_click(app, arrow, move |app, _| {
                let order = app
                    .trade_window
                    .as_mut()
                    .and_then(|trade_window| trade_window.arrow_clicked(index, is_up));
                app.send_trade_order(order);
            });
        }
    }
    let target = Rc::clone(app);
    let on_key = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let text = |object: &JsValue, name: &str| {
            js_sys::Reflect::get(object, &name.into())
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_default()
        };
        let flag = |name: &str| {
            js_sys::Reflect::get(&event, &name.into()).is_ok_and(|value| value.is_truthy())
        };
        let Ok(mut app) = target.try_borrow_mut() else {
            return;
        };
        // On every key, as a Shift released over another window is only
        // seen on the next key or blur.
        let shift = flag("shiftKey");
        if shift != app.mass_barter {
            if !shift && let Some(details) = &mut app.details {
                details.flush_batch();
            }
            app.mass_barter = shift;
            app.details_dirty = true;
        }
        if event.type_() != "keydown" {
            return;
        }
        let tag = event
            .target()
            .map(|element| text(&element, "tagName"))
            .unwrap_or_default();
        if matches!(tag.as_str(), "INPUT" | "SELECT" | "TEXTAREA")
            || flag("altKey")
            || flag("metaKey")
        {
            return;
        }
        match text(&event, "code").as_str() {
            "KeyB" if flag("ctrlKey") && !shift => {
                // The browser's bookmarks panel would take it otherwise.
                event.prevent_default();
                if !flag("repeat") {
                    app.toggle_trade(None);
                }
            }
            "Escape" if !flag("ctrlKey") && !shift => app.toggle_trade(Some(false)),
            _ => {}
        }
    });
    for kind in ["keydown", "keyup"] {
        let _ = window.add_event_listener_with_callback(kind, on_key.as_ref().unchecked_ref());
    }
    // The dialog lives as long as the page, so its handler does too.
    on_key.forget();
    let target = Rc::clone(app);
    let on_blur = Closure::<dyn FnMut()>::new(move || {
        if let Ok(mut app) = target.try_borrow_mut()
            && app.mass_barter
        {
            if let Some(details) = &mut app.details {
                details.flush_batch();
            }
            app.mass_barter = false;
            app.details_dirty = true;
        }
    });
    let _ = window.add_event_listener_with_callback("blur", on_blur.as_ref().unchecked_ref());
    on_blur.forget();
}

/// A click on `button` calls `handler` with whether Shift was held.
fn on_shift_click(
    app: &Rc<RefCell<App>>,
    button: &HtmlElement,
    handler: impl Fn(&mut App, bool) + 'static,
) {
    let target = Rc::clone(app);
    let on_click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if let Ok(mut app) = target.try_borrow_mut() {
            handler(&mut app, event.shift_key());
        }
    });
    button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
    // The page's buttons live as long as it does, so their handlers do too.
    on_click.forget();
}

/// The Menu and its Resign and Exit, which ask first as the session's do.
fn wire_game_menu(app: &Rc<RefCell<App>>) {
    let Some((open, pause, resign, exit)) = app.borrow().game_menu.as_ref().map(|menu| {
        let (open, _, pause, resign, exit) = menu.buttons();
        (open.clone(), pause.clone(), resign.clone(), exit.clone())
    }) else {
        return;
    };
    let target = Rc::clone(app);
    on_click(&open, move || {
        if let Some(menu) = &target.borrow().game_menu {
            menu.toggle();
        }
    });
    // Captured, as the view's own handlers may stop the press.
    let target = Rc::clone(app);
    let on_press = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        if let Some(menu) = &target.borrow().game_menu {
            let node = event
                .target()
                .and_then(|node| node.dyn_into::<web_sys::Node>().ok());
            menu.close_unless_on(node.as_ref());
        }
    });
    if let Some(window) = web_sys::window() {
        let _ = window.add_event_listener_with_callback_and_bool(
            "pointerdown",
            on_press.as_ref().unchecked_ref(),
            true,
        );
    }
    on_press.forget();
    let target = Rc::clone(app);
    on_click(&pause, move || {
        if let Some(menu) = &target.borrow().game_menu {
            menu.close();
        }
        toggle_pause(&target);
    });
    for (button, resign) in [(resign, true), (exit, false)] {
        let target = Rc::clone(app);
        on_click(&button, move || {
            let app = &mut *target.borrow_mut();
            if let Some(menu) = &app.game_menu {
                menu.close();
            }
            // `closeOpenDialogs`.
            if let Some(diplomacy_window) = &mut app.diplomacy_window {
                diplomacy_window.set_open(false);
            }
            if let Some(trade_window) = &mut app.trade_window {
                trade_window.set_open(false, app.perspective);
            }
            if let Some(message_box) = &mut app.message_box {
                if resign {
                    message_box.confirm_resign();
                } else {
                    message_box.confirm_exit();
                }
            }
        });
    }
}

/// `PauseControl.setPaused(!explicitPause, true)`: asks the server to pause
/// the game for the viewer's seat, or to resume it if the seat has paused it.
fn toggle_pause(app: &Rc<RefCell<App>>) {
    let Ok(app) = app.try_borrow() else {
        return;
    };
    if app.owner_seat.is_none()
        || !seated()
        || app.finish_noticed
        || app.match_over.ended
        || app.game_menu.is_none()
    {
        return;
    }
    let action = if app.own_pause() { "resume" } else { "pause" };
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = game_menu::post(action).await {
            web_sys::console::error_1(&error.into());
        }
    });
}

/// The Pause and Shift+Space hotkeys, and a press on the veil, which
/// resumes a game the viewer's seat paused (`PauseOverlay.onPress`).
fn wire_pause(app: &Rc<RefCell<App>>, window: &web_sys::Window) {
    if app.borrow().game_menu.is_none() {
        return;
    }
    let target = Rc::clone(app);
    let on_key = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let text = |object: &JsValue, name: &str| {
            js_sys::Reflect::get(object, &name.into())
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_default()
        };
        let flag = |name: &str| {
            js_sys::Reflect::get(&event, &name.into()).is_ok_and(|value| value.is_truthy())
        };
        let tag = event
            .target()
            .map(|element| text(&element, "tagName"))
            .unwrap_or_default();
        if matches!(tag.as_str(), "INPUT" | "SELECT" | "TEXTAREA")
            || flag("ctrlKey")
            || flag("altKey")
            || flag("metaKey")
            || flag("repeat")
        {
            return;
        }
        let pressed = match text(&event, "code").as_str() {
            "Pause" => !flag("shiftKey"),
            "Space" => flag("shiftKey"),
            _ => false,
        };
        if !pressed {
            return;
        }
        // A focused button would also take the Space as a click.
        let _ = js_sys::Reflect::get(&event, &"preventDefault".into())
            .ok()
            .and_then(|function| function.dyn_into::<js_sys::Function>().ok())
            .map(|function| function.call0(&event));
        toggle_pause(&target);
    });
    let _ = window.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
    on_key.forget();
    let root = app
        .borrow()
        .pause_overlay
        .as_ref()
        .map(|overlay| overlay.root().clone());
    if let Some(root) = root {
        let target = Rc::clone(app);
        on_click(&root, move || {
            let own = target.try_borrow().is_ok_and(|app| app.own_pause());
            if own {
                toggle_pause(&target);
            }
        });
    }
}

/// Where the summary window's numbers come from: the seat's own copies on
/// `/play`, else the match on screen, at the playhead when that is behind
/// the live edge.
fn summary_source(app: &App) -> Option<Source> {
    let turn = (!app.live).then_some(app.playhead);
    if let Some(seat) = app.owner_seat {
        return Some(Source::Play {
            seat,
            running: !app.match_over.ended,
            turn,
        });
    }
    Some(Source::Stream {
        match_id: app.scene.match_id,
        turn,
    })
}

fn open_summary(summary: &Rc<RefCell<SummaryWindow>>, source: Source, playhead: u32) {
    if let Ok(mut window) = summary.try_borrow_mut() {
        window.prepare(&source);
        window.set_open(true);
    }
    summary::refresh(summary, source, playhead);
}

/// Closes the summary; one opened from the result offer goes back to it.
fn close_summary(app: &Rc<RefCell<App>>) {
    let Ok(mut app) = app.try_borrow_mut() else {
        return;
    };
    let Some(summary) = app.summary.clone() else {
        return;
    };
    if let Ok(mut summary) = summary.try_borrow_mut() {
        summary.set_open(false);
    }
    if let Some(won) = app.offer_return.take()
        && !app.match_over.ended
        && let Some(message_box) = &mut app.message_box
    {
        message_box.offer_result(won);
    }
}

/// The buttons that open the summary (`#summary-open` in the controls, the
/// game menu's entry, the match over popup's), its Close, and Escape. It
/// changes nothing of the match: closing it never ends or restarts it.
fn wire_summary(app: &Rc<RefCell<App>>, window: &web_sys::Window) {
    let Some(summary) = app.borrow().summary.clone() else {
        return;
    };
    let open = |app: &Rc<RefCell<App>>| {
        let (summary, source, playhead) = {
            let app = &mut *app.borrow_mut();
            // A summary opened by hand has no offer to return to, unless
            // it is opened under a pending offer.
            if !app.message_box.as_ref().is_some_and(MessageBox::is_offer) {
                app.offer_return = None;
            }
            if let Some(diplomacy_window) = &mut app.diplomacy_window {
                diplomacy_window.set_open(false);
            }
            if let Some(trade_window) = &mut app.trade_window {
                trade_window.set_open(false, app.perspective);
            }
            (app.summary.clone(), summary_source(app), app.playhead)
        };
        if let (Some(summary), Some(source)) = (summary, source) {
            open_summary(&summary, source, playhead);
        }
    };
    let mut buttons: Vec<HtmlElement> = ["summary-open", "match-over-summary"]
        .into_iter()
        .filter_map(|id| {
            window
                .document()
                .and_then(|document| element(&document, id).ok())
        })
        .collect();
    if let Some(menu) = &app.borrow().game_menu {
        buttons.push(menu.buttons().1.clone());
    }
    for button in buttons {
        let target = Rc::clone(app);
        let from_menu = button.id() == "game-menu-summary";
        on_click(&button, move || {
            if from_menu && let Some(menu) = &target.borrow().game_menu {
                menu.close();
            }
            open(&target);
        });
    }
    let close = summary.borrow().close_button().clone();
    let target = Rc::clone(app);
    on_click(&close, move || close_summary(&target));
    let target = Rc::clone(app);
    let on_key = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let code = js_sys::Reflect::get(&event, &"code".into())
            .ok()
            .and_then(|value| value.as_string());
        if code.as_deref() != Some("Escape") {
            return;
        }
        let open = target
            .try_borrow()
            .ok()
            .and_then(|app| app.summary.clone())
            .is_some_and(|summary| summary.try_borrow().is_ok_and(|summary| summary.is_open()));
        if open {
            close_summary(&target);
        }
    });
    let _ = window.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
    on_key.forget();
}

/// No, or Escape, on the open message box; Continue watching of the result
/// offer goes on to the page a defeat leads to.
fn cancel_message_box(app: &Rc<RefCell<App>>) {
    let Ok(mut app) = app.try_borrow_mut() else {
        return;
    };
    if let Some(Pending::Result { won: false }) =
        app.message_box.as_mut().and_then(MessageBox::cancel)
    {
        game_menu::after_defeat();
    }
}

/// The message box's No and Yes, and Escape, its cancel hotkey.
fn wire_message_box(app: &Rc<RefCell<App>>, window: &web_sys::Window) {
    let Some((no, yes)) = app.borrow().message_box.as_ref().map(|message_box| {
        let (no, yes) = message_box.buttons();
        (no.clone(), yes.clone())
    }) else {
        return;
    };
    let target = Rc::clone(app);
    on_click(&no, move || cancel_message_box(&target));
    let target = Rc::clone(app);
    on_click(&yes, move || {
        let mut app = target.borrow_mut();
        match app.message_box.as_mut().and_then(MessageBox::accept) {
            Some(Pending::Delete(entities)) => {
                if let Some(uplink) = &app.uplink {
                    uplink.send(&Command::Delete { entities });
                }
            }
            // The stream reports the defeat, which moves the page on.
            Some(Pending::Resign) => wasm_bindgen_futures::spawn_local(async {
                if let Err(error) = game_menu::post("resign").await {
                    web_sys::console::error_1(&error.into());
                }
            }),
            // Reloaded either way: a game that had already ended shows its
            // end on the seat's stream, sooner than the observers' delay.
            Some(Pending::Exit) => wasm_bindgen_futures::spawn_local(async {
                if let Err(error) = game_menu::post("end").await {
                    web_sys::console::error_1(&error.into());
                }
                game_menu::reload();
            }),
            Some(Pending::Result { won }) => {
                app.offer_return = Some(won);
                let source = summary_source(&app);
                if let (Some(summary), Some(source)) = (app.summary.clone(), source) {
                    let playhead = app.playhead;
                    drop(app);
                    open_summary(&summary, source, playhead);
                } else {
                    drop(app);
                    // Never strand the player when the window cannot open.
                    if !won {
                        game_menu::after_defeat();
                    }
                }
            }
            None => {}
        }
    });
    let target = Rc::clone(app);
    let on_key = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let code = js_sys::Reflect::get(&event, &"code".into())
            .ok()
            .and_then(|value| value.as_string());
        if code.as_deref() != Some("Escape") {
            return;
        }
        let open = target
            .try_borrow()
            .is_ok_and(|app| app.message_box.as_ref().is_some_and(MessageBox::is_open));
        if open {
            cancel_message_box(&target);
        }
    });
    let _ = window.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
    // The box lives as long as the page, so its handler does too.
    on_key.forget();
}

/// The command row's bound hotkeys (`hotkeys/misc.xml`, the engine's
/// default keys): session.unload, unloadturrets and leaveturret (U, all
/// three), session.backtowork (Y), session.stop (H) and session.kill
/// (Delete, Backspace), which asks first but with Shift. Each acts as its
/// button would, so only for the commands the viewer may use.
fn wire_command_hotkeys(app: &Rc<RefCell<App>>, window: &web_sys::Window) {
    let target = Rc::clone(app);
    let on_key = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let text = |object: &JsValue, name: &str| {
            js_sys::Reflect::get(object, &name.into())
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_default()
        };
        let flag = |name: &str| {
            js_sys::Reflect::get(&event, &name.into()).is_ok_and(|value| value.is_truthy())
        };
        let tag = event
            .target()
            .map(|element| text(&element, "tagName"))
            .unwrap_or_default();
        if matches!(tag.as_str(), "INPUT" | "SELECT" | "TEXTAREA")
            || flag("ctrlKey")
            || flag("altKey")
            || flag("metaKey")
            || flag("repeat")
        {
            return;
        }
        let names: &[&str] = match text(&event, "code").as_str() {
            "KeyU" => &["unload-all", "unload-all-turrets", "leave-turret"],
            "KeyY" => &["back-to-work"],
            "KeyH" => &["stop"],
            "Delete" | "Backspace" => &["delete"],
            _ => return,
        };
        let Ok(app) = target.try_borrow() else {
            return;
        };
        if app.message_box.as_ref().is_some_and(MessageBox::is_open) {
            return;
        }
        if let Some(details) = &app.details {
            for name in names {
                details.command_hotkey(name, flag("shiftKey"));
            }
        }
    });
    let _ = window.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
    // The window lives as long as the page, so its handler does too.
    on_key.forget();
}

/// The idle hotkeys (`hotkeys/misc.xml` and the idle worker button's, the
/// engine's default keys): selection.idleworker (Period, NumDecimal),
/// idlewarrior (Slash, NumDivide), idleunit (BackSlash) and idlebuilder
/// (Semicolon), with Shift as selection.add and Alt as selection.offscreen.
/// Repeats count, as the engine sends a held hotkey's KeyDown again.
fn wire_idle(app: &Rc<RefCell<App>>, window: &web_sys::Window) {
    if app.borrow().uplink.is_none() {
        return;
    }
    let target = Rc::clone(app);
    let on_key = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let text = |object: &JsValue, name: &str| {
            js_sys::Reflect::get(object, &name.into())
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_default()
        };
        let flag = |name: &str| {
            js_sys::Reflect::get(&event, &name.into()).is_ok_and(|value| value.is_truthy())
        };
        let tag = event
            .target()
            .map(|element| text(&element, "tagName"))
            .unwrap_or_default();
        if matches!(tag.as_str(), "INPUT" | "SELECT" | "TEXTAREA")
            || flag("ctrlKey")
            || flag("metaKey")
        {
            return;
        }
        let classes = match text(&event, "code").as_str() {
            "Period" | "NumpadDecimal" => idle::WORKERS,
            "Slash" | "NumpadDivide" => idle::WARRIORS,
            "Backslash" => idle::UNITS,
            "Semicolon" => idle::BUILDERS,
            _ => return,
        };
        // Firefox's quick find, and Alt's menu bar.
        event.prevent_default();
        let Ok(mut app) = target.try_borrow_mut() else {
            return;
        };
        if app.message_box.as_ref().is_some_and(MessageBox::is_open) {
            return;
        }
        app.find_idle(classes, flag("shiftKey"), flag("altKey"));
    });
    let _ = window.add_event_listener_with_callback("keydown", on_key.as_ref().unchecked_ref());
    // The window lives as long as the page, so its handler does too.
    on_key.forget();
    let Some(button) = app
        .borrow()
        .idle_worker
        .as_ref()
        .map(|idle_worker| idle_worker.button.clone())
    else {
        return;
    };
    let target = Rc::clone(app);
    let on_click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if let Ok(mut app) = target.try_borrow_mut() {
            app.find_idle(idle::WORKERS, event.shift_key(), event.alt_key());
        }
    });
    button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
    on_click.forget();
}

fn wire_research(app: &Rc<RefCell<App>>) {
    let buttons = {
        let app = app.borrow();
        let Some(research) = &app.research else {
            return;
        };
        research.buttons().cloned().collect::<Vec<_>>()
    };
    for (index, button) in buttons.into_iter().enumerate() {
        let target = Rc::clone(app);
        let on_click = Closure::<dyn FnMut()>::new(move || {
            if let Some(research) = &target.borrow().research {
                research.clicked(index);
            }
        });
        button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
        // The overlay lives as long as the page, so its handlers do too.
        on_click.forget();
    }
}

fn wire_top(app: &Rc<RefCell<App>>) {
    let buttons = {
        let app = app.borrow();
        let Some(top) = &app.top else {
            return;
        };
        top.hero_buttons().cloned().collect::<Vec<_>>()
    };
    for (index, button) in buttons.into_iter().enumerate() {
        let target = Rc::clone(app);
        let on_click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            if let Some(top) = &target.borrow().top {
                top.hero_clicked(index, event.shift_key());
            }
        });
        button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
        // The panel lives as long as the page, so its handlers do too.
        on_click.forget();
        let target = Rc::clone(app);
        let on_double = Closure::<dyn FnMut()>::new(move || {
            if let Some(top) = &target.borrow().top {
                top.hero_double_clicked(index);
            }
        });
        button.set_ondblclick(Some(on_double.as_ref().unchecked_ref()));
        on_double.forget();
    }
}

/// The controls live as long as the page, so their handlers do too: the
/// closure is leaked rather than dropped with this call.
fn on_change(target: &HtmlElement, handler: impl FnMut() + 'static) {
    let closure = Closure::<dyn FnMut()>::new(handler);
    target.set_onchange(Some(closure.as_ref().unchecked_ref()));
    closure.forget();
}

/// As `on_change`.
fn on_click(target: &HtmlElement, handler: impl FnMut() + 'static) {
    let closure = Closure::<dyn FnMut()>::new(handler);
    target.set_onclick(Some(closure.as_ref().unchecked_ref()));
    closure.forget();
}

/// As `on_change`.
fn on_input(target: &HtmlElement, handler: impl FnMut() + 'static) {
    let closure = Closure::<dyn FnMut()>::new(handler);
    target.set_oninput(Some(closure.as_ref().unchecked_ref()));
    closure.forget();
}

fn wire_controls(app: &Rc<RefCell<App>>) {
    let (button, select) = {
        let app = app.borrow();
        (app.diplomacy_button.clone(), app.perspective_select.clone())
    };

    let target = Rc::clone(app);
    on_click(&button, move || {
        let app = &mut *target.borrow_mut();
        app.diplomacy_colors = !app.diplomacy_colors;
        app.show_controls();
        app.update_palette();
    });

    let target = Rc::clone(app);
    on_change(&select, move || {
        let app = &mut *target.borrow_mut();
        app.perspective = match app.perspective_select.value().parse() {
            Ok(0) | Err(_) => Perspective::Observer,
            Ok(id) => Perspective::Player(id),
        };
        app.update_palette();
    });

    app.borrow().show_controls();
}

/// The button and the P key, which the game itself leaves free.
/// The projectiles list, kept across visits; on unless turned off, as the
/// game draws them.
fn wire_projectiles(app: &Rc<RefCell<App>>, select: HtmlSelectElement) {
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(PROJECTILES_SETTINGS_KEY).ok().flatten())
        .is_none_or(|text| text != "0");
    select.set_value(if saved { "1" } else { "0" });
    app.borrow_mut().renderer.set_projectiles(saved);

    let target = Rc::clone(app);
    let list = select.clone();
    on_change(&select, move || {
        let on = list.value() == "1";
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(PROJECTILES_SETTINGS_KEY, if on { "1" } else { "0" });
        }
        let app = &mut *target.borrow_mut();
        app.renderer.set_projectiles(on);
        app.dirty = true;
    });
}

/// The detailed tooltips list, kept across visits; off unless turned on,
/// as the engine's default.cfg.
fn wire_detailed_tooltips(app: &Rc<RefCell<App>>, select: HtmlSelectElement) {
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| {
            storage
                .get_item(DETAILED_TOOLTIPS_SETTINGS_KEY)
                .ok()
                .flatten()
        })
        .is_some_and(|text| text == "1");
    select.set_value(if saved { "1" } else { "0" });
    app.borrow_mut().detailed_tooltips = saved;

    let target = Rc::clone(app);
    let list = select.clone();
    on_change(&select, move || {
        let on = list.value() == "1";
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(DETAILED_TOOLTIPS_SETTINGS_KEY, if on { "1" } else { "0" });
        }
        let app = &mut *target.borrow_mut();
        app.detailed_tooltips = on;
        app.details_dirty = true;
    });
}

/// The status bars list, kept across visits; off unless turned on, as the
/// game starts.
fn wire_status_bars(app: &Rc<RefCell<App>>, select: HtmlSelectElement) {
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(STATUS_BARS_SETTINGS_KEY).ok().flatten())
        .is_some_and(|text| text == "1");
    select.set_value(if saved { "1" } else { "0" });
    {
        let app = &mut *app.borrow_mut();
        app.all_status_bars = saved;
        app.update_palette();
    }

    let target = Rc::clone(app);
    let list = select.clone();
    on_change(&select, move || {
        let on = list.value() == "1";
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(STATUS_BARS_SETTINGS_KEY, if on { "1" } else { "0" });
        }
        let app = &mut *target.borrow_mut();
        app.all_status_bars = on;
        app.update_palette();
        app.dirty = true;
    });
}

/// The range overlay lists, kept across visits; on unless turned off, as the
/// game's defaults. Alt+C, Alt+V and Alt+B change them too, which
/// `RangeControls::show` brings back to the lists each frame.
fn wire_ranges(app: &Rc<RefCell<App>>, selects: [HtmlSelectElement; 3]) {
    let storage = web_sys::window().and_then(|window| window.local_storage().ok().flatten());
    let saved = RANGES_SETTINGS_KEYS.map(|key| {
        storage
            .as_ref()
            .and_then(|storage| storage.get_item(key).ok().flatten())
            .is_none_or(|text| text != "0")
    });
    for (select, on) in selects.iter().zip(saved) {
        select.set_value(if on { "1" } else { "0" });
    }
    {
        let app = &mut *app.borrow_mut();
        app.renderer.set_ranges(saved);
        app.ranges = Some(RangeControls {
            selects: selects.clone(),
            shown: saved,
        });
    }

    for (kind, select) in selects.into_iter().enumerate() {
        let target = Rc::clone(app);
        let list = select.clone();
        on_change(&select, move || {
            let app = &mut *target.borrow_mut();
            let mut ranges = app.renderer.ranges();
            ranges[kind] = list.value() == "1";
            app.renderer.set_ranges(ranges);
            if let Some(controls) = &mut app.ranges {
                controls.show(ranges);
            }
            app.dirty = true;
        });
    }
}

/// The language list: the browser's own order by default, or the one the
/// menu names, kept across visits. The names the engine's text is looked
/// up in change with it, so the panels are made again.
fn wire_language(app: &Rc<RefCell<App>>, select: HtmlSelectElement) {
    let storage = web_sys::window().and_then(|window| window.local_storage().ok().flatten());
    let saved = storage
        .as_ref()
        .and_then(|storage| storage.get_item(LANGUAGE_SETTINGS_KEY).ok().flatten())
        .unwrap_or_default();
    let app = Rc::clone(app);
    wasm_bindgen_futures::spawn_local(async move {
        let languages = Rc::new(lang::languages().await);
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        for language in languages.iter() {
            let Some(option) = document
                .create_element("option")
                .ok()
                .and_then(|option| option.dyn_into::<HtmlOptionElement>().ok())
            else {
                continue;
            };
            option.set_value(&language.code);
            option.set_text(&language.name);
            let _ = select.append_child(&option);
        }
        let default_name = lang::choose(&languages, "")
            .map_or("English".to_string(), |language| language.name.clone());
        let apply = {
            let app = Rc::clone(&app);
            let languages = Rc::clone(&languages);
            let select = select.clone();
            move |setting: String| {
                let app = Rc::clone(&app);
                let languages = Rc::clone(&languages);
                let select = select.clone();
                let default_name = default_name.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let chosen = lang::choose(&languages, &setting);
                    if lang::load(chosen).await {
                        // The list's first entry names the browser's pick in
                        // the language just loaded.
                        if let Some(first) = select
                            .item(0)
                            .and_then(|option| option.dyn_into::<HtmlOptionElement>().ok())
                        {
                            first.set_text(&lang::format(
                                &lang::translate("Default (browser: %(name)s)"),
                                &[("name", &default_name)],
                            ));
                        }
                    }
                    if let Ok(mut app) = app.try_borrow_mut() {
                        app.details_dirty = true;
                        app.top_dirty = true;
                        if let Some(diplomacy_window) = &mut app.diplomacy_window {
                            diplomacy_window.invalidate();
                        }
                        if let Some(summary) = &app.summary
                            && let Ok(mut summary) = summary.try_borrow_mut()
                        {
                            summary.invalidate();
                        }
                        if let Some(trade_window) = &mut app.trade_window {
                            trade_window.invalidate();
                        }
                        app.dirty = true;
                    }
                });
            }
        };
        // A saved code the list no longer has is the default.
        let known = languages.iter().any(|language| language.code == saved);
        select.set_value(if known { &saved } else { "" });
        apply(select.value());
        let list = select.clone();
        on_change(&select, move || {
            let setting = list.value();
            if let Some(storage) =
                web_sys::window().and_then(|window| window.local_storage().ok().flatten())
            {
                let _ = storage.set_item(LANGUAGE_SETTINGS_KEY, &setting);
            }
            apply(setting);
        });
    });
}

/// The texture quality list, kept across visits. A device that is not
/// given the higher qualities has them disabled.
fn wire_textures(app: &Rc<RefCell<App>>, select: HtmlSelectElement) {
    let max = app.borrow().renderer.max_texture_tiers();
    for at in 0..select.length() {
        if let Some(option) = select
            .item(at)
            .and_then(|option| option.dyn_into::<HtmlOptionElement>().ok())
            && option
                .value()
                .parse::<usize>()
                .is_ok_and(|tiers| tiers > max)
        {
            option.set_disabled(true);
        }
    }
    let parse = move |text: &str| text.parse::<usize>().ok().filter(|tiers| *tiers >= 1);
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(TEXTURE_SETTINGS_KEY).ok().flatten())
        .and_then(|text| parse(&text))
        .unwrap_or(TEXTURE_TIER_SIDES.len())
        .min(max);
    select.set_value(&saved.to_string());
    app.borrow_mut().renderer.set_texture_tiers(saved);

    let target = Rc::clone(app);
    let list = select.clone();
    on_change(&select, move || {
        let tiers = parse(&list.value()).unwrap_or(max).min(max);
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(TEXTURE_SETTINGS_KEY, &tiers.to_string());
        }
        let app = &mut *target.borrow_mut();
        app.renderer.set_texture_tiers(tiers);
        app.dirty = true;
    });
}

/// The material maps list, kept across visits; off unless chosen, as
/// the maps cost frame time and download.
fn wire_maps(app: &Rc<RefCell<App>>, select: HtmlSelectElement) {
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(MAP_SETTINGS_KEY).ok().flatten())
        .is_some_and(|text| text == "1");
    select.set_value(if saved { "1" } else { "0" });
    app.borrow_mut().renderer.set_material_maps(saved);

    let target = Rc::clone(app);
    let list = select.clone();
    on_change(&select, move || {
        let on = list.value() == "1";
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(MAP_SETTINGS_KEY, if on { "1" } else { "0" });
        }
        let app = &mut *target.borrow_mut();
        app.renderer.set_material_maps(on);
        app.dirty = true;
    });
}

/// An off/on list, kept across visits under `key`; off unless chosen, as
/// the game's own defaults for these are. `dependents` are the lists that
/// only apply while it is on, disabled while it is off, as the game's
/// options page does.
fn wire_switch(
    app: &Rc<RefCell<App>>,
    select: HtmlSelectElement,
    key: &'static str,
    apply: fn(&mut dyn View, bool),
    dependents: Vec<HtmlSelectElement>,
) {
    wire_switch_default(app, select, key, apply, dependents, false);
}

/// `wire_switch` for a list that is on, not off, until the visitor chooses.
fn wire_switch_default(
    app: &Rc<RefCell<App>>,
    select: HtmlSelectElement,
    key: &'static str,
    apply: fn(&mut dyn View, bool),
    dependents: Vec<HtmlSelectElement>,
    default: bool,
) {
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(key).ok().flatten())
        .map_or(default, |text| text == "1");
    select.set_value(if saved { "1" } else { "0" });
    apply(&mut *app.borrow_mut().renderer, saved);
    for dependent in &dependents {
        dependent.set_disabled(!saved);
    }

    let target = Rc::clone(app);
    let list = select.clone();
    on_change(&select, move || {
        let on = list.value() == "1";
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(key, if on { "1" } else { "0" });
        }
        for dependent in &dependents {
            dependent.set_disabled(!on);
        }
        let app = &mut *target.borrow_mut();
        apply(&mut *app.renderer, on);
        app.dirty = true;
    });
}

/// Disables `select` while any of `parents` is off, as the game's options
/// page does for an option with several dependencies.
fn wire_depends_on_all(select: &HtmlSelectElement, parents: Vec<HtmlSelectElement>) {
    let update = {
        let select = select.clone();
        let parents = parents.clone();
        move || select.set_disabled(parents.iter().any(|parent| parent.value() != "1"))
    };
    update();
    let on_change = Closure::<dyn FnMut()>::new(update);
    for parent in &parents {
        let _ =
            parent.add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref());
    }
    // The controls live as long as the page, so their handlers do too.
    on_change.forget();
}

/// The triplanar list, kept across visits; on unless turned off, as it
/// costs frame time only on the few terrains that use it.
fn wire_triplanar(app: &Rc<RefCell<App>>, select: HtmlSelectElement) {
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(TRIPLANAR_SETTINGS_KEY).ok().flatten())
        .is_none_or(|text| text != "0");
    select.set_value(if saved { "1" } else { "0" });
    app.borrow_mut().renderer.set_triplanar(saved);

    let target = Rc::clone(app);
    let list = select.clone();
    on_change(&select, move || {
        let on = list.value() == "1";
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(TRIPLANAR_SETTINGS_KEY, if on { "1" } else { "0" });
        }
        let app = &mut *target.borrow_mut();
        app.renderer.set_triplanar(on);
        app.dirty = true;
    });
}

/// The wind list, kept across visits; on unless turned off, as the game
/// always sways at its highest material quality.
fn wire_wind(app: &Rc<RefCell<App>>, select: HtmlSelectElement) {
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(WIND_SETTINGS_KEY).ok().flatten())
        .is_none_or(|text| text != "0");
    select.set_value(if saved { "1" } else { "0" });
    app.borrow_mut().renderer.set_wind(saved);

    let target = Rc::clone(app);
    let list = select.clone();
    on_change(&select, move || {
        let on = list.value() == "1";
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(WIND_SETTINGS_KEY, if on { "1" } else { "0" });
        }
        let app = &mut *target.borrow_mut();
        app.renderer.set_wind(on);
        app.dirty = true;
    });
}

/// The shadow list, kept across visits.
fn wire_shadows(app: &Rc<RefCell<App>>, select: HtmlSelectElement) {
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(SHADOW_SETTINGS_KEY).ok().flatten())
        .and_then(|text| text.parse::<u32>().ok())
        .filter(|size| shadow::SIZES.contains(size))
        .unwrap_or(shadow::DEFAULT_SIZE);
    select.set_value(&saved.to_string());
    app.borrow_mut().renderer.set_shadows(saved);

    let target = Rc::clone(app);
    let list = select.clone();
    on_change(&select, move || {
        let size = list
            .value()
            .parse::<u32>()
            .ok()
            .filter(|size| shadow::SIZES.contains(size))
            .unwrap_or(shadow::DEFAULT_SIZE);
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(SHADOW_SETTINGS_KEY, &size.to_string());
        }
        let app = &mut *target.borrow_mut();
        app.renderer.set_shadows(size);
        app.dirty = true;
    });
}

/// The resolution scale and upscale technique lists, kept across visits;
/// the technique is disabled unless post-processing is on and the scale
/// below 1, as in the game's options.
fn wire_render_scale(
    app: &Rc<RefCell<App>>,
    scale: HtmlSelectElement,
    upscale: HtmlSelectElement,
    postproc: HtmlSelectElement,
) {
    wire_list(
        app,
        &scale,
        RENDER_SCALE_SETTINGS_KEY,
        "1",
        |view, value| {
            view.set_render_scale(value.parse().unwrap_or(1.0));
        },
    );
    wire_list(app, &upscale, UPSCALE_SETTINGS_KEY, "fsr", |view, value| {
        view.set_upscale(match value {
            "bilinear" => Upscale::Bilinear,
            "pixelated" => Upscale::Pixelated,
            _ => Upscale::Fsr,
        });
    });
    let update = {
        let scale = scale.clone();
        let postproc = postproc.clone();
        move || {
            let below = scale.value().parse::<f32>().is_ok_and(|scale| scale < 1.0);
            upscale.set_disabled(postproc.value() != "1" || !below);
        }
    };
    update();
    let on_change = Closure::<dyn FnMut()>::new(update);
    for parent in [&postproc, &scale] {
        let _ =
            parent.add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref());
    }
    // The controls live as long as the page, so their handlers do too.
    on_change.forget();
}

/// A list kept across visits under `key`, `default` until chosen or when
/// the saved value is no longer offered.
fn wire_list(
    app: &Rc<RefCell<App>>,
    select: &HtmlSelectElement,
    key: &'static str,
    default: &str,
    apply: fn(&mut dyn View, &str),
) {
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(key).ok().flatten());
    select.set_value(saved.as_deref().unwrap_or(default));
    // An unknown value leaves the list with none selected.
    if select.value().is_empty() {
        select.set_value(default);
    }
    apply(&mut *app.borrow_mut().renderer, &select.value());

    let target = Rc::clone(app);
    let list = select.clone();
    on_change(select, move || {
        let value = list.value();
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(key, &value);
        }
        let app = &mut *target.borrow_mut();
        apply(&mut *app.renderer, &value);
        app.dirty = true;
    });
}

/// The shadow cutoff slider, kept across visits, applied as it is
/// dragged; disabled while shadows are off, as in the game's options.
fn wire_shadow_cutoff(
    app: &Rc<RefCell<App>>,
    slider: HtmlInputElement,
    readout: HtmlElement,
    shadows: &HtmlSelectElement,
) {
    let in_range = |metres: &f32| (shadow::MIN_CUTOFF..=shadow::MAX_CUTOFF).contains(metres);
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(SHADOW_CUTOFF_SETTINGS_KEY).ok().flatten())
        .and_then(|text| text.parse::<f32>().ok())
        .filter(in_range)
        .unwrap_or(shadow::DEFAULT_CUTOFF);
    slider.set_value(&saved.to_string());
    readout.set_text_content(Some(&format!("{saved} m")));
    app.borrow_mut().renderer.set_shadow_cutoff(saved);

    let target = Rc::clone(app);
    let input = slider.clone();
    on_input(&slider, move || {
        let metres = input
            .value()
            .parse::<f32>()
            .ok()
            .filter(in_range)
            .unwrap_or(shadow::DEFAULT_CUTOFF);
        readout.set_text_content(Some(&format!("{metres} m")));
        let app = &mut *target.borrow_mut();
        app.renderer.set_shadow_cutoff(metres);
        app.dirty = true;
    });

    // Saved on release only, so a drag does not write storage per step.
    let input = slider.clone();
    on_change(&slider, move || {
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(SHADOW_CUTOFF_SETTINGS_KEY, &input.value());
        }
    });

    let update = {
        let shadows = shadows.clone();
        move || slider.set_disabled(shadows.value() == "0")
    };
    update();
    let on_shadows = Closure::<dyn FnMut()>::new(update);
    let _ = shadows.add_event_listener_with_callback("change", on_shadows.as_ref().unchecked_ref());
    on_shadows.forget();
}

/// The sharpness slider, kept across visits; usable only while all of
/// `parents` are on, as the game's slider depends on post-processing and
/// sharpening.
fn wire_sharpness(
    app: &Rc<RefCell<App>>,
    slider: HtmlInputElement,
    readout: HtmlElement,
    parents: Vec<HtmlSelectElement>,
) {
    let in_range = |sharpness: &f32| (0.0..=1.0).contains(sharpness);
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(SHARPNESS_SETTINGS_KEY).ok().flatten())
        .and_then(|text| text.parse::<f32>().ok())
        .filter(in_range)
        .unwrap_or(postproc::DEFAULT_SHARPNESS);
    slider.set_value(&saved.to_string());
    readout.set_text_content(Some(&format!("{saved:.2}")));
    app.borrow_mut().renderer.set_sharpness(saved);

    let target = Rc::clone(app);
    let input = slider.clone();
    on_input(&slider, move || {
        let sharpness = input
            .value()
            .parse::<f32>()
            .ok()
            .filter(in_range)
            .unwrap_or(postproc::DEFAULT_SHARPNESS);
        readout.set_text_content(Some(&format!("{sharpness:.2}")));
        let app = &mut *target.borrow_mut();
        app.renderer.set_sharpness(sharpness);
        app.dirty = true;
    });

    // Saved on release only, so a drag does not write storage per step.
    let input = slider.clone();
    on_change(&slider, move || {
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(SHARPNESS_SETTINGS_KEY, &input.value());
        }
    });

    let update = {
        let parents = parents.clone();
        move || slider.set_disabled(parents.iter().any(|parent| parent.value() != "1"))
    };
    update();
    let on_parent = Closure::<dyn FnMut()>::new(update);
    for parent in &parents {
        let _ =
            parent.add_event_listener_with_callback("change", on_parent.as_ref().unchecked_ref());
    }
    on_parent.forget();
}

fn wire_warmup(app: &Rc<RefCell<App>>) {
    let Some(button) = app
        .borrow()
        .warmup
        .as_ref()
        .map(|warmup| warmup.button().clone())
    else {
        return;
    };
    let target = Rc::clone(app);
    on_click(&button, move || {
        let app = &mut *target.borrow_mut();
        if app.renderer.asset_store().is_some() && app.warmup.is_some() {
            app.start_warmup();
            stored::persist();
        }
    });
}

/// Explains what the page stores and what Warm up is for, on the first
/// visit and then at most once a week.
fn wire_storage_notice(app: &Rc<RefCell<App>>, document: &Document) -> Result<(), String> {
    let notice: HtmlElement = element(document, "storage-notice")?;
    let ok: HtmlButtonElement = element(document, "storage-notice-ok")?;
    let warm: HtmlButtonElement = element(document, "storage-notice-warmup")?;
    let storage = web_sys::window().and_then(|window| window.local_storage().ok().flatten());
    let now = js_sys::Date::now();
    let shown_at = storage
        .as_ref()
        .and_then(|storage| storage.get_item(STORAGE_NOTICE_KEY).ok().flatten())
        .and_then(|text| text.parse::<f64>().ok());
    if shown_at.is_some_and(|at| now - at < STORAGE_NOTICE_INTERVAL_MS) {
        return Ok(());
    }
    // Recorded when shown, not when answered, so a viewer who closes the
    // tab is not asked again on every visit.
    if let Some(storage) = &storage {
        let _ = storage.set_item(STORAGE_NOTICE_KEY, &now.to_string());
    }
    notice.set_hidden(false);
    // Minimal graphics on a first visit, so the page loads on any device;
    // the preset button's handler applies and saves each setting.
    if shown_at.is_none()
        && let Ok(min) = element::<HtmlButtonElement>(document, "preset-min")
    {
        min.click();
    }

    let popup = notice.clone();
    on_click(&ok, move || popup.set_hidden(true));

    let target = Rc::clone(app);
    on_click(&warm, move || {
        notice.set_hidden(true);
        if let Some(warmup) = &mut target.borrow_mut().warmup {
            warmup.request();
        }
        // Here rather than when the run starts: Firefox asks the viewer,
        // and only from a click.
        stored::persist();
    });
    Ok(())
}

fn wire_sound(app: &Rc<RefCell<App>>, window: &web_sys::Window) {
    let controls = {
        let app = app.borrow();
        let Some(sound) = &app.sound else {
            return;
        };
        sound.show();
        (
            sound.button.clone(),
            [
                sound.master.clone(),
                sound.effects.clone(),
                sound.music.clone(),
            ],
        )
    };
    let (button, sliders) = controls;

    let target = Rc::clone(app);
    on_click(&button, move || {
        if let Some(sound) = &mut target.borrow_mut().sound {
            let on = !sound.audio.settings().on;
            sound.changed(on);
        }
    });

    let target = Rc::clone(app);
    let on_input = Closure::<dyn FnMut()>::new(move || {
        if let Some(sound) = &mut target.borrow_mut().sound {
            let on = sound.audio.settings().on;
            sound.changed(on);
        }
    });
    for slider in &sliders {
        slider.set_oninput(Some(on_input.as_ref().unchecked_ref()));
    }
    on_input.forget();

    // Sound left on at the last visit can start only in a gesture.
    let target = Rc::clone(app);
    let on_gesture = Closure::<dyn FnMut()>::new(move || {
        if let Some(sound) = &mut target.borrow_mut().sound {
            sound.audio.gesture();
        }
    });
    for event in ["pointerdown", "keydown"] {
        let _ = window.add_event_listener_with_callback(event, on_gesture.as_ref().unchecked_ref());
    }
    on_gesture.forget();
}

fn load_sound_settings() -> Settings {
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(SOUND_SETTINGS_KEY).ok().flatten());
    let mut settings = Settings {
        on: false,
        master: DEFAULT_VOLUME,
        effects: DEFAULT_VOLUME,
        music: DEFAULT_VOLUME,
    };
    if let Some(saved) = saved {
        let fields: Vec<&str> = saved.split(',').collect();
        if let [on, master, effects, music] = fields[..] {
            let volume = |text: &str| {
                text.parse::<f32>()
                    .map_or(DEFAULT_VOLUME, |value| value.clamp(0.0, 1.0))
            };
            settings = Settings {
                on: on == "1",
                master: volume(master),
                effects: volume(effects),
                music: volume(music),
            };
        }
    }
    settings
}

/// Best effort: without storage the settings last only this visit.
fn save_sound_settings(settings: Settings) {
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let text = format!(
            "{},{},{},{}",
            u8::from(settings.on),
            settings.master,
            settings.effects,
            settings.music
        );
        let _ = storage.set_item(SOUND_SETTINGS_KEY, &text);
    }
}

/// `CCmpSoundManager::PlaySoundGroup`'s test for the displayed player:
/// `Some(owned)` when it hears the entity's sound. An observer sees all and
/// owns nothing.
fn hears(perspective: Perspective, owner: i32, visibility: u32) -> Option<bool> {
    match perspective {
        Perspective::Observer => Some(false),
        Perspective::Player(player) => perspective
            .hears(visibility)
            .then_some(usize::try_from(owner) == Ok(player)),
    }
}

/// Where a simulation sound is heard from, in metres, and whether the
/// displayed player owns it; `None` when that player does not hear it.
/// `PlaySoundGroupAtPosition` tests nothing, `PlaySoundGroupForPlayer`
/// plays at the origin and is owned only by its player.
fn heard_event(perspective: Perspective, sound: &Sound) -> Option<([f32; 3], bool)> {
    let metres =
        |x: i32, y: i32, z: i32| [x, y, z].map(|value| value as f32 / POSITION_UNITS_PER_METRE);
    match sound.target {
        SoundTarget::Entity {
            owner,
            x,
            z,
            y,
            visibility,
        } => hears(perspective, owner, visibility).map(|owned| (metres(x, y, z), owned)),
        SoundTarget::Position { x, z, y } => Some((metres(x, y, z), false)),
        SoundTarget::Player(player) => {
            let owned = matches!(perspective, Perspective::Player(shown) if usize::try_from(player) == Ok(shown));
            Some(([0.0; 3], owned))
        }
    }
}

/// What music.js would play: the shown player's civ, victory or defeat
/// once decided, battle while `BattleDetection` says so. The session skips
/// the battle poll for observers, who stay at peace on the defaults.
fn music_for(scene: &Scene, perspective: Perspective) -> (&str, MusicKind) {
    let Perspective::Player(id) = perspective else {
        return ("", MusicKind::Peace);
    };
    let Some(player) = scene.players.get(id) else {
        return ("", MusicKind::Peace);
    };
    let kind = match player.state {
        PlayerState::Won => MusicKind::Victory,
        PlayerState::Defeated => MusicKind::Defeat,
        PlayerState::Active if player.battle => MusicKind::Battle,
        PlayerState::Active => MusicKind::Peace,
    };
    (&player.civ, kind)
}

fn page_now() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
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
    on_click(&play, move || {
        let app = target.borrow();
        let command = if app.paused {
            COMMAND_PLAY
        } else {
            COMMAND_PAUSE
        };
        app.command(command, 0.0);
    });

    let target = Rc::clone(app);
    on_change(&speed, move || {
        let app = target.borrow();
        if let Ok(speed) = app.speed_select.value().parse::<f64>() {
            app.command(COMMAND_SPEED, speed);
        }
    });

    // While dragging only keyframes are shown; the exact turn is decoded on
    // release (init.md 3).
    let target = Rc::clone(app);
    on_input(&timeline, move || {
        let app = &mut *target.borrow_mut();
        app.dragging = true;
        if let Ok(turn) = app.timeline.value().parse::<f64>() {
            app.command(COMMAND_SCRUB, turn);
        }
    });

    let target = Rc::clone(app);
    on_change(&timeline, move || {
        let app = &mut *target.borrow_mut();
        app.dragging = false;
        if let Ok(turn) = app.timeline.value().parse::<f64>() {
            app.command(COMMAND_SEEK, turn);
        }
    });

    let target = Rc::clone(app);
    on_click(&live, move || {
        target.borrow().command(COMMAND_LIVE, 0.0);
    });

    let (current, popup_current, close) = {
        let app = app.borrow();
        (
            app.match_over.button.clone(),
            app.match_over.popup_button.clone(),
            app.match_over.close_button.clone(),
        )
    };
    let owner = app
        .borrow()
        .match_over
        .owner
        .as_ref()
        .map(|owner| (owner.new_match.clone(), owner.rematch.clone()));
    if let Some((new_match, rematch)) = owner {
        on_click(&new_match, open_setup);
        on_click(&rematch, game_menu::rematch);
    }
    let target = Rc::clone(app);
    let on_current = Closure::<dyn FnMut()>::new(move || {
        let app = target.borrow();
        if app.match_over.owner.is_some() {
            open_setup();
            return;
        }
        if app.match_over.archived {
            // The live stream is a page without the match in its link.
            if let Some(window) = web_sys::window() {
                let path = window.location().pathname().unwrap_or_default();
                let _ = window.location().set_href(&path);
            }
            return;
        }
        app.command(COMMAND_CURRENT_MATCH, 0.0);
    });
    current.set_onclick(Some(on_current.as_ref().unchecked_ref()));
    popup_current.set_onclick(Some(on_current.as_ref().unchecked_ref()));
    on_current.forget();

    let target = Rc::clone(app);
    on_click(&close, move || {
        target.borrow_mut().match_over.close();
    });

    let copy_link = app.borrow().copy_link.clone();
    if let Some(copy_link) = copy_link {
        let target = Rc::clone(app);
        on_click(&copy_link, move || copy_match_link(&target));
    }
}

/// The setup page, for a new game in place of the player's.
fn open_setup() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href("/play?new");
    }
}

/// Copies a link to the match at the playhead; `t` is the clock's seconds.
fn copy_match_link(app: &Rc<RefCell<App>>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let (match_id, playhead) = {
        let app = app.borrow();
        (app.scene.match_id, app.playhead)
    };
    if match_id == 0 {
        return;
    }
    let location = window.location();
    let seconds = (f64::from(playhead) * TURN_MS / 1000.0) as u64;
    let link = format!(
        "{}{}?match={match_id:016x}&t={seconds}",
        location.origin().unwrap_or_default(),
        location.pathname().unwrap_or_default()
    );
    // Through Reflect: the Clipboard API is not among web-sys's enabled
    // features, and it is one call.
    let navigator = window.navigator();
    let write_text = js_sys::Reflect::get(&navigator, &"clipboard".into())
        .ok()
        .filter(|clipboard| !clipboard.is_undefined())
        .and_then(|clipboard| {
            let function = js_sys::Reflect::get(&clipboard, &"writeText".into())
                .ok()?
                .dyn_into::<js_sys::Function>()
                .ok()?;
            function.call1(&clipboard, &link.into()).ok()
        });
    // An insecure origin has no clipboard; the label then stays as it is.
    if write_text.is_none() {
        return;
    }
    let app_ref = &mut *app.borrow_mut();
    let Some(button) = app_ref.copy_link.clone() else {
        return;
    };
    lang::set_text(&button, "Link copied");
    if let Some(timer) = app_ref.copy_link_timer.take() {
        window.clear_timeout_with_handle(timer);
    }
    let target = Rc::downgrade(app);
    let restore = Closure::once_into_js(move || {
        if let Some(app) = target.upgrade() {
            let mut app = app.borrow_mut();
            app.copy_link_timer = None;
            if let Some(button) = &app.copy_link {
                lang::set_text(button, "Copy link");
            }
        }
    });
    app_ref.copy_link_timer = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(restore.unchecked_ref(), COPIED_MS)
        .ok();
}

/// The value of `name` in a `?a=1&b=2` query, undecoded; the values read
/// here are hex digits and numbers.
pub fn query_param(search: &str, name: &str) -> Option<String> {
    search
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
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
            Some((&POST_DETAIL, message)) => receive_detail(&receiver, message),
            Some((&POST_STARTS, rest)) => {
                if let Some(wait_ms) = rest.first_chunk::<4>() {
                    // The frame clock, which the countdown runs on.
                    let now = web_sys::window()
                        .and_then(|window| window.performance())
                        .map_or(0.0, |performance| performance.now());
                    let at = now + f64::from(u32::from_le_bytes(*wait_ms));
                    receiver.borrow_mut().stream_start.at = Some(at);
                }
            }
            Some((&POST_TERRAIN, rest)) => {
                if let Some((id, resource)) = rest.split_first_chunk::<8>() {
                    receive_terrain(&receiver, u64::from_le_bytes(*id), resource);
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
                // The frame clock, which the popup's countdown runs on.
                let now = web_sys::window()
                    .and_then(|window| window.performance())
                    .map_or(0.0, |performance| performance.now());
                if let (
                    Some(live),
                    Some(segments),
                    Some(terrain),
                    Some(live_ms),
                    Some(peak_window),
                    Some(detail_live),
                    Some(detail_segments),
                    Some(detail_ms),
                    Some(&detail_on),
                ) = (
                    u64_at(17),
                    u64_at(25),
                    u64_at(37),
                    u64_at(45),
                    u64_at(53),
                    u64_at(61),
                    u64_at(69),
                    u64_at(77),
                    rest.get(85),
                ) {
                    receiver.borrow_mut().stream_bytes = StreamBytes {
                        live,
                        segments,
                        terrain,
                        live_ms: f64::from_bits(live_ms),
                        received_at: now,
                        peak_window,
                        detail_live,
                        detail_segments,
                        detail_ms: f64::from_bits(detail_ms),
                        detail_on: detail_on != 0,
                    };
                }
                receiver
                    .borrow_mut()
                    .match_over
                    .show_status(flags, u32_at(33).unwrap_or(0), now);
                {
                    let app = receiver.borrow();
                    if let Some(menu) = &app.game_menu {
                        menu.update(
                            !seated() || app.finish_noticed,
                            app.match_over.ended,
                            app.own_pause(),
                        );
                    }
                }
                // The match over popup takes the offer's place, and the
                // game has nothing left to reload for.
                if let Ok(mut app) = receiver.try_borrow_mut()
                    && app.match_over.ended
                {
                    app.offer_return = None;
                    if let Some(message_box) = &mut app.message_box
                        && message_box.is_offer()
                    {
                        message_box.cancel();
                    }
                }
                receiver.borrow_mut().show_live_delay();
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

/// None clears them.
fn receive_detail(app: &Rc<RefCell<App>>, message: &[u8]) {
    let details = &mut app.borrow_mut().scene.details;
    if message.is_empty() {
        *details = Details::default();
    } else if let Err(error) = details.apply(message) {
        web_sys::console::error_1(&format!("bad detail message: {error}").into());
        *details = Details::default();
    }
}

/// `turn_ms` is the real time per turn when played back, 0 when live.
fn receive(app_rc: &Rc<RefCell<App>>, turn_ms: f64, message: &[u8]) {
    let app = &mut *app_rc.borrow_mut();
    match app.scene.apply(message, app.progress) {
        // Not a turn: nothing moves, the veil only comes or goes.
        Ok(()) if stream::is_pause(message) => app.sync_pause(),
        // The dots now start where they are drawn, so the new turn plays
        // from 0; a second turn before the next frame starts there too.
        Ok(()) => {
            app.stream_start.at = None;
            app.progress = 0.0;
            app.turn_arrived = true;
            app.scene_changed = true;
            app.renames.extend_from_slice(&app.scene.renames);
            for (revision, reveal) in std::mem::take(&mut app.scene.reveals) {
                if let Some((id, image)) = &mut app.terrain_image
                    && *id == app.scene.terrain_id
                    && image.seat.is_some()
                {
                    match image.apply_reveal(revision, &reveal) {
                        Ok(()) => app.terrain_revealed = true,
                        Err(error) => {
                            web_sys::console::error_1(&format!("bad reveal: {error}").into())
                        }
                    }
                }
            }
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
            app.play_turn_sounds();
            app.sync_pause();
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
                if let Some(warmup) = &mut app.warmup {
                    warmup.set_terrains(Vec::new());
                }
                match &app.terrain_image {
                    Some((id, image)) if *id == terrain_id => {
                        app.renderer.set_terrain(Some(image));
                        if let Some(minimap) = &mut app.minimap {
                            minimap.set_terrain(Some(image));
                        }
                        if let Some(warmup) = &mut app.warmup {
                            warmup.set_terrains(image.names.clone());
                        }
                        app.dirty = true;
                    }
                    // Not fatal: the dots still work on a plain background.
                    _ if terrain_id != 0 => web_sys::console::error_1(
                        &format!("terrain {terrain_id:016x} was not handed over").into(),
                    ),
                    _ => {}
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

/// The worker hands over a match's terrain before the first keyframe that
/// names it; the scene needs its heightmap to apply that keyframe.
fn receive_terrain(app_rc: &Rc<RefCell<App>>, id: u64, resource: &[u8]) {
    let app = &mut *app_rc.borrow_mut();
    let built = terrain::build(resource).and_then(|image| {
        let mut ground = Ground::new(id, image.size, image.heights.clone())?;
        ground.revision = image.seat.as_ref().map_or(0, |seat| seat.revision);
        Ok((image, ground))
    });
    match built {
        Ok((image, ground)) => {
            // A seat's terrain only grows; one the deltas took further is
            // kept.
            let held = |revision: Option<u32>| match (&app.terrain_image, revision) {
                (Some((held, image)), Some(revision)) if *held == id => image
                    .seat
                    .as_ref()
                    .is_some_and(|seat| seat.revision >= revision),
                _ => false,
            };
            if held(image.seat.as_ref().map(|seat| seat.revision)) {
                return;
            }
            app.scene.ground = Some(Rc::new(ground));
            let shown = app.terrain_id == id;
            app.terrain_image = Some((id, image));
            if shown {
                app.terrain_revealed = true;
                app.terrain_rebuilt_at = f64::NEG_INFINITY;
            }
        }
        // The keyframe fails without it, and that stops the stream.
        Err(error) => {
            web_sys::console::error_1(&format!("terrain {id:016x}: {error}").into());
        }
    }
}

async fn load_icons(app: Rc<RefCell<App>>, url: String) {
    let atlas = fetch_bytes(&url)
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

async fn load_selection_atlas(app: Rc<RefCell<App>>, url: String) {
    match fetch_bytes(&url).await {
        Ok(bytes) => app.borrow_mut().renderer.set_selection_atlas(&bytes),
        // Not fatal: the selection works, only unseen.
        Err(error) => web_sys::console::error_1(&format!("selection.bin: {error}").into()),
    }
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

fn animate(app: Rc<RefCell<App>>, bench: Option<Rc<RefCell<Bench>>>) {
    let callback: Rc<RefCell<Option<FrameCallback>>> = Rc::new(RefCell::new(None));
    let next = Rc::clone(&callback);
    // The closure holds an Rc to itself, which keeps the loop alive for the
    // lifetime of the page.
    let previous = Cell::new(None);
    *callback.borrow_mut() = Some(Closure::new(move |now: f64| {
        // Between callbacks, which shows a stall the frame's own time
        // leaves out: the browser's work, or a wait on the GPU.
        if let Some(previous) = previous.replace(Some(now)) {
            timing::interval(now - previous);
        }
        app.borrow_mut().frame(now);
        if let Some(bench) = &bench {
            bench.borrow_mut().step(&app, now);
        }
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
