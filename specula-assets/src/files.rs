//! The split asset files, format v14. Little-endian; every file starts with
//! a 4-byte magic and a u8 version. Strings are u16 length and UTF-8,
//! counts are u32. A file is served as `assets/<kind>-<16 hex>.bin`, the
//! hex being a hash of its content, so a cached copy is never stale and
//! pieces shared by many actors are downloaded once. `assets.txt` next to
//! `assets/` names the current index.
//!
//! Meshes and textures come in two files: a coarse one of a few KB that
//! can be drawn on its own, and a detail one it names that completes it,
//! so a unit shows after one round trip and far ones never need more.
//!
//! - index "SPCI": count x {actor path under art/actors, as the stream
//!   names actors, file ref of its manifest}, count x {terrain name, as
//!   the terrain resource names it, file ref of its baseTex texture, f32
//!   angle in radians, f32 size in metres, optional normTex texture,
//!   optional specTex texture, f32 normal strength and f32 specular power
//!   (its material's `effectSettings` x and y), u8 1 when its material
//!   maps it triplanar}, optional alpha map atlas.
//!   The atlas is an RGBA8 texture, one mip, holding the engine's 14
//!   standard blend masks in its alpha, side by side in
//!   `CTerrainTextureManager::LoadAlphaMap`'s order and layout. Then
//!   count x {water type, the directory under art/textures/animated/water
//!   the terrain resource names, count x texture: its normal maps,
//!   `normal0001.png` on, the frames `WaterManager` animates through},
//!   optional foam texture and optional coastal wave texture (the shore
//!   waves' `foam.png` and `coastalWave.png`), count x {sky set, the
//!   directory under art/textures/skies, 6 x texture: the cube's layers
//!   +X, -X, +Y, -Y, +Z, -Z as `SkyManager` uploads them, one RGBA8 mip
//!   each, rows already flipped and the two top faces turned}. Then
//!   count x {sound group path relative to audio/, as the stream names
//!   groups, file ref of its sound group}, count x {civ code (empty: the
//!   default playlists of gui/common/music.js), count x {u8 kind (0 peace,
//!   1 battle, 2 victory, 3 defeat), file ref of a sound}}, optional sound,
//!   the session's ambient loop. Then count x {civ code, count x actor
//!   path, count x sound group path}: the actors and sound groups the
//!   civ's templates can show or play, keys of the lists above, so a
//!   viewer can fetch a match's assets ahead of time. Then count x
//!   {template name, f32 delay in seconds, f32 sink rate in m/s, f32 sink
//!   acceleration in m/s^2, u8 1 when it sinks like a ship}: every
//!   template's `Decay`, how its corpse or rubble sinks and goes. Then
//!   count x {path under art/textures/ui/session/portraits, as Identity's
//!   Icon names it, file ref of a portrait}. Then count x {technology
//!   name, its path under simulation/data/technologies without `.json`,
//!   its icon's path in the portrait list}: what a queued technology
//!   shows. Then count x {entity template name, its Identity's Icon in
//!   the portrait list}: what a unit shows in the training and queue
//!   panels before any entity of it has been seen.
//! - actor manifest "SPCA": count x object. Object 0 is the actor, the rest
//!   every actor it can prop, transitively, so one fetch resolves an
//!   entity. An object is {name, count x definition, count x {u8 quality,
//!   u32 definition}}, its bases in rising quality as `CActorDef` keeps
//!   them, the last at 255. A definition is {u8 flags (1: player colour,
//!   2: normal map, 4: specular map, 8: ambient occlusion map, as its
//!   material's required textures define them, 16: swayed by `USE_WIND`),
//!   f32 alpha reference (0: none), f32x4 the material's `effectSettings`
//!   (0 where it names none), f32x2 its `windData` (0 where it names none
//!   or does not sway), count x group}; a group is count x variant; a variant is {name (lower
//!   case), i32 frequency, optional mesh, optional baseTex, normTex,
//!   specTex and aoTex textures, optional decal {f32 width, f32 depth,
//!   f32 angle in radians, f32 offset x, f32 offset z}, optional particle
//!   emitter, count x {prop point, u32 object (NONE: removes the point's
//!   props)}, count x {animation name, id, i32 frequency, f32 speed, f32
//!   event, f32 load and f32 sound (fractions, -1: none), optional
//!   animation file}}. Mesh and texture refs name coarse files.
//!   A particle emitter is `CParticleEmitterType` as loaded from
//!   art/particles: {optional texture, u8 blend (`BLEND_*`), u8 flags (1
//!   start full, 2 relative velocity), `PARTICLE_VARIABLES` x variable in
//!   `CParticleEmitterType`'s `VAR_*` order, f32x3 the summed force}; a
//!   variable is u8 kind and then 0 constant: f32 value; 1 uniform: f32
//!   min, f32 max; 2 copy: u8 variable; 3 expr: entity variable name, f32
//!   mul, f32 max.
//! - mesh "SPCM", the coarsest level of detail: u32 vertex count, vertices
//!   of `VERTEX_SIZE` bytes (f32x3 position, f32x3 normal, f32x2 uv, f32x2
//!   second uv (the first again where the PMD has one set), snorm8x4
//!   MikkTSpace tangent with the bitangent's sign in w, u8x4 bones,
//!   unorm8x4 weights; a bone equal to the bone count is the model's own
//!   transform), u32 index count, u16 indices, u8 level count,
//!   levels x {u32 first index, u32 count}, the finest first; optional
//!   skeleton, u32 bone count, prop point count x {name, f32x3 position,
//!   f32x4 rotation, u8 bone (0xFF: the model's origin)}, f32x3 bounds min
//!   and max of the bind pose of the full mesh, optional mesh detail.
//! - mesh detail "SPMD": u32 vertex count, vertices appended to the coarse
//!   ones, u32 index count, u16 indices appended to the coarse ones (and
//!   indexing the vertices of both), u8 level count, levels x {u32 first,
//!   u32 count} into both index lists, the full mesh first; the coarse
//!   level comes after them.
//! - skeleton "SPCS": the bind pose, count x {f32x3 translation, f32x4
//!   rotation}.
//! - animation "SPCN": f32 ms per frame, u32 keys (bones), u32 frames,
//!   frames x keys x {f32x3 translation, f16x4 rotation}, frame-major.
//! - texture "SPCT", the smallest mips: u8 format (`TextureFormat`), u32
//!   width, u32 height, u8 level count, levels x {u32 byte count, bytes},
//!   largest first; block rows and pixel rows top first; an optional
//!   texture detail per tier of `TEXTURE_TIER_SIDES`, smallest first.
//! - texture detail "SPTD": the same up to the levels, for the mips of
//!   one tier: from `Texture::first_level` of its side down to the one
//!   before the next smaller file's first.
//! - sound group "SPCG", `CSoundGroup` as loaded: u8 flags (1 omnipresent,
//!   2 distanceless, 4 heard by the owner only, 8 random gain, 16 random
//!   pitch), f32 gain, f32 pitch, f32 gain lower, f32 gain upper, f32
//!   pitch lower, f32 pitch upper, count x file ref of a sound (the files
//!   that could be read, in the group's order).
//! - sound: an Ogg Opus stream as is (RFC 7845), so a browser decodes it
//!   directly; it starts with Ogg's own "OggS" and version 0, not this
//!   format's version.
//! - portrait: a lossy WebP image as is, at most 128 px on a side, so a
//!   page shows it in an `<img>`; it starts with RIFF's own "RIFF" and a
//!   size, not this format's version.
//!
//! A file ref is {u64 hash, u32 byte size}; the size lets a viewer show
//! download progress before it has the files. An optional file ref is a
//! u8 0 or 1 and then the ref.

use crate::half::f16_bits;
use crate::half::f16_to_f32;

pub const VERSION: u8 = 14;
/// The largest side of each texture detail tier, which a viewer's
/// texture quality picks from: low, medium and the whole chain as the
/// engine uploads it at its default quality.
pub const TEXTURE_TIER_SIDES: [u32; 3] = [256, 1024, u32::MAX];
/// Bytes per mesh vertex.
pub const VERTEX_SIZE: usize = 52;
/// A prop that names no object only removes what was on its point.
pub const NONE: u32 = u32::MAX;
/// A prop point fixed to the model's origin rather than to a bone.
pub const NO_BONE: u8 = 0xFF;
pub const FLAG_PLAYER_COLOR: u8 = 1;
pub const FLAG_NORMAL_MAP: u8 = 2;
pub const FLAG_SPECULAR_MAP: u8 = 4;
pub const FLAG_AO_MAP: u8 = 8;
pub const FLAG_WIND: u8 = 16;
pub const BLEND_ADD: u8 = 0;
pub const BLEND_SUBTRACT: u8 = 1;
pub const BLEND_OVER: u8 = 2;
pub const BLEND_MULTIPLY: u8 = 3;
/// `CParticleEmitterType::VAR__MAX`.
pub const PARTICLE_VARIABLES: usize = 15;
pub const VAR_EMISSION_RATE: usize = 0;
pub const VAR_LIFETIME: usize = 1;
pub const VAR_POSITION_X: usize = 2;
pub const VAR_POSITION_Y: usize = 3;
pub const VAR_POSITION_Z: usize = 4;
pub const VAR_ANGLE: usize = 5;
pub const VAR_VELOCITY_X: usize = 6;
pub const VAR_VELOCITY_Y: usize = 7;
pub const VAR_VELOCITY_Z: usize = 8;
pub const VAR_VELOCITY_ANGLE: usize = 9;
pub const VAR_SIZE: usize = 10;
pub const VAR_SIZE_GROWTH_RATE: usize = 11;
pub const VAR_COLOR_R: usize = 12;
pub const VAR_COLOR_G: usize = 13;
pub const VAR_COLOR_B: usize = 14;
const EMITTER_START_FULL: u8 = 1;
const EMITTER_RELATIVE_VELOCITY: u8 = 2;
const VARIABLE_CONSTANT: u8 = 0;
const VARIABLE_UNIFORM: u8 = 1;
const VARIABLE_COPY: u8 = 2;
const VARIABLE_EXPR: u8 = 3;
pub const SOUND_OMNIPRESENT: u8 = 1;
pub const SOUND_DISTANCELESS: u8 = 2;
pub const SOUND_OWNER_ONLY: u8 = 4;
pub const SOUND_RANDOM_GAIN: u8 = 8;
pub const SOUND_RANDOM_PITCH: u8 = 16;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Index,
    Actor,
    Mesh,
    MeshDetail,
    Skeleton,
    Animation,
    Texture,
    TextureDetail,
    SoundGroup,
    Sound,
    Portrait,
}

impl Kind {
    fn magic(self) -> &'static [u8; 4] {
        match self {
            Kind::Index => b"SPCI",
            Kind::Actor => b"SPCA",
            Kind::Mesh => b"SPCM",
            Kind::MeshDetail => b"SPMD",
            Kind::Skeleton => b"SPCS",
            Kind::Animation => b"SPCN",
            Kind::Texture => b"SPCT",
            Kind::TextureDetail => b"SPTD",
            Kind::SoundGroup => b"SPCG",
            Kind::Sound => b"OggS",
            Kind::Portrait => b"RIFF",
        }
    }

    pub fn prefix(self) -> &'static str {
        match self {
            Kind::Index => "index",
            Kind::Actor => "actor",
            Kind::Mesh => "mesh",
            Kind::MeshDetail => "meshdetail",
            Kind::Skeleton => "skeleton",
            Kind::Animation => "anim",
            Kind::Texture => "texture",
            Kind::TextureDetail => "texturedetail",
            Kind::SoundGroup => "soundgroup",
            Kind::Sound => "sound",
            Kind::Portrait => "portrait",
        }
    }

    /// The file's name inside `assets/`.
    pub fn file_name(self, hash: u64) -> String {
        format!("{}-{hash:016x}.bin", self.prefix())
    }
}

/// The name inside `assets/` of the list of every file in the bake whose
/// index has this hash: the hashes in their names, as little-endian u64s.
/// A client keeping files from earlier bakes drops what is not in it.
pub fn file_list_name(index: u64) -> String {
    format!("files-{index:016x}.bin")
}

/// The hash in the name of a file `Kind::file_name` made.
pub fn hash_of_name(name: &str) -> Option<u64> {
    let (_, hash) = name.strip_suffix(".bin")?.rsplit_once('-')?;
    u64::from_str_radix(hash, 16).ok()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FileRef {
    pub hash: u64,
    pub size: u32,
}

pub struct Index {
    pub actors: Vec<(String, FileRef)>,
    pub terrains: Vec<(String, TerrainTexture)>,
    pub alpha_maps: Option<FileRef>,
    /// Water type -> its normal map frames.
    pub waters: Vec<(String, Vec<FileRef>)>,
    pub foam: Option<FileRef>,
    pub coastal_wave: Option<FileRef>,
    /// Sky set -> its cube layers.
    pub skies: Vec<(String, [FileRef; 6])>,
    pub sound_groups: Vec<(String, FileRef)>,
    /// By civ code; the empty code holds the defaults.
    pub music: Vec<(String, Vec<(MusicKind, FileRef)>)>,
    pub ambient: Option<FileRef>,
    pub civs: Vec<(String, CivAssets)>,
    /// By template name, without filters.
    pub sinking: Vec<(String, Sinking)>,
    /// By Identity's Icon, relative to art/textures/ui/session/portraits.
    pub portraits: Vec<(String, FileRef)>,
    /// Technology -> its icon, a key of `portraits`.
    pub technology_icons: Vec<(String, String)>,
    /// Entity template -> its `Identity/Icon`, a key of `portraits`, for
    /// the templates a viewer has not yet seen an entity of.
    pub template_icons: Vec<(String, String)>,
}

/// A template's `Decay`, as `CCmpDecay` sinks a local entity of it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sinking {
    /// Seconds before it starts to sink.
    pub delay: f32,
    /// m/s, and m/s^2.
    pub rate: f32,
    pub acceleration: f32,
    /// `SinkingAnim`: sinks tilting, as a ship.
    pub ship: bool,
}

/// What a civ's templates can show and play, as keys of the index's
/// actors and sound groups.
#[derive(Default)]
pub struct CivAssets {
    pub actors: Vec<String>,
    pub sound_groups: Vec<String>,
}

/// The music.js states a session plays.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MusicKind {
    Peace,
    Battle,
    Victory,
    Defeat,
}

impl MusicKind {
    fn code(self) -> u8 {
        match self {
            MusicKind::Peace => 0,
            MusicKind::Battle => 1,
            MusicKind::Victory => 2,
            MusicKind::Defeat => 3,
        }
    }

    fn from_code(code: u8) -> Result<Self, &'static str> {
        match code {
            0 => Ok(MusicKind::Peace),
            1 => Ok(MusicKind::Battle),
            2 => Ok(MusicKind::Victory),
            3 => Ok(MusicKind::Defeat),
            _ => Err("unknown music kind"),
        }
    }
}

/// `CSoundGroup`'s settings, with its sounds.
pub struct SoundGroup {
    /// `SOUND_*` bits.
    pub flags: u8,
    pub gain: f32,
    pub pitch: f32,
    pub gain_lower: f32,
    pub gain_upper: f32,
    pub pitch_lower: f32,
    pub pitch_upper: f32,
    pub sounds: Vec<FileRef>,
}

/// `CTerrainTextureEntry`'s textures and its planar mapping.
#[derive(Clone, Copy, Debug)]
pub struct TerrainTexture {
    pub texture: FileRef,
    /// Radians.
    pub angle: f32,
    /// Metres the texture spans.
    pub size: f32,
    /// Only where the material requires it.
    pub normal: Option<FileRef>,
    pub specular: Option<FileRef>,
    pub normal_strength: f32,
    pub specular_power: f32,
    /// World-space triplanar instead of the planar mapping
    /// (`USE_TRIPLANAR`).
    pub triplanar: bool,
}

pub struct Manifest {
    pub objects: Vec<Object>,
}

pub struct Object {
    pub name: String,
    pub definitions: Vec<Definition>,
    /// (quality, definition), rising; the last is 255.
    pub bases: Vec<(u8, u32)>,
}

pub struct Definition {
    /// `FLAG_*` bits.
    pub flags: u8,
    pub alpha_ref: f32,
    /// `effectSettings`: normal strength, specular power, parallax scale,
    /// and how much of the vertex lighting replaces the per-pixel one.
    pub effect: [f32; 4],
    /// `windData`, which `FLAG_WIND` sways the model by.
    pub wind: [f32; 2],
    pub groups: Vec<Vec<Variant>>,
}

pub struct Variant {
    pub name: String,
    pub frequency: i32,
    pub mesh: Option<FileRef>,
    pub texture: Option<FileRef>,
    pub normal_texture: Option<FileRef>,
    pub specular_texture: Option<FileRef>,
    pub ao_texture: Option<FileRef>,
    pub decal: Option<Decal>,
    pub particles: Option<Emitter>,
    pub props: Vec<Prop>,
    pub animations: Vec<Animation>,
}

/// `CParticleEmitterType`: what an actor variant's `<particles>` emits.
#[derive(Clone, PartialEq, Debug)]
pub struct Emitter {
    /// `None` where the engine would draw its error texture.
    pub texture: Option<FileRef>,
    /// `BLEND_*`.
    pub blend: u8,
    /// Created as if it had been emitting for its longest lifetime.
    pub start_full: bool,
    /// The velocity turns with the emitter.
    pub relative_velocity: bool,
    /// By `VAR_*`.
    pub variables: [ParticleVariable; PARTICLE_VARIABLES],
    /// Every `<force>`, summed: an acceleration in m/s^2.
    pub force: [f32; 3],
}

/// How a particle variable is evaluated for each new particle.
#[derive(Clone, PartialEq, Debug)]
pub enum ParticleVariable {
    Constant(f32),
    /// `[min, max)`; min below max.
    Uniform(f32, f32),
    /// The value another variable gave this particle; a `VAR_*`.
    Copy(u8),
    /// `min(max, entity variable * mul)`.
    Expr {
        from: String,
        mul: f32,
        max: f32,
    },
}

/// `CObjectBase::Decal`: a texture laid on the terrain under the object.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Decal {
    /// Metres, along the object's x and z.
    pub size_x: f32,
    pub size_z: f32,
    /// Radians.
    pub angle: f32,
    pub offset_x: f32,
    pub offset_z: f32,
}

pub struct Prop {
    pub point: String,
    /// Index into `Manifest::objects`, or `NONE`.
    pub object: u32,
}

pub struct Animation {
    /// As the actor spells it; the engine lower-cases it when it builds
    /// the variation.
    pub name: String,
    pub id: String,
    pub frequency: i32,
    pub speed: f32,
    /// Fractions of the duration, -1 for none.
    pub event: f32,
    pub load: f32,
    pub sound: f32,
    pub file: Option<FileRef>,
}

pub struct Mesh {
    pub vertex_count: u32,
    /// `VERTEX_SIZE` bytes each, ready for a vertex buffer.
    pub vertices: Vec<u8>,
    pub indices: Vec<u16>,
    /// First index and count, the finest first.
    pub levels: Vec<(u32, u32)>,
    pub skeleton: Option<FileRef>,
    pub bone_count: u32,
    pub prop_points: Vec<PropPoint>,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// What completes a coarse mesh; `None` once refined, or when the
    /// coarse level is all there is.
    pub detail: Option<FileRef>,
}

/// The vertices and indices a coarse mesh lacks.
pub struct MeshDetail {
    pub vertex_count: u32,
    pub vertices: Vec<u8>,
    pub indices: Vec<u16>,
    /// Into the coarse indices followed by these, the full mesh first.
    pub levels: Vec<(u32, u32)>,
}

#[derive(Clone)]
pub struct PropPoint {
    pub name: String,
    pub position: [f32; 3],
    /// x, y, z, w.
    pub rotation: [f32; 4],
    pub bone: u8,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Bone {
    pub translation: [f32; 3],
    /// x, y, z, w.
    pub rotation: [f32; 4],
}

pub struct Skeleton {
    pub bones: Vec<Bone>,
}

pub struct AnimationData {
    pub frame_ms: f32,
    pub keys: u32,
    pub frames: u32,
    /// `frames * keys`, frame-major.
    pub states: Vec<Bone>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextureFormat {
    Rgba8,
    Bc1,
    Bc2,
    Bc3,
}

pub struct Texture {
    pub format: TextureFormat,
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
    /// What completes a coarse texture, per tier of `TEXTURE_TIER_SIDES`;
    /// `None` once refined with it, or when that tier has no levels.
    pub details: [Option<FileRef>; 3],
}

/// The mips of one tier, larger than a coarse texture's first.
pub struct TextureDetail {
    pub format: TextureFormat,
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
}

impl Index {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(Kind::Index);
        out.count(self.actors.len());
        for (name, manifest) in &self.actors {
            out.string(name);
            out.file(*manifest);
        }
        out.count(self.terrains.len());
        for (name, terrain) in &self.terrains {
            out.string(name);
            out.file(terrain.texture);
            out.f32(terrain.angle);
            out.f32(terrain.size);
            out.optional_file(terrain.normal);
            out.optional_file(terrain.specular);
            out.f32(terrain.normal_strength);
            out.f32(terrain.specular_power);
            out.u8(u8::from(terrain.triplanar));
        }
        out.optional_file(self.alpha_maps);
        out.count(self.waters.len());
        for (name, frames) in &self.waters {
            out.string(name);
            out.count(frames.len());
            for frame in frames {
                out.file(*frame);
            }
        }
        out.optional_file(self.foam);
        out.optional_file(self.coastal_wave);
        out.count(self.skies.len());
        for (name, layers) in &self.skies {
            out.string(name);
            for layer in layers {
                out.file(*layer);
            }
        }
        out.count(self.sound_groups.len());
        for (name, group) in &self.sound_groups {
            out.string(name);
            out.file(*group);
        }
        out.count(self.music.len());
        for (civ, tracks) in &self.music {
            out.string(civ);
            out.count(tracks.len());
            for (kind, track) in tracks {
                out.u8(kind.code());
                out.file(*track);
            }
        }
        out.optional_file(self.ambient);
        out.count(self.civs.len());
        for (civ, assets) in &self.civs {
            out.string(civ);
            out.count(assets.actors.len());
            for actor in &assets.actors {
                out.string(actor);
            }
            out.count(assets.sound_groups.len());
            for group in &assets.sound_groups {
                out.string(group);
            }
        }
        out.count(self.sinking.len());
        for (template, sinking) in &self.sinking {
            out.string(template);
            out.f32s(&[sinking.delay, sinking.rate, sinking.acceleration]);
            out.u8(u8::from(sinking.ship));
        }
        out.count(self.portraits.len());
        for (icon, portrait) in &self.portraits {
            out.string(icon);
            out.file(*portrait);
        }
        out.count(self.technology_icons.len());
        for (technology, icon) in &self.technology_icons {
            out.string(technology);
            out.string(icon);
        }
        out.count(self.template_icons.len());
        for (template, icon) in &self.template_icons {
            out.string(template);
            out.string(icon);
        }
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let mut input = Reader::new(bytes, Kind::Index)?;
        let actors = (0..input.count()?)
            .map(|_| Ok((input.string()?, input.file()?)))
            .collect::<Result<_, &'static str>>()?;
        let terrains = (0..input.count()?)
            .map(|_| {
                let name = input.string()?;
                let texture = input.file()?;
                let angle = input.f32()?;
                let size = input.f32()?;
                let normal = input.optional_file()?;
                let specular = input.optional_file()?;
                let normal_strength = input.f32()?;
                let specular_power = input.f32()?;
                let triplanar = input.u8()? != 0;
                Ok((
                    name,
                    TerrainTexture {
                        texture,
                        angle,
                        size,
                        normal,
                        specular,
                        normal_strength,
                        specular_power,
                        triplanar,
                    },
                ))
            })
            .collect::<Result<_, &'static str>>()?;
        let alpha_maps = input.optional_file()?;
        let waters = (0..input.count()?)
            .map(|_| {
                let name = input.string()?;
                let frames = (0..input.count()?)
                    .map(|_| input.file())
                    .collect::<Result<_, &'static str>>()?;
                Ok((name, frames))
            })
            .collect::<Result<_, &'static str>>()?;
        let foam = input.optional_file()?;
        let coastal_wave = input.optional_file()?;
        let skies = (0..input.count()?)
            .map(|_| {
                let name = input.string()?;
                let mut layers = [FileRef { hash: 0, size: 0 }; 6];
                for layer in &mut layers {
                    *layer = input.file()?;
                }
                Ok((name, layers))
            })
            .collect::<Result<_, &'static str>>()?;
        let sound_groups = (0..input.count()?)
            .map(|_| Ok((input.string()?, input.file()?)))
            .collect::<Result<_, &'static str>>()?;
        let music = (0..input.count()?)
            .map(|_| {
                let civ = input.string()?;
                let tracks = (0..input.count()?)
                    .map(|_| Ok((MusicKind::from_code(input.u8()?)?, input.file()?)))
                    .collect::<Result<_, &'static str>>()?;
                Ok((civ, tracks))
            })
            .collect::<Result<_, &'static str>>()?;
        let ambient = input.optional_file()?;
        let civs = (0..input.count()?)
            .map(|_| {
                let civ = input.string()?;
                let actors = (0..input.count()?)
                    .map(|_| input.string())
                    .collect::<Result<_, &'static str>>()?;
                let sound_groups = (0..input.count()?)
                    .map(|_| input.string())
                    .collect::<Result<_, &'static str>>()?;
                Ok((
                    civ,
                    CivAssets {
                        actors,
                        sound_groups,
                    },
                ))
            })
            .collect::<Result<_, &'static str>>()?;
        let sinking = (0..input.count()?)
            .map(|_| {
                let template = input.string()?;
                let [delay, rate, acceleration] = input.f32s()?;
                let ship = input.u8()? != 0;
                Ok((
                    template,
                    Sinking {
                        delay,
                        rate,
                        acceleration,
                        ship,
                    },
                ))
            })
            .collect::<Result<_, &'static str>>()?;
        let portraits = (0..input.count()?)
            .map(|_| Ok((input.string()?, input.file()?)))
            .collect::<Result<_, &'static str>>()?;
        let technology_icons = (0..input.count()?)
            .map(|_| Ok((input.string()?, input.string()?)))
            .collect::<Result<_, &'static str>>()?;
        let template_icons = (0..input.count()?)
            .map(|_| Ok((input.string()?, input.string()?)))
            .collect::<Result<_, &'static str>>()?;
        input.finish()?;
        Ok(Index {
            actors,
            terrains,
            alpha_maps,
            waters,
            foam,
            coastal_wave,
            skies,
            sound_groups,
            music,
            ambient,
            civs,
            sinking,
            portraits,
            technology_icons,
            template_icons,
        })
    }
}

impl SoundGroup {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(Kind::SoundGroup);
        out.u8(self.flags);
        out.f32s(&[
            self.gain,
            self.pitch,
            self.gain_lower,
            self.gain_upper,
            self.pitch_lower,
            self.pitch_upper,
        ]);
        out.count(self.sounds.len());
        for sound in &self.sounds {
            out.file(*sound);
        }
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let mut input = Reader::new(bytes, Kind::SoundGroup)?;
        let flags = input.u8()?;
        let [
            gain,
            pitch,
            gain_lower,
            gain_upper,
            pitch_lower,
            pitch_upper,
        ] = input.f32s()?;
        let sounds = (0..input.count()?)
            .map(|_| input.file())
            .collect::<Result<_, &'static str>>()?;
        input.finish()?;
        Ok(SoundGroup {
            flags,
            gain,
            pitch,
            gain_lower,
            gain_upper,
            pitch_lower,
            pitch_upper,
            sounds,
        })
    }
}

impl Manifest {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(Kind::Actor);
        out.count(self.objects.len());
        for object in &self.objects {
            out.string(&object.name);
            out.count(object.definitions.len());
            for definition in &object.definitions {
                out.u8(definition.flags);
                out.f32(definition.alpha_ref);
                out.f32s(&definition.effect);
                out.f32s(&definition.wind);
                out.count(definition.groups.len());
                for group in &definition.groups {
                    out.count(group.len());
                    for variant in group {
                        out.variant(variant);
                    }
                }
            }
            out.count(object.bases.len());
            for (quality, definition) in &object.bases {
                out.u8(*quality);
                out.u32(*definition);
            }
        }
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let mut input = Reader::new(bytes, Kind::Actor)?;
        let mut objects = Vec::new();
        for _ in 0..input.count()? {
            let name = input.string()?;
            let mut definitions = Vec::new();
            for _ in 0..input.count()? {
                let flags = input.u8()?;
                let alpha_ref = input.f32()?;
                let effect = input.f32s()?;
                let wind = input.f32s()?;
                let mut groups = Vec::new();
                for _ in 0..input.count()? {
                    let group = (0..input.count()?)
                        .map(|_| input.variant())
                        .collect::<Result<_, _>>()?;
                    groups.push(group);
                }
                definitions.push(Definition {
                    flags,
                    alpha_ref,
                    effect,
                    wind,
                    groups,
                });
            }
            let bases: Vec<(u8, u32)> = (0..input.count()?)
                .map(|_| Ok((input.u8()?, input.u32()?)))
                .collect::<Result<_, &'static str>>()?;
            let valid = bases.last().is_some_and(|(quality, _)| *quality == 255)
                && bases
                    .iter()
                    .all(|(_, definition)| (*definition as usize) < definitions.len());
            if !valid {
                return Err("actor bases out of range");
            }
            objects.push(Object {
                name,
                definitions,
                bases,
            });
        }
        input.finish()?;
        let props_valid = objects.iter().all(|object| {
            object.definitions.iter().all(|definition| {
                definition.groups.iter().flatten().all(|variant| {
                    variant
                        .props
                        .iter()
                        .all(|prop| prop.object == NONE || (prop.object as usize) < objects.len())
                })
            })
        });
        if objects.is_empty() || !props_valid {
            return Err("actor props out of range");
        }
        Ok(Manifest { objects })
    }
}

impl Mesh {
    /// A full mesh whose coarsest level is the last, at index 0, and uses
    /// only the vertices before all others, as the coarse file it starts
    /// and the detail that completes it. The coarse file's detail ref is
    /// left for the caller, who writes the detail first to learn it.
    pub fn split(&self) -> Result<(Mesh, Option<MeshDetail>), &'static str> {
        let Some(&(first, count)) = self.levels.last() else {
            return Err("mesh has no levels");
        };
        if first != 0 {
            return Err("the coarsest level does not start the indices");
        }
        let coarse_indices = &self.indices[..count as usize];
        let coarse_vertices = coarse_indices
            .iter()
            .map(|index| u32::from(*index) + 1)
            .max()
            .unwrap_or(0);
        let coarse = Mesh {
            vertex_count: coarse_vertices,
            vertices: self.vertices[..coarse_vertices as usize * VERTEX_SIZE].to_vec(),
            indices: coarse_indices.to_vec(),
            levels: vec![(0, count)],
            skeleton: self.skeleton,
            bone_count: self.bone_count,
            prop_points: self.prop_points.clone(),
            min: self.min,
            max: self.max,
            detail: None,
        };
        let complete = coarse_vertices == self.vertex_count
            && count as usize == self.indices.len()
            && self.levels.len() == 1;
        if complete {
            return Ok((coarse, None));
        }
        let detail = MeshDetail {
            vertex_count: self.vertex_count - coarse_vertices,
            vertices: self.vertices[coarse_vertices as usize * VERTEX_SIZE..].to_vec(),
            indices: self.indices[count as usize..].to_vec(),
            levels: self.levels[..self.levels.len() - 1].to_vec(),
        };
        Ok((coarse, Some(detail)))
    }

    /// The full mesh: this coarse one completed by its detail.
    pub fn refine(&self, detail: &MeshDetail) -> Result<Mesh, &'static str> {
        let vertex_count = self.vertex_count + detail.vertex_count;
        let mut vertices = self.vertices.clone();
        vertices.extend_from_slice(&detail.vertices);
        let mut indices = self.indices.clone();
        indices.extend_from_slice(&detail.indices);
        let mut levels = detail.levels.clone();
        levels.extend_from_slice(&self.levels);
        check_mesh(vertex_count, &indices, &levels)?;
        Ok(Mesh {
            vertex_count,
            vertices,
            indices,
            levels,
            skeleton: self.skeleton,
            bone_count: self.bone_count,
            prop_points: self.prop_points.clone(),
            min: self.min,
            max: self.max,
            detail: None,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(Kind::Mesh);
        out.vertices(self.vertex_count, &self.vertices, &self.indices);
        out.levels(&self.levels);
        out.optional_file(self.skeleton);
        out.u32(self.bone_count);
        out.count(self.prop_points.len());
        for point in &self.prop_points {
            out.string(&point.name);
            out.f32s(&point.position);
            out.f32s(&point.rotation);
            out.u8(point.bone);
        }
        out.f32s(&self.min);
        out.f32s(&self.max);
        out.optional_file(self.detail);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let mut input = Reader::new(bytes, Kind::Mesh)?;
        let (vertex_count, vertices, indices) = input.vertices()?;
        let levels = input.levels()?;
        check_mesh(vertex_count, &indices, &levels)?;
        let skeleton = input.optional_file()?;
        let bone_count = input.u32()?;
        let prop_points = (0..input.count()?)
            .map(|_| {
                Ok(PropPoint {
                    name: input.string()?,
                    position: input.f32s()?,
                    rotation: input.f32s()?,
                    bone: input.u8()?,
                })
            })
            .collect::<Result<_, &'static str>>()?;
        let min = input.f32s()?;
        let max = input.f32s()?;
        let detail = input.optional_file()?;
        input.finish()?;
        Ok(Mesh {
            vertex_count,
            vertices,
            indices,
            levels,
            skeleton,
            bone_count,
            prop_points,
            min,
            max,
            detail,
        })
    }
}

impl MeshDetail {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(Kind::MeshDetail);
        out.vertices(self.vertex_count, &self.vertices, &self.indices);
        out.levels(&self.levels);
        out.0
    }

    /// Indices and levels are checked against the coarse mesh by
    /// `Mesh::refine`.
    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let mut input = Reader::new(bytes, Kind::MeshDetail)?;
        let (vertex_count, vertices, indices) = input.vertices()?;
        let levels = input.levels()?;
        input.finish()?;
        Ok(MeshDetail {
            vertex_count,
            vertices,
            indices,
            levels,
        })
    }
}

fn check_mesh(
    vertex_count: u32,
    indices: &[u16],
    levels: &[(u32, u32)],
) -> Result<(), &'static str> {
    if indices
        .iter()
        .any(|index| u32::from(*index) >= vertex_count)
    {
        return Err("mesh index out of range");
    }
    let levels_valid = !levels.is_empty()
        && levels.iter().all(|(first, count)| {
            (*first as usize + *count as usize) <= indices.len() && count % 3 == 0
        });
    if !levels_valid {
        return Err("mesh levels out of range");
    }
    Ok(())
}

impl Skeleton {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(Kind::Skeleton);
        out.count(self.bones.len());
        for bone in &self.bones {
            out.f32s(&bone.translation);
            out.f32s(&bone.rotation);
        }
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let mut input = Reader::new(bytes, Kind::Skeleton)?;
        let bones = (0..input.count()?)
            .map(|_| {
                Ok(Bone {
                    translation: input.f32s()?,
                    rotation: input.f32s()?,
                })
            })
            .collect::<Result<_, &'static str>>()?;
        input.finish()?;
        Ok(Skeleton { bones })
    }
}

impl AnimationData {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(Kind::Animation);
        out.f32(self.frame_ms);
        out.u32(self.keys);
        out.u32(self.frames);
        for state in &self.states {
            out.f32s(&state.translation);
            for component in state.rotation {
                out.0.extend_from_slice(&f16_bits(component).to_le_bytes());
            }
        }
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let mut input = Reader::new(bytes, Kind::Animation)?;
        let frame_ms = input.f32()?;
        let keys = input.u32()?;
        let frames = input.u32()?;
        if frames == 0 {
            return Err("animation has no frames");
        }
        let count = keys as usize * frames as usize;
        if count * 20 != input.0.len() {
            return Err("animation size does not match its keys");
        }
        let states = (0..count)
            .map(|_| {
                let translation = input.f32s()?;
                let mut rotation = [0.0; 4];
                for component in &mut rotation {
                    *component = f16_to_f32(input.u16()?);
                }
                Ok(Bone {
                    translation,
                    rotation,
                })
            })
            .collect::<Result<_, &'static str>>()?;
        input.finish()?;
        Ok(AnimationData {
            frame_ms,
            keys,
            frames,
            states,
        })
    }
}

impl TextureFormat {
    fn code(self) -> u8 {
        match self {
            TextureFormat::Rgba8 => 0,
            TextureFormat::Bc1 => 1,
            TextureFormat::Bc2 => 2,
            TextureFormat::Bc3 => 3,
        }
    }

    /// The BC format, or `None` for RGBA8.
    pub fn bc(self) -> Option<crate::bc::Format> {
        match self {
            TextureFormat::Rgba8 => None,
            TextureFormat::Bc1 => Some(crate::bc::Format::Bc1),
            TextureFormat::Bc2 => Some(crate::bc::Format::Bc2),
            TextureFormat::Bc3 => Some(crate::bc::Format::Bc3),
        }
    }

    /// Bytes of one level of this size.
    pub fn level_size(self, width: u32, height: u32) -> usize {
        match self.bc() {
            Some(bc) => bc.size(width, height),
            None => width as usize * height as usize * 4,
        }
    }
}

impl Texture {
    /// The first level whose larger side is at most `max_side`, or a
    /// larger one where that is not whole blocks: WebGPU takes a BC
    /// texture only when its top level is.
    pub fn first_level(&self, max_side: u32) -> usize {
        let extent = |level: usize| level_extent(self.width, self.height, level as u32);
        let small = (0..self.levels.len())
            .find(|level| extent(*level).0.max(extent(*level).1) <= max_side)
            .unwrap_or(0);
        (0..=small)
            .rev()
            .find(|level| {
                let (w, h) = extent(*level);
                self.format.bc().is_none() || (w % 4 == 0 && h % 4 == 0)
            })
            .unwrap_or(0)
    }

    /// The coarse file from level `first` on and the detail of each tier
    /// above it that has levels. The coarse file's detail refs are left
    /// for the caller, who writes the details first to learn them.
    pub fn split(&self, first: usize) -> (Texture, [Option<TextureDetail>; 3]) {
        let first = first.min(self.levels.len() - 1);
        let (width, height) = level_extent(self.width, self.height, first as u32);
        let coarse = Texture {
            format: self.format,
            width,
            height,
            levels: self.levels[first..].to_vec(),
            details: [None; 3],
        };
        let mut end = first;
        let details = TEXTURE_TIER_SIDES.map(|side| {
            let start = self.first_level(side).min(end);
            let (width, height) = level_extent(self.width, self.height, start as u32);
            let detail = (start < end).then(|| TextureDetail {
                format: self.format,
                width,
                height,
                levels: self.levels[start..end].to_vec(),
            });
            end = start;
            detail
        });
        (coarse, details)
    }

    /// This texture completed by the detail of `tier`, the next larger
    /// one it has.
    pub fn refine(&self, tier: usize, detail: &TextureDetail) -> Result<Texture, &'static str> {
        let next = level_extent(detail.width, detail.height, detail.levels.len() as u32);
        if detail.format != self.format || next != (self.width, self.height) {
            return Err("texture detail does not fit its coarse levels");
        }
        let mut levels = detail.levels.clone();
        levels.extend_from_slice(&self.levels);
        let mut details = self.details;
        details[tier] = None;
        Ok(Texture {
            format: self.format,
            width: detail.width,
            height: detail.height,
            levels,
            details,
        })
    }

    /// The tier whose detail is still missing, the smallest first, among
    /// the first `tiers`.
    pub fn next_detail(&self, tiers: usize) -> Option<(usize, FileRef)> {
        self.details
            .iter()
            .take(tiers)
            .enumerate()
            .find_map(|(tier, file)| file.map(|file| (tier, file)))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(Kind::Texture);
        out.mips(self.format, self.width, self.height, &self.levels);
        for detail in self.details {
            out.optional_file(detail);
        }
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let mut input = Reader::new(bytes, Kind::Texture)?;
        let TextureDetail {
            format,
            width,
            height,
            levels,
        } = input.mips()?;
        let details = [
            input.optional_file()?,
            input.optional_file()?,
            input.optional_file()?,
        ];
        input.finish()?;
        Ok(Texture {
            format,
            width,
            height,
            levels,
            details,
        })
    }
}

impl TextureDetail {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(Kind::TextureDetail);
        out.mips(self.format, self.width, self.height, &self.levels);
        out.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        let mut input = Reader::new(bytes, Kind::TextureDetail)?;
        let detail = input.mips()?;
        input.finish()?;
        Ok(detail)
    }
}

/// The size of mip `level`, halving down to 1.
pub fn level_extent(width: u32, height: u32, level: u32) -> (u32, u32) {
    ((width >> level).max(1), (height >> level).max(1))
}

struct Writer(Vec<u8>);

impl Writer {
    fn new(kind: Kind) -> Self {
        let mut out = Vec::new();
        out.extend_from_slice(kind.magic());
        out.push(VERSION);
        Writer(out)
    }

    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn count(&mut self, value: usize) {
        self.u32(value as u32);
    }

    fn i32(&mut self, value: i32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn f32(&mut self, value: f32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn f32s(&mut self, values: &[f32]) {
        for value in values {
            self.f32(*value);
        }
    }

    fn string(&mut self, text: &str) {
        self.0.extend_from_slice(&(text.len() as u16).to_le_bytes());
        self.0.extend_from_slice(text.as_bytes());
    }

    fn file(&mut self, file: FileRef) {
        self.0.extend_from_slice(&file.hash.to_le_bytes());
        self.u32(file.size);
    }

    fn optional_file(&mut self, file: Option<FileRef>) {
        match file {
            Some(file) => {
                self.u8(1);
                self.file(file);
            }
            None => self.u8(0),
        }
    }

    fn vertices(&mut self, vertex_count: u32, vertices: &[u8], indices: &[u16]) {
        self.u32(vertex_count);
        self.0.extend_from_slice(vertices);
        self.count(indices.len());
        for index in indices {
            self.0.extend_from_slice(&index.to_le_bytes());
        }
    }

    fn levels(&mut self, levels: &[(u32, u32)]) {
        self.u8(levels.len() as u8);
        for (first, count) in levels {
            self.u32(*first);
            self.u32(*count);
        }
    }

    fn mips(&mut self, format: TextureFormat, width: u32, height: u32, levels: &[Vec<u8>]) {
        self.u8(format.code());
        self.u32(width);
        self.u32(height);
        self.u8(levels.len() as u8);
        for level in levels {
            self.count(level.len());
            self.0.extend_from_slice(level);
        }
    }

    fn variant(&mut self, variant: &Variant) {
        self.string(&variant.name);
        self.i32(variant.frequency);
        self.optional_file(variant.mesh);
        self.optional_file(variant.texture);
        self.optional_file(variant.normal_texture);
        self.optional_file(variant.specular_texture);
        self.optional_file(variant.ao_texture);
        match variant.decal {
            Some(decal) => {
                self.u8(1);
                self.f32s(&[
                    decal.size_x,
                    decal.size_z,
                    decal.angle,
                    decal.offset_x,
                    decal.offset_z,
                ]);
            }
            None => self.u8(0),
        }
        match &variant.particles {
            Some(emitter) => {
                self.u8(1);
                self.emitter(emitter);
            }
            None => self.u8(0),
        }
        self.count(variant.props.len());
        for prop in &variant.props {
            self.string(&prop.point);
            self.u32(prop.object);
        }
        self.count(variant.animations.len());
        for animation in &variant.animations {
            self.string(&animation.name);
            self.string(&animation.id);
            self.i32(animation.frequency);
            self.f32(animation.speed);
            self.f32(animation.event);
            self.f32(animation.load);
            self.f32(animation.sound);
            self.optional_file(animation.file);
        }
    }

    fn emitter(&mut self, emitter: &Emitter) {
        self.optional_file(emitter.texture);
        self.u8(emitter.blend);
        let flags = [
            (emitter.start_full, EMITTER_START_FULL),
            (emitter.relative_velocity, EMITTER_RELATIVE_VELOCITY),
        ]
        .iter()
        .filter(|(set, _)| *set)
        .fold(0, |flags, (_, flag)| flags | flag);
        self.u8(flags);
        for variable in &emitter.variables {
            match variable {
                ParticleVariable::Constant(value) => {
                    self.u8(VARIABLE_CONSTANT);
                    self.f32(*value);
                }
                ParticleVariable::Uniform(min, max) => {
                    self.u8(VARIABLE_UNIFORM);
                    self.f32s(&[*min, *max]);
                }
                ParticleVariable::Copy(from) => {
                    self.u8(VARIABLE_COPY);
                    self.u8(*from);
                }
                ParticleVariable::Expr { from, mul, max } => {
                    self.u8(VARIABLE_EXPR);
                    self.string(from);
                    self.f32s(&[*mul, *max]);
                }
            }
        }
        self.f32s(&emitter.force);
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], kind: Kind) -> Result<Self, &'static str> {
        let mut input = Reader(bytes);
        if input.take(4)? != kind.magic() {
            return Err("wrong asset kind");
        }
        if input.u8()? != VERSION {
            return Err("unknown asset version");
        }
        Ok(input)
    }

    fn finish(&self) -> Result<(), &'static str> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err("asset has trailing bytes")
        }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], &'static str> {
        if self.0.len() < length {
            return Err("asset truncated");
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

    /// A count, checked against what is left so a corrupt one cannot ask
    /// for a huge allocation.
    fn count(&mut self) -> Result<usize, &'static str> {
        let count = self.u32()? as usize;
        if count > self.0.len() {
            return Err("asset count out of range");
        }
        Ok(count)
    }

    fn i32(&mut self) -> Result<i32, &'static str> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, &'static str> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32s<const N: usize>(&mut self) -> Result<[f32; N], &'static str> {
        let mut values = [0.0; N];
        for value in &mut values {
            *value = self.f32()?;
        }
        Ok(values)
    }

    fn string(&mut self) -> Result<String, &'static str> {
        let length = usize::from(self.u16()?);
        let bytes = self.take(length)?;
        Ok(std::str::from_utf8(bytes)
            .map_err(|_| "asset string not UTF-8")?
            .to_string())
    }

    fn file(&mut self) -> Result<FileRef, &'static str> {
        let hash = u64::from_le_bytes(self.take(8)?.try_into().unwrap());
        let size = self.u32()?;
        Ok(FileRef { hash, size })
    }

    fn optional_file(&mut self) -> Result<Option<FileRef>, &'static str> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.file()?)),
            _ => Err("bad optional file flag"),
        }
    }

    fn vertices(&mut self) -> Result<(u32, Vec<u8>, Vec<u16>), &'static str> {
        let vertex_count = self.u32()?;
        let vertices = self.take(vertex_count as usize * VERTEX_SIZE)?.to_vec();
        let index_count = self.count()?;
        let indices = self
            .take(index_count * 2)?
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Ok((vertex_count, vertices, indices))
    }

    fn levels(&mut self) -> Result<Vec<(u32, u32)>, &'static str> {
        (0..self.u8()?)
            .map(|_| Ok((self.u32()?, self.u32()?)))
            .collect()
    }

    /// Format, size and levels, each level's size checked.
    fn mips(&mut self) -> Result<TextureDetail, &'static str> {
        let format = match self.u8()? {
            0 => TextureFormat::Rgba8,
            1 => TextureFormat::Bc1,
            2 => TextureFormat::Bc2,
            3 => TextureFormat::Bc3,
            _ => return Err("unknown texture format"),
        };
        let width = self.u32()?;
        let height = self.u32()?;
        if width == 0 || height == 0 {
            return Err("empty texture");
        }
        let mut levels = Vec::new();
        for level in 0..self.u8()? {
            let length = self.count()?;
            let (w, h) = level_extent(width, height, u32::from(level));
            if length != format.level_size(w, h) {
                return Err("texture level size does not match");
            }
            levels.push(self.take(length)?.to_vec());
        }
        if levels.is_empty() {
            return Err("texture has no levels");
        }
        Ok(TextureDetail {
            format,
            width,
            height,
            levels,
        })
    }

    fn variant(&mut self) -> Result<Variant, &'static str> {
        let name = self.string()?;
        let frequency = self.i32()?;
        let mesh = self.optional_file()?;
        let texture = self.optional_file()?;
        let normal_texture = self.optional_file()?;
        let specular_texture = self.optional_file()?;
        let ao_texture = self.optional_file()?;
        let decal = match self.u8()? {
            0 => None,
            1 => {
                let [size_x, size_z, angle, offset_x, offset_z] = self.f32s()?;
                Some(Decal {
                    size_x,
                    size_z,
                    angle,
                    offset_x,
                    offset_z,
                })
            }
            _ => return Err("bad decal flag"),
        };
        let particles = match self.u8()? {
            0 => None,
            1 => Some(self.emitter()?),
            _ => return Err("bad particles flag"),
        };
        let props = (0..self.count()?)
            .map(|_| {
                Ok(Prop {
                    point: self.string()?,
                    object: self.u32()?,
                })
            })
            .collect::<Result<_, &'static str>>()?;
        let animations = (0..self.count()?)
            .map(|_| {
                Ok(Animation {
                    name: self.string()?,
                    id: self.string()?,
                    frequency: self.i32()?,
                    speed: self.f32()?,
                    event: self.f32()?,
                    load: self.f32()?,
                    sound: self.f32()?,
                    file: self.optional_file()?,
                })
            })
            .collect::<Result<_, &'static str>>()?;
        Ok(Variant {
            name,
            frequency,
            mesh,
            texture,
            normal_texture,
            specular_texture,
            ao_texture,
            decal,
            particles,
            props,
            animations,
        })
    }

    fn emitter(&mut self) -> Result<Emitter, &'static str> {
        let texture = self.optional_file()?;
        let blend = self.u8()?;
        if blend > BLEND_MULTIPLY {
            return Err("bad particle blend mode");
        }
        let flags = self.u8()?;
        let mut variables = Vec::with_capacity(PARTICLE_VARIABLES);
        for _ in 0..PARTICLE_VARIABLES {
            variables.push(match self.u8()? {
                VARIABLE_CONSTANT => ParticleVariable::Constant(self.f32()?),
                VARIABLE_UNIFORM => {
                    let [min, max] = self.f32s()?;
                    ParticleVariable::Uniform(min, max)
                }
                VARIABLE_COPY => {
                    let from = self.u8()?;
                    if usize::from(from) >= PARTICLE_VARIABLES {
                        return Err("particle variable copies out of range");
                    }
                    ParticleVariable::Copy(from)
                }
                VARIABLE_EXPR => {
                    let from = self.string()?;
                    let [mul, max] = self.f32s()?;
                    ParticleVariable::Expr { from, mul, max }
                }
                _ => return Err("bad particle variable kind"),
            });
        }
        Ok(Emitter {
            texture,
            blend,
            start_full: flags & EMITTER_START_FULL != 0,
            relative_velocity: flags & EMITTER_RELATIVE_VELOCITY != 0,
            variables: variables.try_into().map_err(|_| "particle variables")?,
            force: self.f32s()?,
        })
    }
}
