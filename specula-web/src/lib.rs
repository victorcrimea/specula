mod assets;
mod audio;
mod bench;
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
mod particles;
mod pick;
mod pose;
mod postproc;
mod projectile;
mod render;
mod shadow;
mod sky;
mod stored;
mod stream;
mod terrain;
mod territory;
mod timing;
mod unit;
mod variation;
mod warmup;
mod water;
mod worker;

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use specula_assets::files::MusicKind;
use specula_assets::files::TEXTURE_TIER_SIDES;
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
use crate::assets::Store;
use crate::audio::Audio;
use crate::audio::DEFAULT_VOLUME;
use crate::audio::Settings;
use crate::bench::Bench;
use crate::bench::BenchInfo;
use crate::bench::CameraPose;
use crate::diplomacy::Perspective;
use crate::field::Field;
use crate::icons::IconAtlas;
use crate::minimap::Minimap;
use crate::postproc::Upscale;
use crate::render::Renderer;
use crate::stream::Ground;
use crate::stream::PlayerState;
use crate::stream::Scene;
use crate::stream::Sound;
use crate::stream::SoundTarget;
use crate::terrain::TerrainImage;
use crate::warmup::Warmup;
use crate::worker::COMMAND_CURRENT_MATCH;
use crate::worker::COMMAND_LIVE;
use crate::worker::COMMAND_PAUSE;
use crate::worker::COMMAND_PLAY;
use crate::worker::COMMAND_SCRUB;
use crate::worker::COMMAND_SEEK;
use crate::worker::COMMAND_SPEED;
use crate::worker::POST_SCENE;
use crate::worker::POST_STATUS;
use crate::worker::POST_TERRAIN;
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
const POSITION_UNITS_PER_METRE: f32 = 16.0;
/// localStorage key of the sound settings: "on,master,effects,music".
const SOUND_SETTINGS_KEY: &str = "specula.sound";
/// localStorage key of the shadow map size, 0 for no shadows.
const SHADOW_SETTINGS_KEY: &str = "specula.shadows";
/// localStorage key of the shadow cutoff distance in metres.
const SHADOW_CUTOFF_SETTINGS_KEY: &str = "specula.shadowscutoff";
/// localStorage key of the texture quality: 1 low, 2 medium, 3 high.
const TEXTURE_SETTINGS_KEY: &str = "specula.textures";
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
const PARTICLES_SETTINGS_KEY: &str = "specula.particles";
const PROJECTILES_SETTINGS_KEY: &str = "specula.projectiles";
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
    pub visibility: u16,
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
    /// `None` for a new match whose terrain has not arrived yet.
    fn set_terrain(&mut self, image: Option<&TerrainImage>);
    fn set_icon_atlas(&mut self, _atlas: IconAtlas) {}
    /// Whether it moves between turns on its own, and so needs every frame.
    fn animating(&self) -> bool {
        false
    }
    /// Whether to draw projectiles, where it draws any.
    fn set_projectiles(&mut self, _on: bool) {}
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
    /// Whether the actors' particles are drawn, likewise.
    fn set_particles(&mut self, _on: bool) {}
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
/// history segments and the terrain.
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
    /// The terrain shown; 0 for none.
    terrain_id: u64,
    /// The terrain the worker handed over, until a keyframe shows it.
    terrain_image: Option<(u64, TerrainImage)>,
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
    /// Only the 3D page plays sound.
    sound: Option<SoundControls>,
    /// Only the 3D page has assets to warm up.
    warmup: Option<Warmup>,
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
        self.button
            .set_text_content(Some(if settings.on { "Sound on" } else { "Sound off" }));
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
        let start = page_now();
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
            drew = !self.dirty;
            if !self.dirty {
                self.progress = progress;
            }
        }
        if let Some(minimap) = &mut self.minimap {
            minimap.frame(progress, self.renderer.view_outline());
        }
        self.match_over.frame(now);
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
        timing::frame(page_now() - start, drew);
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
/// `glue_url` and `wasm_url` as for `observer_main`.
#[wasm_bindgen]
pub fn page_main(glue_url: String, wasm_url: String) {
    set_panic_hook();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = run(Page::Map, glue_url, wasm_url).await {
            web_sys::console::error_1(&error.into());
        }
    });
}

/// Entry point of index.html: the same stream and controls as the
/// page's, drawn in 3D. `glue_url` and `wasm_url` are the URLs the page loaded
/// this module from; the stream worker loads the same ones, so a cached
/// older worker can never pair an old glue with this wasm.
#[wasm_bindgen]
pub fn observer_main(glue_url: String, wasm_url: String) {
    set_panic_hook();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = run(Page::Observer, glue_url, wasm_url).await {
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

async fn run(page: Page, glue_url: String, wasm_url: String) -> Result<(), String> {
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
    let warmup = match page {
        Page::Map => None,
        Page::Observer => Some(Warmup::new(element(&document, "warmup")?)),
    };
    let options = WorkerOptions::new();
    options.set_type(WorkerType::Module);
    let worker_url = format!(
        "worker.js?glue={}&wasm={}",
        js_sys::encode_uri_component(&glue_url),
        js_sys::encode_uri_component(&wasm_url)
    );
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
        terrain_image: None,
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
        sound,
        warmup,
    }));
    wire_controls(&app);
    if page == Page::Observer {
        let shadows: HtmlSelectElement = element(&document, "shadows")?;
        wire_shadows(&app, shadows.clone());
        wire_shadow_cutoff(
            &app,
            element(&document, "shadowscutoff")?,
            element(&document, "shadowscutoff-value")?,
            &shadows,
        );
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
        wire_switch(
            &app,
            element(&document, "particles")?,
            PARTICLES_SETTINGS_KEY,
            |view, on| view.set_particles(on),
            Vec::new(),
        );
        wire_projectiles(&app, element(&document, "projectiles")?);
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
    wasm_bindgen_futures::spawn_local(load_icons(Rc::clone(&app)));
    animate(app, bench);
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
    let on_change = Closure::<dyn FnMut()>::new(move || {
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
    select.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_change.forget();
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
    let on_change = Closure::<dyn FnMut()>::new(move || {
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
    select.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_change.forget();
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
    let on_change = Closure::<dyn FnMut()>::new(move || {
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
    select.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_change.forget();
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
    let saved = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(key).ok().flatten())
        .is_some_and(|text| text == "1");
    select.set_value(if saved { "1" } else { "0" });
    apply(&mut *app.borrow_mut().renderer, saved);
    for dependent in &dependents {
        dependent.set_disabled(!saved);
    }

    let target = Rc::clone(app);
    let list = select.clone();
    let on_change = Closure::<dyn FnMut()>::new(move || {
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
    select.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_change.forget();
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
    let on_change = Closure::<dyn FnMut()>::new(move || {
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
    select.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_change.forget();
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
    let on_change = Closure::<dyn FnMut()>::new(move || {
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
    select.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_change.forget();
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
    let on_change = Closure::<dyn FnMut()>::new(move || {
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
    select.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_change.forget();
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
    let on_change = Closure::<dyn FnMut()>::new(move || {
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
    select.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_change.forget();
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
    let on_input = Closure::<dyn FnMut()>::new(move || {
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
    slider.set_oninput(Some(on_input.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_input.forget();

    // Saved on release only, so a drag does not write storage per step.
    let input = slider.clone();
    let on_change = Closure::<dyn FnMut()>::new(move || {
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(SHADOW_CUTOFF_SETTINGS_KEY, &input.value());
        }
    });
    slider.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();

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
    let on_input = Closure::<dyn FnMut()>::new(move || {
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
    slider.set_oninput(Some(on_input.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_input.forget();

    // Saved on release only, so a drag does not write storage per step.
    let input = slider.clone();
    let on_change = Closure::<dyn FnMut()>::new(move || {
        // Best effort: without storage the choice lasts only this visit.
        if let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        {
            let _ = storage.set_item(SHARPNESS_SETTINGS_KEY, &input.value());
        }
    });
    slider.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();

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
    let on_click = Closure::<dyn FnMut()>::new(move || {
        let app = &mut *target.borrow_mut();
        if app.renderer.asset_store().is_some() && app.warmup.is_some() {
            app.start_warmup();
            stored::persist();
        }
    });
    button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
    on_click.forget();
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

    let popup = notice.clone();
    let on_ok = Closure::<dyn FnMut()>::new(move || popup.set_hidden(true));
    ok.set_onclick(Some(on_ok.as_ref().unchecked_ref()));
    on_ok.forget();

    let target = Rc::clone(app);
    let on_warm = Closure::<dyn FnMut()>::new(move || {
        notice.set_hidden(true);
        if let Some(warmup) = &mut target.borrow_mut().warmup {
            warmup.request();
        }
        // Here rather than when the run starts: Firefox asks the viewer,
        // and only from a click.
        stored::persist();
    });
    warm.set_onclick(Some(on_warm.as_ref().unchecked_ref()));
    on_warm.forget();
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
    let on_click = Closure::<dyn FnMut()>::new(move || {
        if let Some(sound) = &mut target.borrow_mut().sound {
            let on = !sound.audio.settings().on;
            sound.changed(on);
        }
    });
    button.set_onclick(Some(on_click.as_ref().unchecked_ref()));
    // The controls live as long as the page, so their handlers do too.
    on_click.forget();

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
fn hears(perspective: Perspective, owner: i32, visibility: u16) -> Option<bool> {
    match perspective {
        Perspective::Observer => Some(false),
        Perspective::Player(player) => {
            let sees = player < 16 && visibility & (1 << player) != 0;
            sees.then_some(usize::try_from(owner) == Ok(player))
        }
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
                ) = (u64_at(17), u64_at(25), u64_at(37), u64_at(45), u64_at(53))
                {
                    receiver.borrow_mut().stream_bytes = StreamBytes {
                        live,
                        segments,
                        terrain,
                        live_ms: f64::from_bits(live_ms),
                        received_at: now,
                        peak_window,
                    };
                }
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
            app.play_turn_sounds();
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
                match app.terrain_image.take() {
                    Some((id, image)) if id == terrain_id => {
                        app.renderer.set_terrain(Some(&image));
                        if let Some(minimap) = &mut app.minimap {
                            minimap.set_terrain(Some(&image));
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
        let ground = Ground::new(id, image.size, image.heights.clone())?;
        Ok((image, ground))
    });
    match built {
        Ok((image, ground)) => {
            app.scene.ground = Some(Rc::new(ground));
            app.terrain_image = Some((id, image));
        }
        // The keyframe fails without it, and that stops the stream.
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
