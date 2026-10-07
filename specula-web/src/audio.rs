//! The match's sounds through Web Audio, as Pyrogenesis' sound manager
//! plays them for a viewer: sound groups (`CSoundGroup`) placed by their
//! position on screen, the session music of gui/common/music.js and the
//! ambient loop. Nothing is fetched or decoded until the viewer turns the
//! sound on, which is also the gesture the browser wants before it plays.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use specula_assets::files::FileRef;
use specula_assets::files::Kind;
use specula_assets::files::MusicKind;
use specula_assets::files::SOUND_DISTANCELESS;
use specula_assets::files::SOUND_OMNIPRESENT;
use specula_assets::files::SOUND_OWNER_ONLY;
use specula_assets::files::SOUND_RANDOM_GAIN;
use specula_assets::files::SOUND_RANDOM_PITCH;
use specula_assets::files::SoundGroup;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::AudioBuffer;
use web_sys::AudioBufferSourceNode;
use web_sys::AudioContext;
use web_sys::AudioScheduledSourceNode;
use web_sys::GainNode;

use crate::Listener;
use crate::assets::Get;
use crate::assets::Store;

/// `sound.mindistance` and `sound.maxdistance`, with OpenAL's
/// `AL_LINEAR_DISTANCE` model the engine selects.
const MIN_DISTANCE: f32 = 1.0;
const MAX_DISTANCE: f32 = 350.0;
/// `sound.maxstereoangle`: how far left or right the screen's edges are.
const MAX_STEREO_ANGLE: f32 = std::f32::consts::PI / 6.0;
/// `DEFAULT_ROLLOFF` and, in the margin just off screen, `MAX_ROLLOFF`.
const DEFAULT_ROLLOFF: f32 = 0.5;
const MAX_ROLLOFF: f32 = 0.7;
/// `RadiansOffCenter`'s margins: 10% of the width, 15 pixels.
const X_MARGIN: f32 = 0.1;
const Y_MARGIN: f32 = 15.0;
/// `SOURCE_NUM`: a sound beyond this many at once is dropped.
const MAX_VOICES: u32 = 64;
/// The engine plays a sound the frame it is asked for; a file still on
/// its way is played if it arrives within this, else dropped.
const PENDING_MS: f64 = 500.0;
const MAX_FETCHES: usize = 6;
/// `SetMusicItem`: the old tune fades out over 2 s, the new one in over
/// 1 s; `SetAmbientItem` fades the ambient in over 2 s.
const MUSIC_FADE_OUT_S: f64 = 2.0;
const MUSIC_FADE_IN_S: f64 = 1.0;
const AMBIENT_FADE_IN_S: f64 = 2.0;
/// `sound.mastergain`, `sound.actiongain` and `sound.musicgain`'s
/// defaults; the ambient follows the music.
pub const DEFAULT_VOLUME: f32 = 0.5;

/// What the viewer set; volumes 0..=1.
#[derive(Clone, Copy, PartialEq)]
pub struct Settings {
    pub on: bool,
    pub master: f32,
    pub effects: f32,
    pub music: f32,
}

struct Buses {
    context: AudioContext,
    master: GainNode,
    effects: GainNode,
    music: GainNode,
}

enum Buffer {
    Loading,
    Ready(AudioBuffer),
    Failed,
}

/// A sound waiting for its group or file.
struct Pending {
    group: String,
    /// In metres.
    position: [f32; 3],
    /// Owned by the player whose perspective is shown.
    owned: bool,
    /// The file chosen once the group is in.
    file: Option<FileRef>,
    until: f64,
}

/// The playlist music.js would be playing.
struct Music {
    civ: String,
    kind: MusicKind,
    tracks: Vec<FileRef>,
    next: usize,
    current: Option<(AudioBufferSourceNode, GainNode)>,
    /// Set by the playing track when it ends.
    ended: Rc<Cell<bool>>,
}

pub struct Audio {
    store: Rc<RefCell<Store>>,
    settings: Settings,
    buses: Option<Buses>,
    buffers: Rc<RefCell<HashMap<u64, Buffer>>>,
    fetches: Rc<Cell<usize>>,
    /// Files to fetch once a fetch is free, most wanted first.
    fetch_queue: Vec<FileRef>,
    voices: Rc<Cell<u32>>,
    pending: Vec<Pending>,
    music: Option<Music>,
    /// The (civ, kind) the music should be.
    wanted_music: (String, MusicKind),
    ambient_started: bool,
}

impl Audio {
    pub fn new(store: Rc<RefCell<Store>>, settings: Settings) -> Self {
        Audio {
            store,
            settings,
            buses: None,
            buffers: Rc::new(RefCell::new(HashMap::new())),
            fetches: Rc::new(Cell::new(0)),
            fetch_queue: Vec::new(),
            voices: Rc::new(Cell::new(0)),
            pending: Vec::new(),
            music: None,
            wanted_music: (String::new(), MusicKind::Peace),
            ambient_started: false,
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    /// Called from a user gesture when `on`: the context can only start
    /// in one.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
        if settings.on && self.buses.is_none() {
            self.buses = create_buses();
        }
        if let Some(buses) = &self.buses {
            if settings.on {
                let _ = buses.context.resume();
            }
            buses
                .master
                .gain()
                .set_value(if settings.on { settings.master } else { 0.0 });
            buses.effects.gain().set_value(settings.effects);
            buses.music.gain().set_value(settings.music);
        }
    }

    /// A user gesture: a context the browser held suspended can resume.
    pub fn gesture(&mut self) {
        if self.settings.on {
            self.set_settings(self.settings);
        }
    }

    fn playing(&self) -> bool {
        self.settings.on && self.buses.is_some()
    }

    /// Asks for a sound group to be played, as `CSoundManager::PlayAsGroup`
    /// does once `CCmpSoundManager` let it through: an owner-only group
    /// only when `owned`.
    /// `position` is in metres.
    pub fn play_group(&mut self, group: &str, position: [f32; 3], owned: bool, now: f64) {
        if !self.playing() {
            return;
        }
        self.pending.push(Pending {
            group: group.to_string(),
            position,
            owned,
            file: None,
            until: now + PENDING_MS,
        });
    }

    /// The music for this civ ("" for the defaults) and state.
    pub fn set_music(&mut self, civ: &str, kind: MusicKind) {
        self.wanted_music = (civ.to_string(), kind);
    }

    pub fn frame(&mut self, now: f64, listener: Option<&Listener>) {
        if !self.playing() {
            self.pending.clear();
            return;
        }
        let pending = std::mem::take(&mut self.pending);
        for mut sound in pending {
            if now > sound.until {
                continue;
            }
            let group = match self.store.borrow_mut().sound_group(&sound.group) {
                Get::Ready(group) => group,
                Get::Pending => {
                    self.pending.push(sound);
                    continue;
                }
                Get::Missing => continue,
            };
            if group.flags & SOUND_OWNER_ONLY != 0 && !sound.owned {
                continue;
            }
            // `PlayNext`: any of the files, whatever `RandOrder` says.
            let file = match sound.file {
                Some(file) => file,
                None => {
                    if group.sounds.is_empty() {
                        continue;
                    }
                    let index = (js_sys::Math::random() * group.sounds.len() as f64) as usize;
                    group.sounds[index.min(group.sounds.len() - 1)]
                }
            };
            sound.file = Some(file);
            match self.buffer(file) {
                Some(Buffer::Ready(buffer)) => {
                    if let Some(listener) = listener {
                        self.start(&group, &buffer, sound.position, listener);
                    }
                }
                Some(Buffer::Failed) => {}
                Some(Buffer::Loading) | None => self.pending.push(sound),
            }
        }
        self.update_music();
        self.update_ambient();
        self.pump();
    }

    /// The decoded file, asking for it if it is not.
    fn buffer(&mut self, file: FileRef) -> Option<Buffer> {
        let buffers = self.buffers.borrow();
        match buffers.get(&file.hash) {
            Some(Buffer::Ready(buffer)) => Some(Buffer::Ready(buffer.clone())),
            Some(Buffer::Loading) => Some(Buffer::Loading),
            Some(Buffer::Failed) => Some(Buffer::Failed),
            None => {
                drop(buffers);
                self.buffers.borrow_mut().insert(file.hash, Buffer::Loading);
                self.fetch_queue.push(file);
                None
            }
        }
    }

    fn pump(&mut self) {
        let Some(buses) = &self.buses else {
            return;
        };
        while self.fetches.get() < MAX_FETCHES && !self.fetch_queue.is_empty() {
            let file = self.fetch_queue.remove(0);
            self.fetches.set(self.fetches.get() + 1);
            wasm_bindgen_futures::spawn_local(load(
                buses.context.clone(),
                file,
                Rc::clone(&self.buffers),
                Rc::clone(&self.fetches),
            ));
        }
    }

    /// `CSoundGroup::UploadPropertiesAndPlay`.
    fn start(
        &self,
        group: &SoundGroup,
        buffer: &AudioBuffer,
        position: [f32; 3],
        listener: &Listener,
    ) {
        let Some(buses) = &self.buses else {
            return;
        };
        let omnipresent = group.flags & SOUND_OMNIPRESENT != 0;
        let distanceless = group.flags & SOUND_DISTANCELESS != 0;
        let (on_screen, angle, rolloff) = off_center(position, listener);
        if !on_screen && !distanceless && !omnipresent {
            return;
        }
        if self.voices.get() >= MAX_VOICES {
            return;
        }
        let random =
            |lower: f32, upper: f32| lower + (upper - lower) * js_sys::Math::random() as f32;
        let pitch = if group.flags & SOUND_RANDOM_PITCH != 0 {
            random(group.pitch_lower, group.pitch_upper)
        } else {
            group.pitch
        };
        let mut gain = if group.flags & SOUND_RANDOM_GAIN != 0 {
            random(group.gain_lower, group.gain_upper)
        } else {
            group.gain
        };
        // OpenAL does not place a stereo sound, nor one never placed.
        let mut pan = 0.0;
        if !omnipresent && buffer.number_of_channels() == 1 {
            let distance = (0..3)
                .map(|i| (position[i] - listener.eye[i]).powi(2))
                .sum::<f32>()
                .sqrt()
                .clamp(MIN_DISTANCE, MAX_DISTANCE);
            let rolloff = if distanceless { 0.0 } else { rolloff };
            gain *= (1.0 - rolloff * (distance - MIN_DISTANCE) / (MAX_DISTANCE - MIN_DISTANCE))
                .max(0.0);
            pan = (angle / MAX_STEREO_ANGLE).clamp(-1.0, 1.0);
        }
        let context = &buses.context;
        let (Ok(source), Ok(volume), Ok(panner)) = (
            context.create_buffer_source(),
            context.create_gain(),
            context.create_stereo_panner(),
        ) else {
            return;
        };
        source.set_buffer(Some(buffer));
        source.playback_rate().set_value(pitch);
        volume.gain().set_value(gain);
        panner.pan().set_value(pan);
        let connected = source
            .connect_with_audio_node(&volume)
            .and_then(|_| volume.connect_with_audio_node(&panner))
            .and_then(|_| panner.connect_with_audio_node(&buses.effects));
        if connected.is_err() || source.start().is_err() {
            return;
        }
        let voices = Rc::clone(&self.voices);
        voices.set(voices.get() + 1);
        let on_ended = Closure::once_into_js(move || voices.set(voices.get().saturating_sub(1)));
        AsRef::<AudioScheduledSourceNode>::as_ref(&source)
            .set_onended(Some(on_ended.unchecked_ref()));
    }

    /// music.js' playlists: shuffled, looping, the civ's tracks of the
    /// state or else the defaults of it.
    fn update_music(&mut self) {
        let (civ, kind) = self.wanted_music.clone();
        let changed = self
            .music
            .as_ref()
            .is_none_or(|music| music.civ != civ || music.kind != kind);
        if changed {
            let store = self.store.borrow();
            let Some(own) = store.music(&civ) else {
                return;
            };
            let of_kind = |tracks: &[(MusicKind, FileRef)]| -> Vec<FileRef> {
                tracks
                    .iter()
                    .filter(|(track_kind, _)| *track_kind == kind)
                    .map(|(_, file)| *file)
                    .collect()
            };
            let mut tracks = of_kind(own);
            if tracks.is_empty() {
                tracks = of_kind(store.music("").unwrap_or_default());
            }
            drop(store);
            shuffle(&mut tracks);
            if let Some(old) = self.music.take() {
                fade_out(&old.current, &self.buses);
            }
            self.music = Some(Music {
                civ,
                kind,
                tracks,
                next: 0,
                current: None,
                ended: Rc::new(Cell::new(false)),
            });
        }
        let Some(music) = &self.music else {
            return;
        };
        if music.tracks.is_empty() || (music.current.is_some() && !music.ended.get()) {
            return;
        }
        let file = music.tracks[music.next % music.tracks.len()];
        let Some(Buffer::Ready(buffer)) = self.buffer(file) else {
            return;
        };
        let Some(buses) = &self.buses else {
            return;
        };
        let ended = Rc::new(Cell::new(false));
        let current = play_loop(buses, &buffer, false, MUSIC_FADE_IN_S, &ended);
        let music = self.music.as_mut().unwrap();
        music.next += 1;
        music.ended = ended;
        music.current = current;
    }

    fn update_ambient(&mut self) {
        if self.ambient_started {
            return;
        }
        let Some(file) = self.store.borrow().ambient() else {
            return;
        };
        let Some(Buffer::Ready(buffer)) = self.buffer(file) else {
            return;
        };
        if let Some(buses) = &self.buses {
            let ended = Rc::new(Cell::new(false));
            self.ambient_started =
                play_loop(buses, &buffer, true, AMBIENT_FADE_IN_S, &ended).is_some();
        }
    }
}

fn create_buses() -> Option<Buses> {
    let context = AudioContext::new().ok()?;
    let master = context.create_gain().ok()?;
    let effects = context.create_gain().ok()?;
    let music = context.create_gain().ok()?;
    master
        .connect_with_audio_node(&context.destination())
        .ok()?;
    effects.connect_with_audio_node(&master).ok()?;
    music.connect_with_audio_node(&master).ok()?;
    Some(Buses {
        context,
        master,
        effects,
        music,
    })
}

/// A track on the music bus, faded in; `ended` is set when it stops.
fn play_loop(
    buses: &Buses,
    buffer: &AudioBuffer,
    looping: bool,
    fade_in_s: f64,
    ended: &Rc<Cell<bool>>,
) -> Option<(AudioBufferSourceNode, GainNode)> {
    let context = &buses.context;
    let source = context.create_buffer_source().ok()?;
    let volume = context.create_gain().ok()?;
    source.set_buffer(Some(buffer));
    source.set_loop(looping);
    let now = context.current_time();
    volume.gain().set_value_at_time(0.0, now).ok()?;
    volume
        .gain()
        .linear_ramp_to_value_at_time(1.0, now + fade_in_s)
        .ok()?;
    source.connect_with_audio_node(&volume).ok()?;
    volume.connect_with_audio_node(&buses.music).ok()?;
    source.start().ok()?;
    let ended = Rc::clone(ended);
    let on_ended = Closure::once_into_js(move || ended.set(true));
    AsRef::<AudioScheduledSourceNode>::as_ref(&source).set_onended(Some(on_ended.unchecked_ref()));
    Some((source, volume))
}

fn fade_out(current: &Option<(AudioBufferSourceNode, GainNode)>, buses: &Option<Buses>) {
    let (Some((source, volume)), Some(buses)) = (current, buses) else {
        return;
    };
    let now = buses.context.current_time();
    let gain = volume.gain();
    let _ = gain.cancel_scheduled_values(now);
    let _ = gain.set_value_at_time(gain.value(), now);
    let _ = gain.linear_ramp_to_value_at_time(0.0, now + MUSIC_FADE_OUT_S);
    let _ =
        AsRef::<AudioScheduledSourceNode>::as_ref(source).stop_with_when(now + MUSIC_FADE_OUT_S);
}

/// Fetches and decodes a sound file into `buffers`.
async fn load(
    context: AudioContext,
    file: FileRef,
    buffers: Rc<RefCell<HashMap<u64, Buffer>>>,
    fetches: Rc<Cell<usize>>,
) {
    let url = format!("/assets/{}", Kind::Sound.file_name(file.hash));
    let decoded = async {
        let bytes = crate::fetch_bytes(&url).await?;
        let array = js_sys::Uint8Array::from(&bytes[..]);
        let promise = context
            .decode_audio_data(&array.buffer())
            .map_err(|_| "cannot decode".to_string())?;
        let buffer = JsFuture::from(promise)
            .await
            .map_err(|_| "cannot decode".to_string())?;
        buffer
            .dyn_into::<AudioBuffer>()
            .map_err(|_| "not an audio buffer".to_string())
    }
    .await;
    fetches.set(fetches.get() - 1);
    let slot = match decoded {
        Ok(buffer) => Buffer::Ready(buffer),
        Err(error) => {
            crate::log(&format!("sound {url}: {error}"));
            Buffer::Failed
        }
    };
    buffers.borrow_mut().insert(file.hash, slot);
}

/// `CSoundGroup::RadiansOffCenter`: whether the point is on screen (with
/// the margins), its angle left or right of the centre, and the rolloff.
fn off_center(position: [f32; 3], listener: &Listener) -> (bool, f32, f32) {
    let m = &listener.view_projection;
    let clip: [f32; 4] = std::array::from_fn(|row| {
        m[row][0] * position[0] + m[row][1] * position[1] + m[row][2] * position[2] + m[row][3]
    });
    // Behind the camera it has no place on the screen.
    if clip[3] <= 0.0 {
        return (false, 0.0, DEFAULT_ROLLOFF);
    }
    let (width, height) = (listener.width, listener.height);
    let x = (clip[0] / clip[3] + 1.0) / 2.0 * width;
    let y = (1.0 - clip[1] / clip[3]) / 2.0 * height;
    let x_margin = width * X_MARGIN;
    let mut on_screen = true;
    let mut rolloff = DEFAULT_ROLLOFF;
    let angle = if x < -x_margin {
        on_screen = false;
        -MAX_STEREO_ANGLE
    } else if x > width + x_margin {
        on_screen = false;
        MAX_STEREO_ANGLE
    } else {
        if x < 0.0 || x > width {
            rolloff = MAX_ROLLOFF;
        }
        MAX_STEREO_ANGLE * (x * 2.0 / width - 1.0)
    };
    if y < -Y_MARGIN || y > height + Y_MARGIN {
        on_screen = false;
    } else if y < 0.0 || y > height {
        rolloff = MAX_ROLLOFF;
    }
    (on_screen, angle, rolloff)
}

/// music.js' `shuffleArray`.
fn shuffle(tracks: &mut [FileRef]) {
    for i in (1..tracks.len()).rev() {
        let j = (js_sys::Math::random() * (i + 1) as f64) as usize;
        tracks.swap(i, j.min(i));
    }
}
