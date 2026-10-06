//! The ground's geometry as the engine draws it: each tile in its own
//! texture, then the textures of higher-priority neighbours over it through
//! the blend masks. Ported from `CPatchRData::BuildIndices`, `BuildBlends`
//! and `AddBlend` and from `CAlphaMapCalculator` (renderer/PatchRData.cpp,
//! AlphaMapCalculator.cpp, BlendShapes.h).
//!
//! The engine builds blends per patch; here the whole map is one patch.
//! Blends only cover their own tile and a tile's stack keeps its order, so
//! the picture is the same, from far fewer draws.

use specula_assets::math::Vec3;

use crate::terrain::TILE_SIZE;
use crate::terrain::TerrainImage;

/// f32x3 position, f32x3 normal, f32x3 mask: u and v across the tile's
/// mask (0..1, flips and rotation applied) and the mask's index in the
/// atlas.
pub const VERTEX_SIZE: usize = 36;

/// `BlendOffsets`: (dz, dx) of each neighbour, the tile itself last.
const BLEND_OFFSETS: [(isize, isize); 9] = [
    (0, -1),
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
    (1, 0),
    (1, -1),
    (0, 0),
];
const CURRENT_TILE: u16 = 1 << 8;

const FLIP_V: u32 = 0x01;
const FLIP_U: u32 = 0x02;
const ROTATE_90: u32 = 0x04;
const ROTATE_180: u32 = 0x08;
const ROTATE_270: u32 = 0x10;
/// `blendbad.png`: a shape the tables do not cover.
const BAD_BLEND: i32 = 13;

/// A run of indices drawn with one terrain, a palette index (0xFFFF for
/// none, which no palette entry has).
pub struct Splat {
    pub texture: u16,
    pub first: u32,
    pub count: u32,
}

pub struct GroundMesh {
    pub vertices: Vec<u8>,
    pub indices: Vec<u32>,
    /// Opaque, in any order.
    pub base: Vec<Splat>,
    /// Blended over the base, in this order.
    pub blends: Vec<Splat>,
}

#[derive(Clone, Copy)]
struct TileBlend {
    texture: u16,
    priority: u32,
    /// Bit n: the neighbour at `BLEND_OFFSETS[n]` has this texture.
    mask: u16,
}

pub fn build(image: &TerrainImage) -> GroundMesh {
    let tiles = image.size as usize;
    let side = tiles + 1;
    let height = |i: usize, j: usize| f32::from(image.heights[j * side + i]) / 92.0;
    let normals: Vec<Vec3> = (0..side * side)
        .map(|at| {
            let (i, j) = (at % side, at / side);
            let left = height(i.saturating_sub(1), j);
            let right = height((i + 1).min(tiles), j);
            let down = height(i, j.saturating_sub(1));
            let up = height(i, (j + 1).min(tiles));
            Vec3::new(left - right, 2.0 * TILE_SIZE, down - up).normalized()
        })
        .collect();
    let mut vertices = Vec::with_capacity(side * side * VERTEX_SIZE);
    let push_vertex = |vertices: &mut Vec<u8>, i: usize, j: usize, mask: [f32; 3]| {
        let normal = normals[j * side + i];
        for value in [
            i as f32 * TILE_SIZE,
            height(i, j),
            j as f32 * TILE_SIZE,
            normal.x,
            normal.y,
            normal.z,
            mask[0],
            mask[1],
            mask[2],
        ] {
            vertices.extend_from_slice(&value.to_le_bytes());
        }
    };
    for j in 0..side {
        for i in 0..side {
            push_vertex(&mut vertices, i, j, [0.0; 3]);
        }
    }

    // BuildIndices: the base tiles, grouped by texture.
    let mut order: Vec<usize> = (0..tiles * tiles).collect();
    order.sort_by_key(|&tile| image.tiles[tile]);
    let mut indices = Vec::with_capacity(tiles * tiles * 6);
    let mut base: Vec<Splat> = Vec::new();
    for tile in order {
        let (i, j) = (tile % tiles, tile / tiles);
        let texture = image.tiles[tile];
        if base.last().is_none_or(|splat| splat.texture != texture) {
            base.push(Splat {
                texture,
                first: indices.len() as u32,
                count: 0,
            });
        }
        indices.extend_from_slice(&tile_triangles(image, i, j));
        base.last_mut().unwrap().count += 6;
    }

    // BuildBlends: each tile's stack of blends, highest priority first,
    // down to but not including its own texture.
    let name = |texture: u16| image.names.get(usize::from(texture));
    let mut stacks: Vec<(usize, Vec<TileBlend>)> = Vec::new();
    for gz in 0..tiles {
        for gx in 0..tiles {
            let mut blends: Vec<TileBlend> = BLEND_OFFSETS
                .iter()
                .enumerate()
                .filter_map(|(n, (dz, dx))| {
                    let ox = gx.checked_add_signed(*dx).filter(|&x| x < tiles)?;
                    let oz = gz.checked_add_signed(*dz).filter(|&z| z < tiles)?;
                    let tile = oz * tiles + ox;
                    Some(TileBlend {
                        texture: image.tiles[tile],
                        priority: image.priorities[tile],
                        mask: 1 << n,
                    })
                })
                .collect();
            // DecreasingPriority: ties go to the greater tag.
            blends.sort_by(|a, b| {
                b.priority
                    .cmp(&a.priority)
                    .then_with(|| match (name(a.texture), name(b.texture)) {
                        (Some(a), Some(b)) => b.cmp(a),
                        _ => std::cmp::Ordering::Equal,
                    })
            });
            let mut stack: Vec<TileBlend> = Vec::new();
            for blend in blends {
                match stack.last_mut() {
                    Some(last) if last.texture == blend.texture => last.mask |= blend.mask,
                    _ => stack.push(blend),
                }
            }
            if let Some(current) = stack
                .iter()
                .position(|blend| blend.mask & CURRENT_TILE != 0)
            {
                stack.truncate(current);
            }
            if !stack.is_empty() {
                stacks.push((gz * tiles + gx, stack));
            }
        }
    }

    // Layers of one texture each, taken off the stacks' bottoms; a new
    // layer takes the texture of the longest stack left.
    let mut blends: Vec<Splat> = Vec::new();
    // The first of the longest, as the engine's strict comparison keeps.
    let longest = |stacks: &[(usize, Vec<TileBlend>)]| {
        let mut best: Option<&Vec<TileBlend>> = None;
        for (_, stack) in stacks {
            if stack.len() > best.map_or(0, Vec::len) {
                best = Some(stack);
            }
        }
        best.map(|stack| stack.last().unwrap().texture)
    };
    while let Some(texture) = longest(&stacks) {
        let first = indices.len() as u32;
        for (tile, stack) in &mut stacks {
            if stack.last().is_none_or(|blend| blend.texture != texture) {
                continue;
            }
            let blend = stack.pop().unwrap();
            add_blend(
                &mut vertices,
                &mut indices,
                image,
                *tile,
                blend.mask as u8,
                &push_vertex,
            );
        }
        stacks.retain(|(_, stack)| !stack.is_empty());
        let count = indices.len() as u32 - first;
        if count > 0 {
            blends.push(Splat {
                texture,
                first,
                count,
            });
        }
    }
    GroundMesh {
        vertices,
        indices,
        base,
        blends,
    }
}

/// The base pass's triangles of tile (i, j), indexing the grid of
/// vertices that starts `GroundMesh::vertices`. Decals draw these very
/// triangles, so their depth equals the ground's.
pub fn tile_triangles(image: &TerrainImage, i: usize, j: usize) -> [u32; 6] {
    let side = image.size as usize + 1;
    let at = |di: usize, dj: usize| ((j + dj) * side + i + di) as u32;
    // The same diagonal as CTerrain::GetExactGroundLevel, so units stand
    // on the surface drawn.
    if image.triangulation_dir(i, j) {
        [at(0, 0), at(1, 0), at(0, 1), at(1, 0), at(1, 1), at(0, 1)]
    } else {
        [at(0, 0), at(1, 1), at(0, 1), at(0, 0), at(1, 0), at(1, 1)]
    }
}

/// `CPatchRData::AddBlend`: a quad over the tile whose mask fits the
/// neighbours that have the texture.
fn add_blend(
    vertices: &mut Vec<u8>,
    indices: &mut Vec<u32>,
    image: &TerrainImage,
    tile: usize,
    shape: u8,
    push_vertex: &impl Fn(&mut Vec<u8>, usize, usize, [f32; 3]),
) {
    let tiles = image.size as usize;
    let (gx, gz) = (tile % tiles, tile / tiles);
    let shape8: [bool; 8] = std::array::from_fn(|m| shape & (1 << m) == 0);
    let (alpha_map, flags) = calculate(shape8);
    if alpha_map < 0 {
        return;
    }
    let (mut u0, mut u1, mut v0, mut v1) = (0.0, 1.0, 0.0, 1.0);
    if flags & FLIP_U != 0 {
        std::mem::swap(&mut u0, &mut u1);
    }
    if flags & FLIP_V != 0 {
        std::mem::swap(&mut v0, &mut v1);
    }
    let base = if flags & ROTATE_90 != 0 {
        1
    } else if flags & ROTATE_180 != 0 {
        2
    } else if flags & ROTATE_270 != 0 {
        3
    } else {
        0
    };
    let mut uvs = [[0.0f32; 2]; 4];
    uvs[base % 4] = [u0, v0];
    uvs[(base + 1) % 4] = [u1, v0];
    uvs[(base + 2) % 4] = [u1, v1];
    uvs[(base + 3) % 4] = [u0, v1];

    let index = (vertices.len() / VERTEX_SIZE) as u32;
    let mask = alpha_map as f32;
    for (corner, (di, dj)) in [(0, 0), (1, 0), (1, 1), (0, 1)].into_iter().enumerate() {
        let [u, v] = uvs[corner];
        push_vertex(vertices, gx + di, gz + dj, [u, v, mask]);
    }
    let quad = if image.triangulation_dir(gx, gz) {
        [0, 1, 3, 1, 2, 3]
    } else {
        [0, 1, 2, 2, 3, 0]
    };
    indices.extend(quad.map(|corner| index + corner));
}

type Shape4 = [bool; 4];
type Shape8 = [bool; 8];

fn shape4(bits: [u8; 4]) -> Shape4 {
    bits.map(|bit| bit != 0)
}

/// `BlendShape4` and `BlendShape8`'s rotations and flips, as index maps:
/// `dst[k] = src[map[k]]`.
trait Shape: Copy + PartialEq {
    const ROTATE_90: &'static [usize];
    const ROTATE_180: &'static [usize];
    const ROTATE_270: &'static [usize];
    const FLIP_U: &'static [usize];
    const FLIP_V: &'static [usize];
    fn permuted(&self, map: &[usize]) -> Self;
}

impl Shape for Shape4 {
    const ROTATE_90: &'static [usize] = &[3, 0, 1, 2];
    const ROTATE_180: &'static [usize] = &[2, 3, 0, 1];
    const ROTATE_270: &'static [usize] = &[1, 2, 3, 0];
    const FLIP_U: &'static [usize] = &[2, 1, 0, 3];
    const FLIP_V: &'static [usize] = &[0, 3, 2, 1];
    fn permuted(&self, map: &[usize]) -> Self {
        std::array::from_fn(|k| self[map[k]])
    }
}

impl Shape for Shape8 {
    const ROTATE_90: &'static [usize] = &[6, 7, 0, 1, 2, 3, 4, 5];
    const ROTATE_180: &'static [usize] = &[4, 5, 6, 7, 0, 1, 2, 3];
    const ROTATE_270: &'static [usize] = &[2, 3, 4, 5, 6, 7, 0, 1];
    const FLIP_U: &'static [usize] = &[4, 3, 2, 1, 0, 7, 6, 5];
    const FLIP_V: &'static [usize] = &[0, 7, 6, 5, 4, 3, 2, 1];
    fn permuted(&self, map: &[usize]) -> Self {
        std::array::from_fn(|k| self[map[k]])
    }
}

/// `MatchBlendShapeFlipped`.
fn match_flipped<S: Shape>(template: S, shape: S, flags: &mut u32) -> bool {
    if shape == template {
        return true;
    }
    if shape == template.permuted(S::FLIP_U) {
        *flags |= FLIP_U;
        return true;
    }
    if shape == template.permuted(S::FLIP_V) {
        *flags |= FLIP_V;
        return true;
    }
    false
}

/// `MatchBlendShape`. A flipped match turns the other way.
fn match_shape<S: Shape>(template: S, shape: S, flags: &mut u32) -> bool {
    if match_flipped(template, shape, flags) {
        return true;
    }
    if match_flipped(template.permuted(S::ROTATE_90), shape, flags) {
        *flags |= if *flags != 0 { ROTATE_270 } else { ROTATE_90 };
        return true;
    }
    if match_flipped(template.permuted(S::ROTATE_180), shape, flags) {
        *flags |= ROTATE_180;
        return true;
    }
    if match_flipped(template.permuted(S::ROTATE_270), shape, flags) {
        *flags |= if *flags != 0 { ROTATE_90 } else { ROTATE_270 };
        return true;
    }
    false
}

fn lookup<S: Shape>(table: &[(S, i32)], shape: S, flags: &mut u32) -> i32 {
    table
        .iter()
        .find(|(template, _)| match_shape(*template, shape, flags))
        .map_or(BAD_BLEND, |(_, alpha_map)| *alpha_map)
}

/// `CAlphaMapCalculator::Calculate`: the mask fitting `shape` (true where
/// the neighbour lacks the texture, in `BLEND_OFFSETS` order) and its
/// flips and rotation; -1 for none.
fn calculate(shape: Shape8) -> (i32, u32) {
    let mut flags = 0;
    let count = shape.iter().filter(|&&set| set).count();
    match count {
        0 => return (0, flags),
        8 => return (-1, flags),
        _ => {}
    }
    if count <= 4 && !shape[1] && !shape[3] && !shape[5] && !shape[7] {
        let four: Shape4 = [shape[0], shape[2], shape[4], shape[6]];
        let alpha_map = match count {
            1 => lookup(&[(shape4([1, 0, 0, 0]), 12)], four, &mut flags),
            2 => lookup(
                &[(shape4([0, 1, 1, 0]), 7), (shape4([1, 0, 1, 0]), 10)],
                four,
                &mut flags,
            ),
            3 => lookup(&[(shape4([1, 1, 1, 0]), 4)], four, &mut flags),
            _ => 5,
        };
        return (alpha_map, flags);
    }
    let alpha_map = match count {
        1 => 0,
        2 => lookup(BLENDS_2, shape, &mut flags),
        3 => lookup(BLENDS_3, shape, &mut flags),
        4 => lookup(BLENDS_4, shape, &mut flags),
        5 => lookup(BLENDS_5, shape, &mut flags),
        6 => lookup(BLENDS_6, shape, &mut flags),
        7 => lookup(BLENDS_7, shape, &mut flags),
        _ => BAD_BLEND,
    };
    (alpha_map, flags)
}

macro_rules! table {
    ($name:ident: $([$($bit:literal),*] => $map:literal),* $(,)?) => {
        const $name: &[(Shape8, i32)] = &[$(([$($bit != 0),*], $map)),*];
    };
}

table!(BLENDS_2:
    [1, 1, 0, 0, 0, 0, 0, 0] => 12,
    [1, 0, 0, 0, 0, 1, 0, 0] => 12,
    [0, 1, 0, 1, 0, 0, 0, 0] => 0,
    [0, 1, 0, 0, 0, 1, 0, 0] => 0,
);
table!(BLENDS_3:
    [1, 1, 0, 0, 1, 0, 0, 0] => 10,
    [1, 1, 0, 0, 0, 0, 0, 1] => 12,
    [1, 1, 1, 0, 0, 0, 0, 0] => 1,
    [0, 1, 1, 0, 1, 0, 0, 0] => 7,
    [0, 0, 1, 0, 1, 0, 1, 0] => 4,
    [1, 1, 0, 0, 0, 1, 0, 0] => 12,
    [1, 1, 0, 1, 0, 0, 0, 0] => 12,
    [0, 0, 1, 0, 1, 0, 0, 1] => 7,
    [1, 0, 0, 1, 0, 1, 0, 0] => 12,
    [0, 1, 0, 1, 0, 1, 0, 0] => 0,
);
table!(BLENDS_4:
    [1, 1, 0, 0, 1, 0, 0, 1] => 10,
    [1, 1, 0, 1, 1, 0, 0, 0] => 10,
    [1, 1, 0, 0, 1, 1, 0, 0] => 10,
    [1, 1, 0, 1, 0, 0, 0, 1] => 12,
    [0, 1, 1, 0, 1, 1, 0, 0] => 7,
    [1, 1, 1, 1, 0, 0, 0, 0] => 1,
    [1, 1, 1, 0, 1, 0, 0, 0] => 3,
    [0, 0, 1, 0, 1, 1, 0, 1] => 7,
    [1, 0, 1, 0, 1, 1, 0, 0] => 4,
    [1, 1, 1, 0, 0, 1, 0, 0] => 1,
    [1, 1, 0, 1, 0, 1, 0, 0] => 12,
    [0, 1, 0, 1, 0, 1, 0, 1] => 0,
);
table!(BLENDS_5:
    [1, 1, 1, 1, 1, 0, 0, 0] => 2,
    [1, 1, 1, 1, 0, 0, 0, 1] => 1,
    [1, 1, 1, 0, 1, 0, 0, 1] => 3,
    [1, 1, 1, 0, 1, 0, 1, 0] => 11,
    [1, 1, 1, 0, 0, 1, 0, 1] => 1,
    [1, 1, 0, 1, 1, 1, 0, 0] => 10,
    [1, 1, 1, 0, 1, 1, 0, 0] => 3,
    [1, 0, 1, 0, 1, 1, 0, 1] => 4,
    [1, 1, 0, 1, 0, 1, 0, 1] => 12,
    [0, 1, 1, 0, 1, 1, 0, 1] => 7,
);
table!(BLENDS_6:
    [1, 1, 1, 1, 1, 1, 0, 0] => 2,
    [1, 1, 1, 1, 1, 0, 1, 0] => 8,
    [1, 1, 1, 1, 0, 1, 0, 1] => 1,
    [1, 1, 1, 0, 1, 1, 1, 0] => 6,
    [1, 1, 1, 0, 1, 1, 0, 1] => 3,
    [1, 1, 0, 1, 1, 1, 0, 1] => 10,
);
table!(BLENDS_7:
    [1, 1, 1, 1, 1, 1, 0, 1] => 2,
    [1, 1, 1, 1, 1, 1, 1, 0] => 9,
);
