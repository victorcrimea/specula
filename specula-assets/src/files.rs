//! The split asset files, format v5. Little-endian; every file starts with
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
//!   angle in radians, f32 size in metres}, optional alpha map atlas.
//!   The atlas is an RGBA8 texture, one mip, holding the engine's 14
//!   standard blend masks in its alpha, side by side in
//!   `CTerrainTextureManager::LoadAlphaMap`'s order and layout. Then
//!   count x {sound group path relative to audio/, as the stream names
//!   groups, file ref of its sound group}, count x {civ code (empty: the
//!   default playlists of gui/common/music.js), count x {u8 kind (0 peace,
//!   1 battle, 2 victory, 3 defeat), file ref of a sound}}, optional sound,
//!   the session's ambient loop. Then count x {civ code, count x actor
//!   path, count x sound group path}: the actors and sound groups the
//!   civ's templates can show or play, keys of the lists above, so a
//!   viewer can fetch a match's assets ahead of time.
//! - actor manifest "SPCA": count x object. Object 0 is the actor, the rest
//!   every actor it can prop, transitively, so one fetch resolves an
//!   entity. An object is {name, count x definition, count x {u8 quality,
//!   u32 definition}}, its bases in rising quality as `CActorDef` keeps
//!   them, the last at 255. A definition is {u8 flags (1: player colour),
//!   f32 alpha reference (0: none), count x group}; a group is count x
//!   variant; a variant is {name (lower case), i32 frequency, optional
//!   mesh, optional baseTex texture, optional decal {f32 width, f32 depth,
//!   f32 angle in radians, f32 offset x, f32 offset z}, count x {prop
//!   point, u32 object (NONE: removes the point's props)}, count x {animation name, id, i32
//!   frequency, f32 speed, f32 event, f32 load and f32 sound (fractions,
//!   -1: none), optional animation file}}. Mesh and texture refs name
//!   coarse files.
//! - mesh "SPCM", the coarsest level of detail: u32 vertex count, vertices
//!   of `VERTEX_SIZE` bytes (f32x3 position, f32x3 normal, f32x2 uv, u8x4
//!   bones, unorm8x4 weights; a bone equal to the bone count is the
//!   model's own transform), u32 index count, u16 indices, u8 level count,
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
//!   largest first; block rows and pixel rows top first; optional texture
//!   detail.
//! - texture detail "SPTD": the same up to the levels, for the larger mips
//!   down to the one before the coarse file's first.
//! - sound group "SPCG", `CSoundGroup` as loaded: u8 flags (1 omnipresent,
//!   2 distanceless, 4 heard by the owner only, 8 random gain, 16 random
//!   pitch), f32 gain, f32 pitch, f32 gain lower, f32 gain upper, f32
//!   pitch lower, f32 pitch upper, count x file ref of a sound (the files
//!   that could be read, in the group's order).
//! - sound: an Ogg Opus stream as is (RFC 7845), so a browser decodes it
//!   directly; it starts with Ogg's own "OggS" and version 0, not this
//!   format's version.
//!
//! A file ref is {u64 hash, u32 byte size}; the size lets a viewer show
//! download progress before it has the files. An optional file ref is a
//! u8 0 or 1 and then the ref.

use crate::half::f16_bits;
use crate::half::f16_to_f32;

pub const VERSION: u8 = 4;
/// Bytes per mesh vertex.
pub const VERTEX_SIZE: usize = 40;
/// A prop that names no object only removes what was on its point.
pub const NONE: u32 = u32::MAX;
/// A prop point fixed to the model's origin rather than to a bone.
pub const NO_BONE: u8 = 0xFF;
pub const FLAG_PLAYER_COLOR: u8 = 1;
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
        }
    }

    /// The file's name inside `assets/`.
    pub fn file_name(self, hash: u64) -> String {
        format!("{}-{hash:016x}.bin", self.prefix())
    }
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
    pub sound_groups: Vec<(String, FileRef)>,
    /// By civ code; the empty code holds the defaults.
    pub music: Vec<(String, Vec<(MusicKind, FileRef)>)>,
    pub ambient: Option<FileRef>,
    pub civs: Vec<(String, CivAssets)>,
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

/// `CTerrainTextureEntry`'s diffuse texture and its planar mapping.
#[derive(Clone, Copy, Debug)]
pub struct TerrainTexture {
    pub texture: FileRef,
    /// Radians.
    pub angle: f32,
    /// Metres the texture spans.
    pub size: f32,
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
    pub player_color: bool,
    pub alpha_ref: f32,
    pub groups: Vec<Vec<Variant>>,
}

pub struct Variant {
    pub name: String,
    pub frequency: i32,
    pub mesh: Option<FileRef>,
    pub texture: Option<FileRef>,
    pub decal: Option<Decal>,
    pub props: Vec<Prop>,
    pub animations: Vec<Animation>,
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
    /// What completes a coarse texture; `None` once refined, or when the
    /// coarse levels are all there is.
    pub detail: Option<FileRef>,
}

/// The mips larger than a coarse texture's first.
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
        }
        out.optional_file(self.alpha_maps);
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
                Ok((
                    name,
                    TerrainTexture {
                        texture,
                        angle,
                        size,
                    },
                ))
            })
            .collect::<Result<_, &'static str>>()?;
        let alpha_maps = input.optional_file()?;
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
        input.finish()?;
        Ok(Index {
            actors,
            terrains,
            alpha_maps,
            sound_groups,
            music,
            ambient,
            civs,
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
                out.u8(if definition.player_color {
                    FLAG_PLAYER_COLOR
                } else {
                    0
                });
                out.f32(definition.alpha_ref);
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
                let mut groups = Vec::new();
                for _ in 0..input.count()? {
                    let group = (0..input.count()?)
                        .map(|_| input.variant())
                        .collect::<Result<_, _>>()?;
                    groups.push(group);
                }
                definitions.push(Definition {
                    player_color: flags & FLAG_PLAYER_COLOR != 0,
                    alpha_ref,
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
    /// The coarse file from level `first` on and the detail of the levels
    /// before it, if any. The coarse file's detail ref is left for the
    /// caller, who writes the detail first to learn it.
    pub fn split(&self, first: usize) -> (Texture, Option<TextureDetail>) {
        let first = first.min(self.levels.len() - 1);
        let (width, height) = level_extent(self.width, self.height, first as u32);
        let coarse = Texture {
            format: self.format,
            width,
            height,
            levels: self.levels[first..].to_vec(),
            detail: None,
        };
        let detail = (first > 0).then(|| TextureDetail {
            format: self.format,
            width: self.width,
            height: self.height,
            levels: self.levels[..first].to_vec(),
        });
        (coarse, detail)
    }

    /// The full texture: this coarse one completed by its detail.
    pub fn refine(&self, detail: &TextureDetail) -> Result<Texture, &'static str> {
        let next = level_extent(detail.width, detail.height, detail.levels.len() as u32);
        if detail.format != self.format || next != (self.width, self.height) {
            return Err("texture detail does not fit its coarse levels");
        }
        let mut levels = detail.levels.clone();
        levels.extend_from_slice(&self.levels);
        Ok(Texture {
            format: self.format,
            width: detail.width,
            height: detail.height,
            levels,
            detail: None,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new(Kind::Texture);
        out.mips(self.format, self.width, self.height, &self.levels);
        out.optional_file(self.detail);
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
        let detail = input.optional_file()?;
        input.finish()?;
        Ok(Texture {
            format,
            width,
            height,
            levels,
            detail,
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
            decal,
            props,
            animations,
        })
    }
}
