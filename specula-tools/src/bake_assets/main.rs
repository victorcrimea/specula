//! Bakes every actor of the game into the split asset files the observer
//! page streams (format in specula-assets' `files.rs`): one manifest per
//! actor holding all of its variants and quality levels, and meshes,
//! skeletons, animations and textures as separate files named by their
//! content, so pieces shared by many actors are fetched once. Also every
//! terrain's diffuse texture and the blend mask atlas for the ground, and
//! every sound group, the session music and the ambient loop, their Ogg
//! Vorbis files transcoded to Ogg Opus.
//!
//! Usage: bake-assets [output dir]
//! Default: specula-web. MOD_DIR overrides the mod,
//! ../0ad/binaries/data/mods/public; PYROGENESIS overrides the engine,
//! ../0ad-specula/binaries/system/pyrogenesis, which converts the meshes,
//! animations and textures. Transcoded sounds are cached in
//! target/bake-assets/sounds by their source's content.
//!
//! The files go to `<output dir>/assets/`; `<output dir>/assets.txt`
//! names the index and is written last. Next to the index goes the list
//! of every file in the bake, and next to each file but the sounds (Opus
//! does not shrink) its brotli copy `<name>.br`, which the server sends to
//! browsers that take it. A brotli copy left by an earlier bake is kept, as
//! the name says its content. Files of earlier bakes that this one did not
//! write are removed.

mod actor;
mod mesh;
mod sound;
mod templates;
mod terrain;
mod texture;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Instant;

use specula_assets::files;
use specula_assets::files::AnimationData;
use specula_assets::files::Bone;
use specula_assets::files::CivAssets;
use specula_assets::files::FileRef;
use specula_assets::files::Index;
use specula_assets::files::Kind;
use specula_assets::files::Manifest;
use specula_assets::files::MusicKind;
use specula_assets::files::Object;
use specula_assets::files::SoundGroup;
use specula_assets::files::TerrainTexture;
use specula_tools::convert;
use specula_tools::convert::cached_path;
use specula_tools::pmd;
use specula_tools::psa;

use crate::actor::ActorDef;
use crate::actor::Loader;

const DEFAULT_MOD: &str = "../0ad/binaries/data/mods/public";
const DEFAULT_PYROGENESIS: &str = "../0ad-specula/binaries/system/pyrogenesis";
const DEFAULT_OUTPUT: &str = "specula-web";
const ASSET_DIR: &str = "assets";
const POINTER: &str = "assets.txt";
const WORK_DIR: &str = "target/bake-assets";
/// Highest quality: each file is compressed once, by the first bake that
/// writes it, and sent many times.
const BROTLI_QUALITY: u32 = 11;
const BROTLI_WINDOW_BITS: u32 = 22;
const PARTIAL_SUFFIX: &str = ".partial";
const SOUND_CACHE_DIR: &str = "sounds";
/// Opus bits per second per channel: speech and effects are short and
/// mostly mono, the music is listened to.
const EFFECT_BITRATE: i32 = 32_000;
const MUSIC_BITRATE: i32 = 48_000;
const MUSIC_PREFIX: &str = "audio/music/";
/// The engine draws `projectile` props only when something is shot.
const PROJECTILE_POINT: &str = "projectile";
/// For the summary: how the split compares with bake-unit's 4.4 MB VAT
/// bake of the same unit.
const SAMPLE_ACTOR: &str = "units/athenians/infantry_spearman_b.xml";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Converted pieces by their source path; `None` when the engine could not
/// convert it.
#[derive(Default)]
struct Pieces {
    meshes: HashMap<String, Option<FileRef>>,
    textures: HashMap<String, Option<FileRef>>,
    animations: HashMap<String, Option<FileRef>>,
}

/// Writes content-named files once and remembers what it wrote.
struct Output {
    dir: PathBuf,
    written: HashSet<String>,
    bytes: u64,
    hashes: BTreeSet<u64>,
    /// Names to write a brotli copy of.
    compress: Vec<String>,
}

impl Output {
    fn write(&mut self, kind: Kind, bytes: &[u8]) -> Result<FileRef, String> {
        // Only names need to differ when the content does, so a hash that
        // is not stable across Rust versions is fine.
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let hash = hasher.finish();
        let name = kind.file_name(hash);
        if self.written.insert(name.clone()) {
            let path = self.dir.join(&name);
            fs::write(&path, bytes)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
            self.bytes += bytes.len() as u64;
            self.hashes.insert(hash);
            if kind != Kind::Sound {
                self.compress.push(name);
            }
        }
        Ok(FileRef {
            hash,
            size: bytes.len() as u32,
        })
    }
}

fn run() -> Result<(), String> {
    let start = Instant::now();
    let output_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| DEFAULT_OUTPUT.into()),
    );
    let mod_dir = PathBuf::from(std::env::var("MOD_DIR").unwrap_or_else(|_| DEFAULT_MOD.into()));
    let pyrogenesis =
        PathBuf::from(std::env::var("PYROGENESIS").unwrap_or_else(|_| DEFAULT_PYROGENESIS.into()));

    let mut names = Vec::new();
    list_actors(&mod_dir.join("art/actors"), "", &mut names)?;
    names.sort();
    let mut loader = Loader::new(&mod_dir);
    for name in &names {
        // Failures are kept by the loader and reported below.
        let _ = loader.load(name);
    }
    let failed: Vec<(&String, &String)> = loader
        .actors()
        .iter()
        .filter_map(|(name, result)| result.as_ref().err().map(|error| (name, error)))
        .collect();
    println!(
        "{} actor files, {} failed to load",
        names.len(),
        failed.len()
    );
    for (_, error) in &failed {
        println!("  {error}");
    }

    let (terrains, unreadable) = terrain::load(&mod_dir);
    println!(
        "{} terrains, {} could not be read",
        terrains.len(),
        unreadable.len()
    );
    for error in &unreadable {
        println!("  {error}");
    }

    let terrain_textures: Vec<String> = terrains
        .iter()
        .map(|terrain| terrain.base_texture.clone())
        .collect();
    let (sources, rejected) = collect_sources(&mod_dir, loader.actors(), &terrain_textures);
    println!("{} sources left out", rejected.len());
    for reason in &rejected {
        println!("  {reason}");
    }
    println!("converting {} files with the engine", sources.len());
    let (converted, missing) =
        convert::run(&mod_dir, &pyrogenesis, &PathBuf::from(WORK_DIR), &sources)?;
    println!(
        "converted in {:.0} s; the engine dropped {}",
        start.elapsed().as_secs_f32(),
        missing.len()
    );
    for source in &missing {
        println!("  {source}");
    }

    let asset_dir = output_dir.join(ASSET_DIR);
    fs::create_dir_all(&asset_dir)
        .map_err(|error| format!("cannot create {}: {error}", asset_dir.display()))?;
    let mut output = Output {
        dir: asset_dir.clone(),
        written: HashSet::new(),
        bytes: 0,
        hashes: BTreeSet::new(),
        compress: Vec::new(),
    };
    let missing: HashSet<String> = missing.into_iter().collect();
    let pieces = write_pieces(&converted, &sources, &missing, &mut output)?;

    let mut index = Index {
        actors: Vec::new(),
        terrains: Vec::new(),
        alpha_maps: None,
        sound_groups: Vec::new(),
        music: Vec::new(),
        ambient: None,
        civs: Vec::new(),
    };
    let mut skipped = Vec::new();
    for name in &names {
        let Ok(actor) = loader.actors()[name].clone() else {
            continue;
        };
        match manifest(name, &actor, loader.actors(), &pieces) {
            Ok(manifest) => {
                let file = output.write(Kind::Actor, &manifest.encode())?;
                index.actors.push((name.clone(), file));
            }
            Err(error) => skipped.push(error),
        }
    }
    println!(
        "{} actors baked, {} skipped for missing pieces",
        index.actors.len(),
        skipped.len()
    );
    for error in &skipped {
        println!("  {error}");
    }
    for terrain in &terrains {
        if let Some(texture) = pieces
            .textures
            .get(&terrain.base_texture)
            .copied()
            .flatten()
        {
            index.terrains.push((
                terrain.tag.clone(),
                TerrainTexture {
                    texture,
                    angle: terrain.angle,
                    size: terrain.size,
                },
            ));
        }
    }
    println!(
        "{} terrains baked, {} without a texture",
        index.terrains.len(),
        terrains.len() - index.terrains.len()
    );
    let alpha_maps = terrain::alpha_maps(&mod_dir)?;
    index.alpha_maps = Some(output.write(Kind::Texture, &alpha_maps.encode())?);
    write_sounds(&mod_dir, &mut index, &mut output)?;
    add_civs(&mod_dir, &mut index);
    let index_file = output.write(Kind::Index, &index.encode())?;
    print_sample(&index, &asset_dir);
    write_file_list(&mut output, index_file.hash)?;
    let compressed = compress_all(&asset_dir, &output.compress)?;
    output.written.extend(compressed);
    remove_stale(&asset_dir, &output.written)?;

    let pointer = output_dir.join(POINTER);
    let name = format!("{ASSET_DIR}/{}", Kind::Index.file_name(index_file.hash));
    fs::write(&pointer, format!("{name}\n"))
        .map_err(|error| format!("cannot write {}: {error}", pointer.display()))?;
    println!(
        "wrote {} files, {:.1} MB, index {name}, in {:.0} s",
        output.written.len(),
        output.bytes as f64 / 1e6,
        start.elapsed().as_secs_f32()
    );
    Ok(())
}

/// The sound groups, music and ambient loop into `index`. A sound that is
/// missing or cannot be decoded is left out of its group, as
/// `CSoundGroup::Reload` skips a file it cannot load.
fn write_sounds(mod_dir: &Path, index: &mut Index, output: &mut Output) -> Result<(), String> {
    let start = Instant::now();
    let (groups, errors) = sound::load_groups(mod_dir);
    println!(
        "{} sound groups, {} could not be read",
        groups.len(),
        errors.len()
    );
    for error in &errors {
        println!("  {error}");
    }
    let (civ_music, errors) = sound::load_music(mod_dir);
    for error in &errors {
        println!("  {error}");
    }
    let default_music: Vec<(MusicKind, String)> = sound::DEFAULT_TRACKS
        .iter()
        .map(|(kind, file)| (*kind, format!("{MUSIC_PREFIX}{file}")))
        .collect();
    let mut sources: BTreeSet<String> = groups
        .iter()
        .flat_map(|(_, group)| group.files.iter().cloned())
        .collect();
    sources.extend(
        civ_music
            .iter()
            .flat_map(|(_, tracks)| tracks.iter())
            .chain(&default_music)
            .map(|(_, path)| path.clone()),
    );
    sources.insert(sound::AMBIENT.to_string());
    let sources: Vec<String> = sources.into_iter().collect();
    let encoded = transcode_all(mod_dir, &sources)?;
    let mut files: HashMap<String, FileRef> = HashMap::new();
    let mut failures = Vec::new();
    for (source, result) in sources.iter().zip(encoded) {
        match result {
            Ok(bytes) => {
                files.insert(source.clone(), output.write(Kind::Sound, &bytes)?);
            }
            Err(error) => failures.push(format!("{source}: {error}")),
        }
    }
    println!(
        "{} sounds transcoded in {:.0} s, {} could not be",
        files.len(),
        start.elapsed().as_secs_f32(),
        failures.len()
    );
    for failure in &failures {
        println!("  {failure}");
    }

    for (name, group) in &groups {
        let sounds: Vec<FileRef> = group
            .files
            .iter()
            .filter_map(|path| files.get(path).copied())
            .collect();
        if sounds.is_empty() {
            continue;
        }
        let manifest = SoundGroup {
            flags: group.flags,
            gain: group.gain,
            pitch: group.pitch,
            gain_lower: group.gain_lower,
            gain_upper: group.gain_upper,
            pitch_lower: group.pitch_lower,
            pitch_upper: group.pitch_upper,
            sounds,
        };
        index.sound_groups.push((
            name.clone(),
            output.write(Kind::SoundGroup, &manifest.encode())?,
        ));
    }
    let playlist = |tracks: &[(MusicKind, String)]| -> Vec<(MusicKind, FileRef)> {
        tracks
            .iter()
            .filter_map(|(kind, path)| files.get(path).map(|file| (*kind, *file)))
            .collect()
    };
    index.music.push((String::new(), playlist(&default_music)));
    for (civ, tracks) in &civ_music {
        index.music.push((civ.clone(), playlist(tracks)));
    }
    index.ambient = files.get(sound::AMBIENT).copied();
    println!(
        "{} sound groups baked, music for {} civs",
        index.sound_groups.len(),
        civ_music.len()
    );
    Ok(())
}

/// Each civ's actors and sound groups into `index`, those that were baked.
fn add_civs(mod_dir: &Path, index: &mut Index) {
    let (civs, errors) = templates::load(mod_dir);
    println!("{} civs, {} templates unresolved", civs.len(), errors.len());
    for error in &errors {
        println!("  {error}");
    }
    let actors: HashSet<&str> = index.actors.iter().map(|(name, _)| name.as_str()).collect();
    let groups: HashSet<&str> = index
        .sound_groups
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let mut missing = BTreeSet::new();
    let mut baked = Vec::new();
    for (civ, refs) in civs {
        let (kept_actors, lost): (Vec<String>, Vec<String>) = refs
            .actors
            .into_iter()
            .partition(|actor| actors.contains(actor.as_str()));
        missing.extend(lost.into_iter().map(|actor| format!("actor {actor}")));
        let (kept_groups, lost): (Vec<String>, Vec<String>) = refs
            .sound_groups
            .into_iter()
            .partition(|group| groups.contains(group.as_str()));
        missing.extend(lost.into_iter().map(|group| format!("sound group {group}")));
        println!(
            "  {civ}: {} actors, {} sound groups",
            kept_actors.len(),
            kept_groups.len()
        );
        baked.push((
            civ,
            CivAssets {
                actors: kept_actors,
                sound_groups: kept_groups,
            },
        ));
    }
    index.civs = baked;
    println!("{} civ references not in the bake", missing.len());
    for name in &missing {
        println!("  {name}");
    }
}

/// A sound as Ogg Opus, or why it could not be.
type Transcoded = Result<Vec<u8>, String>;

/// Each source as Ogg Opus, in order, on every core; a source transcoded
/// before at the same bitrate comes from the cache.
fn transcode_all(mod_dir: &Path, sources: &[String]) -> Result<Vec<Transcoded>, String> {
    let cache = PathBuf::from(WORK_DIR).join(SOUND_CACHE_DIR);
    fs::create_dir_all(&cache)
        .map_err(|error| format!("cannot create {}: {error}", cache.display()))?;
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Transcoded>>> =
        Mutex::new((0..sources.len()).map(|_| None).collect());
    let workers = thread::available_parallelism().map_or(1, |count| count.get());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some(source) = sources.get(at) else {
                        break;
                    };
                    let result = transcode_cached(mod_dir, &cache, source);
                    results.lock().unwrap()[at] = Some(result);
                }
            });
        }
    });
    Ok(results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|result| result.unwrap_or_else(|| Err("not transcoded".into())))
        .collect())
}

fn transcode_cached(mod_dir: &Path, cache: &Path, source: &str) -> Result<Vec<u8>, String> {
    let bytes = fs::read(mod_dir.join(source)).map_err(|error| error.to_string())?;
    if bytes.starts_with(b"version https://git-lfs") {
        return Err("a Git LFS pointer, the file was not fetched".into());
    }
    let bitrate = if source.starts_with(MUSIC_PREFIX) {
        MUSIC_BITRATE
    } else {
        EFFECT_BITRATE
    };
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    bitrate.hash(&mut hasher);
    let cached = cache.join(format!("{:016x}.opus", hasher.finish()));
    if let Ok(encoded) = fs::read(&cached) {
        return Ok(encoded);
    }
    let encoded = sound::transcode(&bytes, bitrate)?;
    // A failed cache write only costs the next bake the transcode.
    let _ = fs::write(&cached, &encoded);
    Ok(encoded)
}

/// Actor files under `dir`, relative to art/actors as the stream names
/// them.
fn list_actors(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        if entry.path().is_dir() {
            list_actors(&entry.path(), &format!("{path}/"), out)?;
        } else if name.ends_with(".xml") {
            out.push(path);
        }
    }
    Ok(())
}

/// Every mesh, animation and colour map the actors name, the terrain
/// textures, and the textures.xml files whose settings the converter
/// applies to them.
/// Also returns the sources left out, with why: the archive builder fails
/// an `ENSURE` on a texture it cannot convert, so those must not reach it.
fn collect_sources(
    mod_dir: &Path,
    actors: &BTreeMap<String, Result<Rc<ActorDef>, String>>,
    terrain_textures: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut sources: BTreeSet<String> = terrain_textures.iter().cloned().collect();
    for actor in actors.values().flatten() {
        for variant in actor
            .definitions
            .iter()
            .flat_map(|definition| definition.groups.iter().flatten())
        {
            sources.extend(variant.mesh.iter().cloned());
            sources.extend(variant.base_texture.iter().cloned());
            sources.extend(
                variant
                    .animations
                    .iter()
                    .filter_map(|animation| animation.file.clone()),
            );
        }
    }
    // Missing sources would fail the staging copy; a missing mesh or
    // texture is the engine's load error too, so it shows as dropped.
    let mut rejected = Vec::new();
    let mut present = BTreeSet::new();
    for source in sources {
        match unusable(&mod_dir.join(&source), &source) {
            Some(reason) => rejected.push(format!("{source}: {reason}")),
            None => {
                present.insert(source);
            }
        }
    }
    let settings: Vec<String> = present
        .iter()
        .filter(|source| source.starts_with("art/textures/"))
        .flat_map(|source| {
            let mut dirs = Vec::new();
            let mut dir = Path::new(source.as_str()).parent();
            while let Some(path) = dir
                && path.starts_with("art/textures")
            {
                dirs.push(format!("{}/textures.xml", path.display()));
                dir = path.parent();
            }
            dirs
        })
        .filter(|settings| mod_dir.join(settings).is_file())
        .collect();
    present.extend(settings);
    (present.into_iter().collect(), rejected)
}

/// Why the engine could not convert `source`, checked from its first
/// bytes: a Git LFS pointer whose object was never fetched, or a texture
/// `CTextureConverter::ConvertTexture` refuses (not PNG or DDS, or not a
/// power of two on each side).
fn unusable(path: &Path, source: &str) -> Option<String> {
    let mut head = [0u8; 32];
    let read = fs::File::open(path).and_then(|mut file| file.read(&mut head));
    let Ok(length) = read else {
        return Some("missing".into());
    };
    let head = &head[..length];
    if head.starts_with(b"version https://git-lfs") {
        return Some("a Git LFS pointer, the file was not fetched".into());
    }
    if !source.starts_with("art/textures/") {
        return None;
    }
    let (width, height) = if head.starts_with(b"\x89PNG") && head.len() >= 24 {
        let be = |at: usize| u32::from_be_bytes(head[at..at + 4].try_into().unwrap());
        (be(16), be(20))
    } else if head.starts_with(b"DDS ") && head.len() >= 20 {
        let le = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap());
        (le(16), le(12))
    } else {
        return Some("neither PNG nor DDS".into());
    };
    if !width.is_power_of_two() || !height.is_power_of_two() {
        return Some(format!("{width}x{height} is not a power of two"));
    }
    None
}

fn write_pieces(
    converted: &Path,
    sources: &[String],
    missing: &HashSet<String>,
    output: &mut Output,
) -> Result<Pieces, String> {
    let mut pieces = Pieces::default();
    let mut failures = Vec::new();
    for source in sources {
        let is_mesh = source.starts_with("art/meshes/");
        let is_animation = source.starts_with("art/animation/");
        let is_texture = source.starts_with("art/textures/") && !source.ends_with("textures.xml");
        if !(is_mesh || is_animation || is_texture) {
            continue;
        }
        let file = if missing.contains(source) {
            None
        } else {
            let result = fs::read(cached_path(converted, source))
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    if is_mesh {
                        write_mesh(source, &bytes, output)
                    } else if is_animation {
                        write_animation(&bytes, output)
                    } else {
                        write_texture(&bytes, output)
                    }
                });
            match result {
                Ok(file) => Some(file),
                Err(error) => {
                    failures.push(format!("{source}: {error}"));
                    None
                }
            }
        };
        let map = if is_mesh {
            &mut pieces.meshes
        } else if is_animation {
            &mut pieces.animations
        } else {
            &mut pieces.textures
        };
        map.insert(source.clone(), file);
    }
    println!(
        "{} meshes, {} animations, {} textures; {} could not be read",
        pieces.meshes.len(),
        pieces.animations.len(),
        pieces.textures.len(),
        failures.len()
    );
    for failure in &failures {
        println!("  {failure}");
    }
    Ok(pieces)
}

fn write_mesh(source: &str, bytes: &[u8], output: &mut Output) -> Result<FileRef, String> {
    let model = pmd::load(bytes)?;
    let (mut mesh, skeleton) = mesh::convert(source, &model)?;
    if let Some(skeleton) = skeleton {
        mesh.skeleton = Some(output.write(Kind::Skeleton, &skeleton.encode())?);
    }
    let (mut coarse, detail) = mesh.split().map_err(|error| format!("{source}: {error}"))?;
    if let Some(detail) = detail {
        coarse.detail = Some(output.write(Kind::MeshDetail, &detail.encode())?);
    }
    output.write(Kind::Mesh, &coarse.encode())
}

fn write_texture(bytes: &[u8], output: &mut Output) -> Result<FileRef, String> {
    let texture = texture::convert(bytes)?;
    let (mut coarse, detail) = texture.split(texture::coarse_level(&texture));
    if let Some(detail) = detail {
        coarse.detail = Some(output.write(Kind::TextureDetail, &detail.encode())?);
    }
    output.write(Kind::Texture, &coarse.encode())
}

fn write_animation(bytes: &[u8], output: &mut Output) -> Result<FileRef, String> {
    let animation = psa::load(bytes)?;
    let data = AnimationData {
        frame_ms: animation.frame_time,
        keys: animation.keys as u32,
        frames: animation.frames as u32,
        states: animation
            .states
            .iter()
            .map(|state| Bone {
                translation: [
                    state.translation.x,
                    state.translation.y,
                    state.translation.z,
                ],
                rotation: [
                    state.rotation.x,
                    state.rotation.y,
                    state.rotation.z,
                    state.rotation.w,
                ],
            })
            .collect(),
    };
    output.write(Kind::Animation, &data.encode())
}

/// The actor and every actor it can prop, transitively. A mesh the engine
/// could not convert fails the actor, as `BuildVariation` fails without
/// its model; a prop actor that failed, either way, is left out, as
/// `CObjectEntry::BuildVariation` skips a prop it cannot build.
fn manifest(
    name: &str,
    actor: &Rc<ActorDef>,
    actors: &BTreeMap<String, Result<Rc<ActorDef>, String>>,
    pieces: &Pieces,
) -> Result<Manifest, String> {
    if let Some(path) = missing_mesh(actor, pieces) {
        return Err(format!("{name}: {path} did not convert"));
    }
    let mut order: Vec<(String, Rc<ActorDef>)> = vec![(name.to_string(), Rc::clone(actor))];
    let mut index_of: HashMap<String, u32> = HashMap::from([(name.to_string(), 0)]);
    let mut next = 0;
    while next < order.len() {
        let current = Rc::clone(&order[next].1);
        next += 1;
        for variant in current
            .definitions
            .iter()
            .flat_map(|definition| definition.groups.iter().flatten())
        {
            for (point, prop) in &variant.props {
                if prop.is_empty() || point == PROJECTILE_POINT || index_of.contains_key(prop) {
                    continue;
                }
                if let Some(Ok(prop_actor)) = actors.get(prop)
                    && missing_mesh(prop_actor, pieces).is_none()
                {
                    index_of.insert(prop.clone(), order.len() as u32);
                    order.push((prop.clone(), Rc::clone(prop_actor)));
                }
            }
        }
    }

    let mut objects = Vec::with_capacity(order.len());
    for (object_name, def) in &order {
        let mut definitions = Vec::with_capacity(def.definitions.len());
        for definition in &def.definitions {
            let mut groups = Vec::with_capacity(definition.groups.len());
            for group in &definition.groups {
                let mut variants = Vec::with_capacity(group.len());
                for variant in group {
                    // Every object here has all its meshes.
                    let mesh = variant
                        .mesh
                        .as_ref()
                        .and_then(|path| pieces.meshes.get(path).copied().flatten());
                    let texture = variant
                        .base_texture
                        .as_ref()
                        .and_then(|path| pieces.textures.get(path).copied().flatten());
                    let props = variant
                        .props
                        .iter()
                        .filter(|(point, _)| point != PROJECTILE_POINT)
                        .map(|(point, prop)| files::Prop {
                            point: point.clone(),
                            object: index_of.get(prop).copied().unwrap_or(files::NONE),
                        })
                        .collect();
                    // An animation whose file did not convert is dropped by
                    // `BuildAnimation` but still overrides its name.
                    let animations = variant
                        .animations
                        .iter()
                        .map(|animation| files::Animation {
                            name: animation.name.clone(),
                            id: animation.id.clone(),
                            frequency: animation.frequency,
                            speed: animation.speed,
                            event: animation.event,
                            load: animation.load,
                            sound: animation.sound,
                            file: animation
                                .file
                                .as_ref()
                                .and_then(|path| pieces.animations.get(path).copied().flatten()),
                        })
                        .collect();
                    variants.push(files::Variant {
                        name: variant.name.clone(),
                        frequency: variant.frequency,
                        mesh,
                        texture,
                        decal: variant.decal,
                        props,
                        animations,
                    });
                }
                groups.push(variants);
            }
            definitions.push(files::Definition {
                player_color: definition.material.player_color,
                alpha_ref: if definition.material.transparent {
                    // `REQUIRE_ALPHA_GEQUAL` of the opaque pass in
                    // shaders/effects/model_transparent.xml.
                    0.6375
                } else {
                    0.0
                },
                groups,
            });
        }
        objects.push(Object {
            name: object_name.clone(),
            definitions,
            bases: def
                .bases
                .iter()
                .map(|(quality, definition)| (*quality, *definition as u32))
                .collect(),
        });
    }
    Ok(Manifest { objects })
}

/// A mesh of the actor the engine could not convert, if any.
fn missing_mesh<'a>(actor: &'a ActorDef, pieces: &Pieces) -> Option<&'a String> {
    actor
        .definitions
        .iter()
        .flat_map(|definition| definition.groups.iter().flatten())
        .filter_map(|variant| variant.mesh.as_ref())
        .find(|path| pieces.meshes.get(*path).copied().flatten().is_none())
}

/// What one unit costs to fetch in full, to compare with bake-unit, and
/// what its coarse files alone cost.
fn print_sample(index: &Index, asset_dir: &Path) {
    let Some((_, file)) = index.actors.iter().find(|(name, _)| name == SAMPLE_ACTOR) else {
        return;
    };
    let read = |kind: Kind, hash: u64| fs::read(asset_dir.join(kind.file_name(hash))).ok();
    let Some(manifest) =
        read(Kind::Actor, file.hash).and_then(|bytes| Manifest::decode(&bytes).ok())
    else {
        return;
    };
    let mut seen = HashSet::new();
    let mut coarse = u64::from(file.size);
    let mut total = coarse;
    let mut counts = [0usize; 3];
    for variant in manifest
        .objects
        .iter()
        .flat_map(|object| object.definitions.iter())
        .flat_map(|definition| definition.groups.iter().flatten())
    {
        let animations = variant
            .animations
            .iter()
            .filter_map(|animation| animation.file);
        for (slot, file) in variant
            .mesh
            .map(|file| (0, file))
            .into_iter()
            .chain(variant.texture.map(|file| (1, file)))
            .chain(animations.map(|file| (2, file)))
        {
            if !seen.insert(file.hash) {
                continue;
            }
            coarse += u64::from(file.size);
            total += u64::from(file.size);
            counts[slot] += 1;
            let detail = match slot {
                0 => read(Kind::Mesh, file.hash)
                    .and_then(|bytes| files::Mesh::decode(&bytes).ok())
                    .and_then(|mesh| mesh.detail),
                1 => read(Kind::Texture, file.hash)
                    .and_then(|bytes| files::Texture::decode(&bytes).ok())
                    .and_then(|texture| texture.detail),
                _ => None,
            };
            total += detail.map_or(0, |detail| u64::from(detail.size));
        }
    }
    println!(
        "{SAMPLE_ACTOR}: {} objects, every variant: {} meshes, {} textures, {} animations, {:.2} MB in all, {:.2} MB coarse",
        manifest.objects.len(),
        counts[0],
        counts[1],
        counts[2],
        total as f64 / 1e6,
        coarse as f64 / 1e6
    );
}

/// The list `files::file_list_name` names, itself listed for compression
/// but not in the list.
fn write_file_list(output: &mut Output, index: u64) -> Result<(), String> {
    let name = files::file_list_name(index);
    let bytes: Vec<u8> = output
        .hashes
        .iter()
        .flat_map(|hash| hash.to_le_bytes())
        .collect();
    let path = output.dir.join(&name);
    fs::write(&path, &bytes)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    output.bytes += bytes.len() as u64;
    output.written.insert(name.clone());
    output.compress.push(name);
    Ok(())
}

/// A brotli copy of each of `names` that would be smaller, on every core;
/// returns the names of the copies there now.
fn compress_all(dir: &Path, names: &[String]) -> Result<Vec<String>, String> {
    let start = Instant::now();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Result<Compressed, String>>> = Mutex::new(Vec::new());
    let workers = thread::available_parallelism().map_or(1, |count| count.get());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some(name) = names.get(at) else {
                        break;
                    };
                    let result = compress_file(dir, name);
                    results.lock().unwrap().push(result);
                }
            });
        }
    });
    let mut copies = Vec::new();
    let (mut kept, mut made, mut raw, mut packed) = (0, 0, 0, 0);
    for result in results.into_inner().unwrap() {
        match result? {
            Compressed::Kept(name) => {
                kept += 1;
                copies.push(name);
            }
            Compressed::Made {
                name,
                raw: from,
                packed: to,
            } => {
                made += 1;
                raw += from;
                packed += to;
                copies.push(name);
            }
            Compressed::Larger => {}
        }
    }
    println!(
        "brotli: {made} copies made ({:.1} MB to {:.1} MB), {kept} kept from earlier bakes, \
         {} left raw, in {:.0} s",
        raw as f64 / 1e6,
        packed as f64 / 1e6,
        names.len() - made - kept,
        start.elapsed().as_secs_f32()
    );
    Ok(copies)
}

enum Compressed {
    Kept(String),
    Made {
        name: String,
        raw: u64,
        packed: u64,
    },
    /// Not smaller; the server sends the file as it is.
    Larger,
}

fn compress_file(dir: &Path, name: &str) -> Result<Compressed, String> {
    let copy = format!("{name}.br");
    let path = dir.join(&copy);
    if path.exists() {
        return Ok(Compressed::Kept(copy));
    }
    let source = dir.join(name);
    let bytes =
        fs::read(&source).map_err(|error| format!("cannot read {}: {error}", source.display()))?;
    let mut writer =
        brotli::CompressorWriter::new(Vec::new(), 4096, BROTLI_QUALITY, BROTLI_WINDOW_BITS);
    writer
        .write_all(&bytes)
        .map_err(|error| format!("cannot compress {name}: {error}"))?;
    let packed = writer.into_inner();
    if packed.len() >= bytes.len() {
        return Ok(Compressed::Larger);
    }
    // Renamed into place, so an interrupted bake does not leave a cut copy
    // that the next one would keep.
    let partial = dir.join(format!("{copy}{PARTIAL_SUFFIX}"));
    fs::write(&partial, &packed)
        .map_err(|error| format!("cannot write {}: {error}", partial.display()))?;
    fs::rename(&partial, &path)
        .map_err(|error| format!("cannot rename to {}: {error}", path.display()))?;
    Ok(Compressed::Made {
        name: copy,
        raw: bytes.len() as u64,
        packed: packed.len() as u64,
    })
}

fn remove_stale(dir: &Path, written: &HashSet<String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours =
            name.ends_with(".bin") || name.ends_with(".bin.br") || name.ends_with(PARTIAL_SUFFIX);
        if ours && !written.contains(&name) {
            fs::remove_file(entry.path())
                .map_err(|error| format!("cannot remove {name}: {error}"))?;
        }
    }
    Ok(())
}
