//! Fetches the split asset files bake-assets writes (format in
//! specula-assets' `files.rs`) when something on screen first needs them,
//! a few at a time so the stream and the page stay responsive, and keeps
//! them decoded. Every reference carries its file's size, so the page can
//! show how much of what it has asked for has arrived.
//!
//! A mesh or texture is ready once its coarse file is in. Its detail file
//! is fetched only when asked for, after every pending coarse file, and
//! then replaces the coarse value with the full one. A texture's detail
//! comes in tiers, each fetched once the smaller one is in, up to the
//! viewer's texture quality.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::rc::Rc;

use specula_assets::files::AnimationData;
use specula_assets::files::CivAssets;
use specula_assets::files::FileRef;
use specula_assets::files::Index;
use specula_assets::files::Kind;
use specula_assets::files::Manifest;
use specula_assets::files::Mesh;
use specula_assets::files::MeshDetail;
use specula_assets::files::MusicKind;
use specula_assets::files::Sinking;
use specula_assets::files::Skeleton;
use specula_assets::files::SoundGroup;
use specula_assets::files::TEXTURE_TIER_SIDES;
use specula_assets::files::TerrainTexture;
use specula_assets::files::Texture;
use specula_assets::files::TextureDetail;

/// Requests at once; more would only queue in the browser and hold back
/// the segment fetches.
const MAX_IN_FLIGHT: usize = 6;
/// While the cache answers: they take no connection, and one at a time
/// each still waits a round of the event loop.
const MAX_CACHED_IN_FLIGHT: usize = 32;
/// Of those, detail files at most; the rest stay free for coarse files,
/// which a unit cannot be drawn without.
const MAX_DETAIL_IN_FLIGHT: usize = 4;
const POINTER: &str = "/assets.txt";

pub enum Get<T> {
    Ready(T),
    Pending,
    /// Failed to download or decode, or not in the bake; it will not come.
    Missing,
}

/// Since the page loaded: what is held decoded, and how the files came.
#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub files: u32,
    /// Their file sizes.
    pub bytes: u64,
    /// Over the network, headers included.
    pub downloaded: u64,
    /// Bodies the browser's caches served.
    pub cached: u64,
}

/// What the page's loading bar shows.
pub enum Progress {
    Idle,
    /// The index is on its way; sizes are not known yet.
    Starting,
    Bytes {
        done: u64,
        total: u64,
    },
}

enum Slot<T> {
    Pending,
    Ready(T),
    Failed,
}

enum Decoded {
    Actor(Rc<Manifest>),
    Mesh(Rc<Mesh>),
    Skeleton(Rc<Skeleton>),
    Animation(Rc<AnimationData>),
    Texture(Rc<Texture>),
    SoundGroup(Rc<SoundGroup>),
    MeshDetail(MeshDetail),
    TextureDetail(TextureDetail),
}

/// A file to fetch, and for a detail file the hash of the coarse file it
/// completes.
#[derive(Clone, Copy)]
struct Job {
    kind: Kind,
    file: FileRef,
    coarse: u64,
}

#[derive(Default)]
pub struct Store {
    /// Actor path -> manifest; `None` until the index arrives.
    index: Option<HashMap<String, FileRef>>,
    /// Terrain name -> its texture, from the same index.
    terrains: HashMap<String, TerrainTexture>,
    alpha_maps: Option<FileRef>,
    /// Water type -> its normal map frames.
    waters: HashMap<String, Vec<FileRef>>,
    /// The shore waves' foam.png and coastalWave.png.
    foam: Option<FileRef>,
    coastal_wave: Option<FileRef>,
    /// Sky set -> its cube layers.
    skies: HashMap<String, [FileRef; 6]>,
    /// Sound group path relative to audio/ -> its manifest.
    sound_groups: HashMap<String, FileRef>,
    /// Civ code -> its tracks; the empty code holds the defaults.
    music: HashMap<String, Vec<(MusicKind, FileRef)>>,
    ambient: Option<FileRef>,
    /// Civ code -> what its templates can show and play.
    civs: HashMap<String, CivAssets>,
    /// Template name -> how its corpse or rubble sinks.
    sinking: HashMap<String, Sinking>,
    /// Identity's Icon, relative to art/textures/ui/session/portraits/ ->
    /// its portrait.
    portraits: HashMap<String, FileRef>,
    /// Technology name -> its icon, a key of `portraits`.
    technology_icons: HashMap<String, String>,
    index_failed: bool,
    /// The index's path, `assets/index-<hash>.bin`, once loaded.
    index_name: Option<String>,
    actors: HashMap<u64, Slot<Rc<Manifest>>>,
    meshes: HashMap<u64, Slot<Rc<Mesh>>>,
    skeletons: HashMap<u64, Slot<Rc<Skeleton>>>,
    animations: HashMap<u64, Slot<Rc<AnimationData>>>,
    textures: HashMap<u64, Slot<Rc<Texture>>>,
    sound_group_files: HashMap<u64, Slot<Rc<SoundGroup>>>,
    queue: VecDeque<Job>,
    detail_queue: VecDeque<Job>,
    /// Detail files asked for, so each is fetched once.
    detail_asked: HashSet<u64>,
    /// How many of `TEXTURE_TIER_SIDES` a texture's detail is fetched for.
    texture_tiers: usize,
    /// Whether a warm-up fetches the normal, specular and ambient occlusion
    /// maps, the viewer's material maps setting.
    material_maps: bool,
    in_flight: usize,
    detail_in_flight: usize,
    /// Whether the last file came from the cache.
    from_cache: bool,
    /// Bytes asked for and arrived since the queue was last empty.
    queued_bytes: u64,
    done_bytes: u64,
    stats: Stats,
}

impl Store {
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// A file the Cache API served; no request went out.
    fn count_stored(&mut self, size: u64) {
        self.stats.cached += size;
        self.from_cache = true;
    }

    /// Splits a finished fetch between network and cache.
    fn count_transfer(&mut self, url: &str, size: u64) {
        match crate::transfer_sizes(url) {
            Some((transfer, body)) => {
                self.stats.downloaded += transfer;
                // Less over the wire than the body: the cache had it, and at
                // most headers moved to revalidate it.
                self.from_cache = transfer < body;
                if self.from_cache {
                    self.stats.cached += body;
                }
            }
            // No Resource Timing: count it as downloaded.
            None => {
                self.stats.downloaded += size;
                self.from_cache = false;
            }
        }
    }

    pub fn progress(&self) -> Progress {
        if self.index.is_none() && !self.index_failed {
            Progress::Starting
        } else if self.queued_bytes == 0 {
            Progress::Idle
        } else {
            Progress::Bytes {
                done: self.done_bytes,
                total: self.queued_bytes,
            }
        }
    }

    /// The manifest of an actor as the stream names it.
    pub fn actor(&mut self, name: &str) -> Get<Rc<Manifest>> {
        let Some(index) = &self.index else {
            return if self.index_failed {
                Get::Missing
            } else {
                Get::Pending
            };
        };
        let Some(file) = index.get(name).copied() else {
            return Get::Missing;
        };
        get(
            &mut self.actors,
            &mut self.queue,
            &mut self.queued_bytes,
            Kind::Actor,
            file,
        )
    }

    /// A terrain's texture and mapping, as the terrain resource names it.
    pub fn terrain(&self, name: &str) -> Get<TerrainTexture> {
        if self.index.is_none() {
            return if self.index_failed {
                Get::Missing
            } else {
                Get::Pending
            };
        }
        match self.terrains.get(name) {
            Some(terrain) => Get::Ready(*terrain),
            None => Get::Missing,
        }
    }

    /// The blend mask atlas's file.
    pub fn alpha_maps(&self) -> Get<FileRef> {
        if self.index.is_none() {
            return if self.index_failed {
                Get::Missing
            } else {
                Get::Pending
            };
        }
        match self.alpha_maps {
            Some(file) => Get::Ready(file),
            None => Get::Missing,
        }
    }

    /// A water type's normal map frames, as the terrain resource names
    /// the type.
    pub fn water(&self, kind: &str) -> Get<Vec<FileRef>> {
        if self.index.is_none() {
            return if self.index_failed {
                Get::Missing
            } else {
                Get::Pending
            };
        }
        match self.waters.get(kind) {
            Some(frames) => Get::Ready(frames.clone()),
            None => Get::Missing,
        }
    }

    /// The shore waves' textures: foam.png, then coastalWave.png.
    pub fn wave_textures(&self) -> Get<(FileRef, FileRef)> {
        if self.index.is_none() {
            return if self.index_failed {
                Get::Missing
            } else {
                Get::Pending
            };
        }
        match (self.foam, self.coastal_wave) {
            (Some(foam), Some(coastal_wave)) => Get::Ready((foam, coastal_wave)),
            _ => Get::Missing,
        }
    }

    /// A sky set's cube layers.
    pub fn sky(&self, name: &str) -> Get<[FileRef; 6]> {
        if self.index.is_none() {
            return if self.index_failed {
                Get::Missing
            } else {
                Get::Pending
            };
        }
        match self.skies.get(name) {
            Some(layers) => Get::Ready(*layers),
            None => Get::Missing,
        }
    }

    /// A sound group as the stream names it, relative to audio/.
    pub fn sound_group(&mut self, name: &str) -> Get<Rc<SoundGroup>> {
        if self.index.is_none() {
            return if self.index_failed {
                Get::Missing
            } else {
                Get::Pending
            };
        }
        let Some(file) = self.sound_groups.get(name).copied() else {
            return Get::Missing;
        };
        get(
            &mut self.sound_group_files,
            &mut self.queue,
            &mut self.queued_bytes,
            Kind::SoundGroup,
            file,
        )
    }

    /// A civ's tracks, `None` before the index arrives; the empty code
    /// gives the defaults.
    pub fn music(&self, civ: &str) -> Option<&[(MusicKind, FileRef)]> {
        self.index.as_ref()?;
        Some(self.music.get(civ).map_or(&[], Vec::as_slice))
    }

    /// How a local entity of `template` sinks, its filters left out;
    /// `None` before the index or for a template without `Decay`.
    pub fn sinking(&self, template: &str) -> Option<Sinking> {
        let name = template.rsplit('|').next().unwrap_or(template);
        self.sinking.get(name).copied()
    }

    /// Where an `<img>` finds the portrait of Identity's `icon`; `None`
    /// before the index or for an icon the bake has none for.
    pub fn portrait_url(&self, icon: &str) -> Option<String> {
        let file = self.portraits.get(icon)?;
        Some(format!("/assets/{}", Kind::Portrait.file_name(file.hash)))
    }

    /// A queued technology's portrait, as `portrait_url`.
    pub fn technology_portrait_url(&self, technology: &str) -> Option<String> {
        self.portrait_url(self.technology_icons.get(technology)?)
    }

    /// The session's ambient loop, `None` before the index or without one.
    pub fn ambient(&self) -> Option<FileRef> {
        self.ambient
    }

    /// Names the asset bake, so what is derived from its files can be
    /// told from what an earlier bake's gave.
    /// What a warm-up of these civs, actors and terrains starts from:
    /// actor manifests, sound groups and terrain textures, the alpha maps
    /// last. `None` before the index arrives.
    pub fn warmup_roots(
        &self,
        civs: &[&str],
        extra_actors: &[&str],
        terrains: &[String],
    ) -> Option<Vec<(Kind, FileRef)>> {
        let actors = self.index.as_ref()?;
        let mut roots = Vec::new();
        roots.extend(
            extra_actors
                .iter()
                .filter_map(|name| actors.get(*name))
                .map(|file| (Kind::Actor, *file)),
        );
        for assets in civs.iter().filter_map(|civ| self.civs.get(*civ)) {
            roots.extend(
                assets
                    .actors
                    .iter()
                    .filter_map(|name| actors.get(name))
                    .map(|file| (Kind::Actor, *file)),
            );
            roots.extend(
                assets
                    .sound_groups
                    .iter()
                    .filter_map(|name| self.sound_groups.get(name))
                    .map(|file| (Kind::SoundGroup, *file)),
            );
        }
        roots.extend(
            terrains
                .iter()
                .filter_map(|name| self.terrains.get(name))
                .flat_map(|terrain| {
                    let maps = if self.material_maps {
                        [terrain.normal, terrain.specular]
                    } else {
                        [None; 2]
                    };
                    [Some(terrain.texture), maps[0], maps[1]]
                        .into_iter()
                        .flatten()
                        .map(|file| (Kind::Texture, file))
                }),
        );
        roots.extend(self.alpha_maps.map(|file| (Kind::Texture, file)));
        Some(roots)
    }

    pub fn index_name(&self) -> Option<&str> {
        self.index_name.as_deref()
    }

    /// The coarse mesh until its detail has arrived, which is fetched when
    /// `detail` is asked for.
    pub fn mesh(&mut self, file: FileRef, detail: bool) -> Get<Rc<Mesh>> {
        let got = get(
            &mut self.meshes,
            &mut self.queue,
            &mut self.queued_bytes,
            Kind::Mesh,
            file,
        );
        if let Get::Ready(mesh) = &got
            && detail
            && let Some(rest) = mesh.detail
        {
            self.ask_detail(Kind::MeshDetail, rest, file.hash);
        }
        got
    }

    pub fn skeleton(&mut self, file: FileRef) -> Get<Rc<Skeleton>> {
        get(
            &mut self.skeletons,
            &mut self.queue,
            &mut self.queued_bytes,
            Kind::Skeleton,
            file,
        )
    }

    pub fn animation(&mut self, file: FileRef) -> Get<Rc<AnimationData>> {
        get(
            &mut self.animations,
            &mut self.queue,
            &mut self.queued_bytes,
            Kind::Animation,
            file,
        )
    }

    pub fn texture_tiers(&self) -> usize {
        self.texture_tiers
    }

    /// Raising it fetches the larger tiers the next time a texture is
    /// asked for with its detail; lowering it keeps what has arrived.
    pub fn set_texture_tiers(&mut self, tiers: usize) {
        self.texture_tiers = tiers.min(TEXTURE_TIER_SIDES.len());
    }

    pub fn material_maps(&self) -> bool {
        self.material_maps
    }

    pub fn set_material_maps(&mut self, on: bool) {
        self.material_maps = on;
    }

    /// The smallest mips until the rest has arrived, which is fetched,
    /// one tier at a time up to `texture_tiers`, when `detail` is asked
    /// for.
    pub fn texture(&mut self, file: FileRef, detail: bool) -> Get<Rc<Texture>> {
        let got = get(
            &mut self.textures,
            &mut self.queue,
            &mut self.queued_bytes,
            Kind::Texture,
            file,
        );
        if let Get::Ready(texture) = &got
            && detail
            && let Some((_, rest)) = texture.next_detail(self.texture_tiers)
        {
            self.ask_detail(Kind::TextureDetail, rest, file.hash);
        }
        got
    }

    fn ask_detail(&mut self, kind: Kind, file: FileRef, coarse: u64) {
        if self.detail_asked.insert(file.hash) {
            self.detail_queue.push_back(Job { kind, file, coarse });
            self.queued_bytes += u64::from(file.size);
        }
    }

    fn arrive(&mut self, job: Job, decoded: Result<Decoded, String>) {
        let Job { kind, file, coarse } = job;
        self.in_flight -= 1;
        if matches!(kind, Kind::MeshDetail | Kind::TextureDetail) {
            self.detail_in_flight -= 1;
        }
        self.done_bytes += u64::from(file.size);
        let hash = file.hash;
        let decoded = decoded.and_then(|decoded| self.refine(coarse, hash, decoded));
        if decoded.is_ok() {
            self.stats.files += 1;
            self.stats.bytes += u64::from(file.size);
        }
        match decoded {
            Ok(Decoded::Actor(value)) => {
                self.actors.insert(hash, Slot::Ready(value));
            }
            Ok(Decoded::Mesh(value)) => {
                self.meshes.insert(hash, Slot::Ready(value));
            }
            Ok(Decoded::Skeleton(value)) => {
                self.skeletons.insert(hash, Slot::Ready(value));
            }
            Ok(Decoded::Animation(value)) => {
                self.animations.insert(hash, Slot::Ready(value));
            }
            Ok(Decoded::Texture(value)) => {
                self.textures.insert(hash, Slot::Ready(value));
            }
            Ok(Decoded::SoundGroup(value)) => {
                self.sound_group_files.insert(hash, Slot::Ready(value));
            }
            // Merged by `refine`.
            Ok(Decoded::MeshDetail(_) | Decoded::TextureDetail(_)) => {}
            Err(error) => {
                crate::log(&format!("asset {}: {error}", kind.file_name(hash)));
                match kind {
                    Kind::Actor => fail(&mut self.actors, hash),
                    Kind::Mesh => fail(&mut self.meshes, hash),
                    Kind::Skeleton => fail(&mut self.skeletons, hash),
                    Kind::Animation => fail(&mut self.animations, hash),
                    Kind::Texture => fail(&mut self.textures, hash),
                    Kind::SoundGroup => fail(&mut self.sound_group_files, hash),
                    // The coarse value stays, and is not asked again; sounds
                    // are fetched by the audio and portraits by an `<img>`,
                    // not here.
                    Kind::MeshDetail
                    | Kind::TextureDetail
                    | Kind::Index
                    | Kind::Sound
                    | Kind::Portrait => {}
                }
            }
        }
        if self.in_flight == 0 && self.queue.is_empty() && self.detail_queue.is_empty() {
            // The next burst of loads gets a bar of its own.
            self.queued_bytes = 0;
            self.done_bytes = 0;
        }
    }
}

impl Store {
    /// Puts a detail file into the coarse value it completes.
    fn refine(&mut self, coarse: u64, file: u64, decoded: Decoded) -> Result<Decoded, String> {
        match decoded {
            Decoded::MeshDetail(detail) => {
                if let Some(Slot::Ready(mesh)) = self.meshes.get(&coarse) {
                    let full = mesh.refine(&detail)?;
                    self.meshes.insert(coarse, Slot::Ready(Rc::new(full)));
                }
                Ok(Decoded::MeshDetail(detail))
            }
            Decoded::TextureDetail(detail) => {
                if let Some(Slot::Ready(texture)) = self.textures.get(&coarse)
                    && let Some(tier) = texture
                        .details
                        .iter()
                        .position(|detail| detail.is_some_and(|detail| detail.hash == file))
                {
                    let full = texture.refine(tier, &detail)?;
                    self.textures.insert(coarse, Slot::Ready(Rc::new(full)));
                }
                Ok(Decoded::TextureDetail(detail))
            }
            other => Ok(other),
        }
    }
}

fn fail<T>(slots: &mut HashMap<u64, Slot<T>>, hash: u64) {
    slots.insert(hash, Slot::Failed);
}

fn get<T: Clone>(
    slots: &mut HashMap<u64, Slot<T>>,
    queue: &mut VecDeque<Job>,
    queued_bytes: &mut u64,
    kind: Kind,
    file: FileRef,
) -> Get<T> {
    match slots.get(&file.hash) {
        Some(Slot::Ready(value)) => Get::Ready(value.clone()),
        Some(Slot::Pending) => Get::Pending,
        Some(Slot::Failed) => Get::Missing,
        None => {
            slots.insert(file.hash, Slot::Pending);
            queue.push_back(Job {
                kind,
                file,
                coarse: file.hash,
            });
            *queued_bytes += u64::from(file.size);
            Get::Pending
        }
    }
}

pub fn new() -> Rc<RefCell<Store>> {
    // The default keeps 250 entries, and the cache statistics read one per
    // asset fetched.
    if let Some(performance) = web_sys::window().and_then(|window| window.performance()) {
        performance.set_resource_timing_buffer_size(1_000_000);
    }
    let store = Rc::new(RefCell::new(Store {
        texture_tiers: TEXTURE_TIER_SIDES.len(),
        material_maps: false,
        ..Store::default()
    }));
    wasm_bindgen_futures::spawn_local(load_index(Rc::clone(&store)));
    store
}

/// Starts queued downloads while there is room; called every frame, after
/// the frame's requests.
pub fn pump(store: &Rc<RefCell<Store>>) {
    let mut started = Vec::new();
    {
        let mut state = store.borrow_mut();
        let limit = if state.from_cache {
            MAX_CACHED_IN_FLIGHT
        } else {
            MAX_IN_FLIGHT
        };
        while state.in_flight < limit {
            let next = match state.queue.pop_front() {
                Some(job) => job,
                None if state.detail_in_flight < MAX_DETAIL_IN_FLIGHT => {
                    let Some(job) = state.detail_queue.pop_front() else {
                        break;
                    };
                    state.detail_in_flight += 1;
                    job
                }
                None => break,
            };
            state.in_flight += 1;
            started.push(next);
        }
    }
    for job in started {
        wasm_bindgen_futures::spawn_local(fetch(Rc::clone(store), job));
    }
}

async fn fetch(store: Rc<RefCell<Store>>, job: Job) {
    let url = format!("/assets/{}", job.kind.file_name(job.file.hash));
    let bytes = crate::stored::fetch(&url).await;
    let bytes = bytes.map(|(bytes, stored)| {
        let size = u64::from(job.file.size);
        let mut state = store.borrow_mut();
        if stored {
            state.count_stored(size);
        } else {
            state.count_transfer(&url, size);
        }
        bytes
    });
    let decoded = bytes.and_then(|bytes| decode(job.kind, &bytes).map_err(String::from));
    store.borrow_mut().arrive(job, decoded);
    pump(&store);
}

fn decode(kind: Kind, bytes: &[u8]) -> Result<Decoded, &'static str> {
    Ok(match kind {
        Kind::Actor => Decoded::Actor(Rc::new(Manifest::decode(bytes)?)),
        Kind::Mesh => Decoded::Mesh(Rc::new(Mesh::decode(bytes)?)),
        Kind::Skeleton => Decoded::Skeleton(Rc::new(Skeleton::decode(bytes)?)),
        Kind::Animation => Decoded::Animation(Rc::new(AnimationData::decode(bytes)?)),
        Kind::Texture => Decoded::Texture(Rc::new(Texture::decode(bytes)?)),
        Kind::MeshDetail => Decoded::MeshDetail(MeshDetail::decode(bytes)?),
        Kind::TextureDetail => Decoded::TextureDetail(TextureDetail::decode(bytes)?),
        Kind::SoundGroup => Decoded::SoundGroup(Rc::new(SoundGroup::decode(bytes)?)),
        Kind::Index => return Err("the index is not fetched by hash"),
        Kind::Sound => return Err("sounds are decoded by the browser"),
        Kind::Portrait => return Err("portraits are decoded by the browser"),
    })
}

async fn load_index(store: Rc<RefCell<Store>>) {
    match fetch_index(&store).await {
        Ok((index, name)) => {
            crate::log(&format!(
                "assets: {} actors, {} terrains, {} sound groups",
                index.actors.len(),
                index.terrains.len(),
                index.sound_groups.len()
            ));
            {
                let mut store = store.borrow_mut();
                store.index = Some(index.actors.into_iter().collect());
                store.terrains = index.terrains.into_iter().collect();
                store.alpha_maps = index.alpha_maps;
                store.waters = index.waters.into_iter().collect();
                store.foam = index.foam;
                store.coastal_wave = index.coastal_wave;
                store.skies = index.skies.into_iter().collect();
                store.sound_groups = index.sound_groups.into_iter().collect();
                store.music = index.music.into_iter().collect();
                store.ambient = index.ambient;
                store.civs = index.civs.into_iter().collect();
                store.sinking = index.sinking.into_iter().collect();
                store.portraits = index.portraits.into_iter().collect();
                store.technology_icons = index.technology_icons.into_iter().collect();
                store.index_name = Some(name.clone());
            }
            crate::stored::prune(&name).await;
        }
        // Not fatal: the map and the minimap colours still show.
        Err(error) => {
            web_sys::console::error_1(&format!("assets: {error}").into());
            store.borrow_mut().index_failed = true;
        }
    }
}

async fn fetch_index(store: &Rc<RefCell<Store>>) -> Result<(Index, String), String> {
    let bytes = crate::fetch_bytes(POINTER)
        .await
        .map_err(|error| format!("{POINTER}: {error} (run bake-assets)"))?;
    let name = std::str::from_utf8(&bytes)
        .map_err(|_| format!("{POINTER} is not UTF-8"))?
        .trim();
    // The name goes into a URL; anything else is not bake-assets'.
    let valid = name.starts_with("assets/index-")
        && name.ends_with(".bin")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-./".contains(&byte));
    if !valid {
        return Err(format!("{POINTER} names {name:?}, not an index"));
    }
    let url = format!("/{name}");
    let (bytes, stored) = crate::stored::fetch(&url)
        .await
        .map_err(|error| format!("{url}: {error}"))?;
    if stored {
        store.borrow_mut().count_stored(bytes.len() as u64);
    } else {
        store.borrow_mut().count_transfer(&url, bytes.len() as u64);
    }
    let index = Index::decode(&bytes).map_err(|error| format!("/{name}: {error}"))?;
    Ok((index, name.to_string()))
}
