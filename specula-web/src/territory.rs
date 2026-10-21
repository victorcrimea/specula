//! Territory as the game shows it: on the minimap a band of the owner's
//! colour inside each border (`CTerritoryTexture`), on the ground a
//! textured line along each border (`CCmpTerritoryManager`'s boundary
//! lines, drawn by `CTexturedLineRData`).
//!
//! On the ground the line is the engine's width up close, but never
//! narrower than `MIN_PIXELS` on screen, so a zoomed-out view still shows
//! where territories end. It widens inwards only, so neighbours' lines
//! stay back to back.

use crate::stream::Territory;
use crate::terrain::TerrainImage;
use crate::unit::DEPTH_FORMAT;
use crate::unit::create_buffer;

/// `NAVCELL_SIZE * NAVCELLS_PER_TERRITORY_TILE`, in metres.
pub const TILE_SIZE: f32 = 8.0;
/// `ICmpTerritoryManager::TERRITORY_PLAYER_MASK`.
pub const OWNER_MASK: u8 = 0x1F;
const BLINKING_MASK: u8 = 0x40;
/// Never sent by the hook; marks tiles while tracing, as the engine does.
const PROCESSED_MASK: u8 = 0x80;
/// `TERRITORY_DISCR_MASK` of `TerritoryBoundary.cpp`: a boundary parts
/// tiles of another owner or another blinking state.
const DISCRIMINATOR_MASK: u8 = BLINKING_MASK | OWNER_MASK;
/// `CTerritoryTexture::GenerateBitmap`.
const ALPHA_MAX: u8 = 0xC0;
const ALPHA_FALLOFF: i32 = 0x20;
/// `InterpolatePointsRNS`' default.
const SEGMENT_SAMPLES: usize = 4;
/// `OverlayRenderer::OVERLAY_VOFFSET`, in metres along the ground normal.
pub const OVERLAY_VOFFSET: f32 = 0.2;
/// The narrowest a ground line is drawn, in CSS pixels.
const MIN_PIXELS: f32 = 8.0;
/// The engine's colour for a player it cannot find.
const MISSING_COLOR: u32 = 0xFFFF_00FF;
/// Position, miter, uv f32x3 f32x3 f32x2, colour u8x4, then a float of
/// the shader's: borders.wgsl's blinking, selection.wgsl's thickness.
pub const VERTEX_SIZE: usize = 40;
const UV_OFFSET: usize = 24;
pub const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    3 => Unorm8x4,
    4 => Float32,
];
/// Metres per pixel at depth 1, narrowest width in pixels, seconds, unused;
/// borders.wgsl `Params`.
const PARAMS_SIZE: u64 = 16;
/// The engine samples the border and selection textures anisotropically.
pub const ANISOTROPY: u16 = 16;
/// territory-border.bin's format, built by specula-tools' territory-border
/// (layout documented there).
const BORDER_VERSION: u8 = 0;
const BORDER_TEXTURES: &[u8] = include_bytes!("../territory-border.bin");

/// The minimap picture: `territory.width` x `territory.height` RGBA8,
/// row-major, row 0 at z = 0. As `CTerritoryTexture::GenerateBitmap`:
/// opaque-ish at each border, fading inwards, and clear on the border tiles
/// themselves, so neighbours' bands are apart.
pub fn minimap_bitmap(territory: &Territory, palette: &[u32]) -> Vec<u8> {
    let w = territory.width as usize;
    let h = territory.height as usize;
    let owner = |i: usize, j: usize| territory.tiles[j * w + i] & OWNER_MASK;
    let mut bitmap = vec![0u8; w * h * 4];
    for j in 0..h {
        for i in 0..w {
            let value = owner(i, j);
            let color = palette
                .get(usize::from(value))
                .copied()
                .unwrap_or(MISSING_COLOR);
            let pixel = &mut bitmap[(j * w + i) * 4..][..4];
            pixel[..3].copy_from_slice(&color.to_be_bytes()[1..]);
            // Gaia's tiles too; they are cleared below with the borders.
            let edge = value == 0
                || (i > 0 && owner(i - 1, j) != value)
                || (i + 1 < w && owner(i + 1, j) != value)
                || (j > 0 && owner(i, j - 1) != value)
                || (j + 1 < h && owner(i, j + 1) != value);
            pixel[3] = if edge { ALPHA_MAX } else { 0 };
        }
    }

    // The engine's cheap blur: a falloff dragged along each row and column
    // both ways.
    let mut blur = |indices: &mut dyn Iterator<Item = usize>| {
        let mut alpha = 0;
        for index in indices {
            let pixel = &mut bitmap[index * 4 + 3];
            alpha = (alpha - ALPHA_FALLOFF).max(i32::from(*pixel));
            *pixel = alpha as u8;
        }
    };
    for j in 0..h {
        blur(&mut (0..w).map(|i| j * w + i));
        blur(&mut (0..w).rev().map(|i| j * w + i));
    }
    for i in 0..w {
        blur(&mut (0..h).map(|j| j * w + i));
        blur(&mut (0..h).rev().map(|j| j * w + i));
    }

    for alpha in bitmap.iter_mut().skip(3).step_by(4) {
        if *alpha == ALPHA_MAX {
            *alpha = 0;
        }
    }
    bitmap
}

/// `STerritoryBoundary`.
struct Boundary {
    owner: u8,
    blinking: bool,
    /// In metres, x and z; anticlockwise round an outer border, clockwise
    /// round an inner one.
    points: Vec<[f32; 2]>,
}

/// `CTerritoryBoundaryCalculator::ComputeBoundaries`.
fn boundaries(territory: &Territory) -> Vec<Boundary> {
    const TILE_BOTTOM: u8 = 0;
    const TILE_RIGHT: u8 = 1;
    const TILE_TOP: u8 = 2;
    const TILE_LEFT: u8 = 3;
    const EDGE_OFFSETS: [[f32; 2]; 4] = [[0.5, 0.0], [1.0, 0.5], [0.5, 1.0], [0.0, 0.5]];

    let w = territory.width as usize;
    let h = territory.height as usize;
    let mut grid = territory.tiles.clone();
    let mut boundaries = Vec::new();
    if w == 0 || h == 0 {
        return boundaries;
    }
    let (max_i, max_j) = (w - 1, h - 1);
    for j in 0..h {
        for i in 0..w {
            let state = grid[j * w + i];
            let discriminator = state & DISCRIMINATOR_MASK;
            if discriminator == 0 {
                continue;
            }
            let processed = state & PROCESSED_MASK != 0;
            let eligible = j == 0 || discriminator != grid[(j - 1) * w + i] & DISCRIMINATOR_MASK;
            if processed || !eligible {
                continue;
            }

            let mut points = Vec::new();
            let same = |grid: &[u8], i: usize, j: usize| {
                grid[j * w + i] & DISCRIMINATOR_MASK == discriminator
            };
            let (mut ci, mut cj, mut direction) = (i, j, TILE_BOTTOM);
            loop {
                let offset = EDGE_OFFSETS[usize::from(direction)];
                points.push([
                    (ci as f32 + offset[0]) * TILE_SIZE,
                    (cj as f32 + offset[1]) * TILE_SIZE,
                ]);
                // Along a border, anticlockwise: on, or round a corner one
                // way or the other.
                match direction {
                    TILE_BOTTOM => {
                        grid[cj * w + ci] |= PROCESSED_MASK;
                        if ci < max_i && cj > 0 && same(&grid, ci + 1, cj - 1) {
                            ci += 1;
                            cj -= 1;
                            direction = TILE_LEFT;
                        } else if ci < max_i && same(&grid, ci + 1, cj) {
                            ci += 1;
                        } else {
                            direction = TILE_RIGHT;
                        }
                    }
                    TILE_RIGHT => {
                        if ci < max_i && cj < max_j && same(&grid, ci + 1, cj + 1) {
                            ci += 1;
                            cj += 1;
                            direction = TILE_BOTTOM;
                        } else if cj < max_j && same(&grid, ci, cj + 1) {
                            cj += 1;
                        } else {
                            direction = TILE_TOP;
                        }
                    }
                    TILE_TOP => {
                        if ci > 0 && cj < max_j && same(&grid, ci - 1, cj + 1) {
                            ci -= 1;
                            cj += 1;
                            direction = TILE_RIGHT;
                        } else if ci > 0 && same(&grid, ci - 1, cj) {
                            ci -= 1;
                        } else {
                            direction = TILE_LEFT;
                        }
                    }
                    _ => {
                        if ci > 0 && cj > 0 && same(&grid, ci - 1, cj - 1) {
                            ci -= 1;
                            cj -= 1;
                            direction = TILE_TOP;
                        } else if cj > 0 && same(&grid, ci, cj - 1) {
                            cj -= 1;
                        } else {
                            direction = TILE_BOTTOM;
                        }
                    }
                }
                if ci == i && cj == j && direction == TILE_BOTTOM {
                    break;
                }
            }
            boundaries.push(Boundary {
                owner: state & OWNER_MASK,
                blinking: state & BLINKING_MASK != 0,
                points,
            });
        }
    }
    boundaries
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale(a: [f32; 2], s: f32) -> [f32; 2] {
    [a[0] * s, a[1] * s]
}

/// `CVector2D::Normalized`: a zero vector stays zero.
fn normalized(a: [f32; 2]) -> [f32; 2] {
    let length = a[0].hypot(a[1]);
    if length > 0.0 {
        scale(a, 1.0 / length)
    } else {
        a
    }
}

/// `SimRender::SmoothPointsAverage` of a closed line.
fn smooth_points_average(points: &mut Vec<[f32; 2]>) {
    let n = points.len();
    if n < 2 {
        return;
    }
    let smoothed = (0..n)
        .map(|i| {
            let sum = add(add(points[(i + n - 1) % n], points[i]), points[(i + 1) % n]);
            scale(sum, 1.0 / 3.0)
        })
        .collect();
    *points = smoothed;
}

/// `SimRender::InterpolatePointsRNS` of a closed line with no offset: the
/// offset is the shader's, as it depends on the line's width there.
fn interpolate_points_rns(points: &mut Vec<[f32; 2]>) {
    let n = points.len();
    if n < 1 {
        return;
    }
    let mut interpolated = Vec::with_capacity(n * SEGMENT_SAMPLES);
    for i in 0..n {
        let p0 = points[(i + n - 1) % n];
        let p1 = points[i];
        let p2 = points[(i + 1) % n];
        let p3 = points[(i + 2) % n];
        // GPG4 "Nonuniform Splines", as the engine.
        let l1 = sub(p2, p1);
        let l1 = l1[0].hypot(l1[1]);
        let s0 = normalized(sub(p1, p0));
        let s1 = normalized(sub(p2, p1));
        let s2 = normalized(sub(p3, p2));
        let v1 = scale(normalized(add(s0, s1)), l1);
        let v2 = scale(normalized(add(s1, s2)), l1);
        let a0 = add(add(scale(p1, 2.0), scale(p2, -2.0)), add(v1, v2));
        let a1 = add(
            add(scale(p1, -3.0), scale(p2, 3.0)),
            add(scale(v1, -2.0), scale(v2, -1.0)),
        );
        let a2 = v1;
        let a3 = p1;
        for sample in 0..SEGMENT_SAMPLES {
            let t = sample as f32 / SEGMENT_SAMPLES as f32;
            interpolated.push(add(
                add(scale(a0, t * t * t), scale(a1, t * t)),
                add(scale(a2, t), a3),
            ));
        }
    }
    *points = interpolated;
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn normalized3(a: [f32; 3]) -> [f32; 3] {
    let length = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if length > 0.0 {
        a.map(|v| v / length)
    } else {
        a
    }
}

/// `CVector3D::Cross`.
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The closed-line part of `CTexturedLineRData::Update`, appended to
/// `vertices` and `indices`. Each vertex keeps the line's own point and the
/// miter for a width of 1 m, which the shader scales, and `extra` for the
/// shader.
pub fn ribbon(
    points: &[[f32; 2]],
    color: u32,
    extra: f32,
    image: &TerrainImage,
    vertices: &mut Vec<u8>,
    indices: &mut Vec<u32>,
) {
    let n = points.len();
    if n < 2 {
        return;
    }
    // Clamped to the water, as the engine does, remembering which points
    // float, whose normal is then straight up.
    let ground = |[x, z]: [f32; 2]| {
        let y = image.ground(x, z);
        if y < image.water_level {
            ([x, image.water_level, z], true)
        } else {
            ([x, y, z], false)
        }
    };
    let first = (vertices.len() / VERTEX_SIZE) as u32;
    let rgba = color.to_be_bytes();
    let color = [rgba[1], rgba[2], rgba[3], rgba[0]];
    let mut push = |position: [f32; 3], miter: [f32; 3], uv: [f32; 2]| {
        for value in position.iter().chain(&miter).chain(&uv) {
            vertices.extend_from_slice(&value.to_le_bytes());
        }
        vertices.extend_from_slice(&color);
        vertices.extend_from_slice(&extra.to_le_bytes());
    };
    let mut v = 0.0;
    for i in 0..n {
        let (p0, _) = ground(points[(i + n - 1) % n]);
        let (p1, floating) = ground(points[i]);
        let (p2, _) = ground(points[(i + 1) % n]);
        let normal = if floating {
            [0.0, 1.0, 0.0]
        } else {
            image.normal(p1[0], p1[2])
        };
        let behind = normalized3(sub3(p1, p0));
        let ahead = normalized3(sub3(p2, p1));
        let mut miter = cross(
            [
                behind[0] + ahead[0],
                behind[1] + ahead[1],
                behind[2] + ahead[2],
            ],
            normal,
        );
        let length = dot3(miter, cross(ahead, normal));
        if length.abs() > 0.000001 {
            miter = miter.map(|value| value / length);
        }
        let lifted = [
            p1[0] + normal[0] * OVERLAY_VOFFSET,
            p1[1] + normal[1] * OVERLAY_VOFFSET,
            p1[2] + normal[2] * OVERLAY_VOFFSET,
        ];
        push(lifted, miter, [0.0, v]);
        push(lifted, miter, [1.0, v]);
        v = 1.0 - v;
    }
    // Quads (2i, 2i+1, 2i+2, 2i+3); the last closes the loop. An odd count
    // ends on v = 0 like the first pair, so the engine adds the first pair
    // again with v = 1 for the last quad.
    let mut pairs = n as u32;
    if !n.is_multiple_of(2) {
        let start = first as usize * VERTEX_SIZE;
        let first_pair = vertices[start..start + 2 * VERTEX_SIZE].to_vec();
        for (u, vertex) in first_pair.chunks_exact(VERTEX_SIZE).enumerate() {
            vertices.extend_from_slice(&vertex[..UV_OFFSET]);
            vertices.extend_from_slice(&(u as f32).to_le_bytes());
            vertices.extend_from_slice(&1.0f32.to_le_bytes());
            vertices.extend_from_slice(&vertex[UV_OFFSET + 8..]);
        }
        pairs += 1;
    }
    let quad = |indices: &mut Vec<u32>, from: u32, to: u32| {
        let (a, b) = (first + 2 * from, first + 2 * from + 1);
        let (c, d) = (first + 2 * to, first + 2 * to + 1);
        indices.extend_from_slice(&[a, b, c, c, b, d]);
    };
    for pair in 1..pairs {
        quad(indices, pair - 1, pair);
    }
    if n.is_multiple_of(2) {
        quad(indices, pairs - 1, 0);
    }
}

/// `SOverlayTexturedLine::LineCapType`, as the hook sends it.
pub const CAP_ROUND: u8 = 1;
pub const CAP_SHARP: u8 = 2;
pub const CAP_SQUARE: u8 = 3;

/// The open-line part of `CTexturedLineRData::Update` with its caps
/// (`CreateLineCap`), appended as `ribbon` appends. The caps are laid out
/// here for `thickness`, which goes to the shader as its float too, and
/// their vertices have no miter.
#[allow(clippy::too_many_arguments)]
pub fn open_ribbon(
    points: &[[f32; 2]],
    color: u32,
    thickness: f32,
    start_cap: u8,
    end_cap: u8,
    image: &TerrainImage,
    vertices: &mut Vec<u8>,
    indices: &mut Vec<u32>,
) {
    let n = points.len();
    if n < 2 {
        return;
    }
    let ground = |[x, z]: [f32; 2]| {
        let y = image.ground(x, z);
        if y < image.water_level {
            ([x, image.water_level, z], true)
        } else {
            ([x, y, z], false)
        }
    };
    // The engine extends the first and the last segment by an artificial
    // point beyond each end, so the ends are cut square.
    let beyond = |end: [f32; 2], next: [f32; 2]| [2.0 * end[0] - next[0], 2.0 * end[1] - next[1]];
    let first = (vertices.len() / VERTEX_SIZE) as u32;
    let rgba = color.to_be_bytes();
    let color = [rgba[1], rgba[2], rgba[3], rgba[0]];
    let mut push = |position: [f32; 3], miter: [f32; 3], uv: [f32; 2]| {
        for value in position.iter().chain(&miter).chain(&uv) {
            vertices.extend_from_slice(&value.to_le_bytes());
        }
        vertices.extend_from_slice(&color);
        vertices.extend_from_slice(&thickness.to_le_bytes());
    };
    // Each pair's corners, at u 0 and u 1, for the caps.
    let mut corners: Vec<([f32; 3], [f32; 3])> = Vec::with_capacity(n);
    let mut v = 0.0;
    for i in 0..n {
        let (p1, floating) = ground(points[i]);
        let (p0, _) = ground(if i == 0 {
            beyond(points[0], points[1])
        } else {
            points[i - 1]
        });
        let (p2, _) = ground(if i == n - 1 {
            beyond(points[i], points[i - 1])
        } else {
            points[i + 1]
        });
        let normal = if floating {
            [0.0, 1.0, 0.0]
        } else {
            image.normal(p1[0], p1[2])
        };
        let behind = normalized3(sub3(p1, p0));
        let ahead = normalized3(sub3(p2, p1));
        let mut miter = cross(
            [
                behind[0] + ahead[0],
                behind[1] + ahead[1],
                behind[2] + ahead[2],
            ],
            normal,
        );
        let length = dot3(miter, cross(ahead, normal));
        if length.abs() > 0.000001 {
            miter = miter.map(|value| value / length);
        }
        let lifted = [
            p1[0] + normal[0] * OVERLAY_VOFFSET,
            p1[1] + normal[1] * OVERLAY_VOFFSET,
            p1[2] + normal[2] * OVERLAY_VOFFSET,
        ];
        push(lifted, miter, [0.0, v]);
        push(lifted, miter, [1.0, v]);
        let across = miter.map(|value| value * thickness);
        corners.push((add3(lifted, across), sub3(lifted, across)));
        v = 1.0 - v;
    }
    for pair in 1..n as u32 {
        let (a, b) = (first + 2 * (pair - 1), first + 2 * (pair - 1) + 1);
        let (c, d) = (first + 2 * pair, first + 2 * pair + 1);
        indices.extend_from_slice(&[a, b, c, c, b, d]);
    }

    let centroid = |(a, b): ([f32; 3], [f32; 3])| {
        [
            0.5 * (a[0] + b[0]),
            0.5 * (a[1] + b[1]),
            0.5 * (a[2] + b[2]),
        ]
    };
    let (last, before_last) = (corners[n - 1], corners[n - 2]);
    // The order of the corners decides which side the cap is on.
    line_cap(
        end_cap,
        last.0,
        last.1,
        normalized3(sub3(centroid(last), centroid(before_last))),
        thickness,
        &mut push,
        vertices_len(first, n),
        indices,
    );
    let start_offset = vertices_len(first, n) + cap_vertex_count(end_cap);
    line_cap(
        start_cap,
        corners[0].1,
        corners[0].0,
        normalized3(sub3(centroid(corners[0]), centroid(corners[1]))),
        thickness,
        &mut push,
        start_offset,
        indices,
    );
}

fn vertices_len(first: u32, pairs: usize) -> u32 {
    first + 2 * pairs as u32
}

fn cap_vertex_count(cap: u8) -> u32 {
    match cap {
        CAP_ROUND => ROUND_CAP_POINTS + 1,
        CAP_SHARP => SHARP_CAP_POINTS + 1,
        CAP_SQUARE => 5,
        _ => 0,
    }
}

/// `CreateLineCap`'s samples along a round cap, corners included, and a
/// sharp cap's single point ahead.
const ROUND_CAP_POINTS: u32 = 8;
const SHARP_CAP_POINTS: u32 = 3;

/// `CTexturedLineRData::CreateLineCap`; its vertices start at `offset`.
#[allow(clippy::too_many_arguments)]
fn line_cap(
    cap: u8,
    corner1: [f32; 3],
    corner2: [f32; 3],
    direction: [f32; 3],
    thickness: f32,
    push: &mut impl FnMut([f32; 3], [f32; 3], [f32; 2]),
    offset: u32,
    indices: &mut Vec<u32>,
) {
    let center = [
        0.5 * (corner1[0] + corner2[0]),
        0.5 * (corner1[1] + corner2[1]),
        0.5 * (corner1[2] + corner2[2]),
    ];
    let none = [0.0; 3];
    match cap {
        CAP_ROUND | CAP_SHARP => {
            let (points, radius, center_u) = if cap == CAP_SHARP {
                // A point ahead, pushed further out to look sharper, and a
                // nudge so the texture meets the corners.
                (SHARP_CAP_POINTS, thickness * 1.5, 0.480)
            } else {
                (ROUND_CAP_POINTS, thickness, 0.5)
            };
            let step = -std::f32::consts::PI / (points - 1) as f32;
            let base = normalized3(sub3(corner2, center)).map(|value| value * radius);
            let axis = normalized3(cross(direction, base));
            push(center, none, [center_u, 0.5]);
            push(corner2, none, [0.0, 0.0]);
            for i in 1..points - 1 {
                let rotated = rotate(base, axis, i as f32 * step);
                let v = (i as f32 / (points - 1) as f32).clamp(0.0, 1.0);
                push(add3(center, rotated), none, [0.0, v]);
            }
            push(corner1, none, [0.0, 1.0]);
            for i in 1..points {
                indices.extend_from_slice(&[offset, offset + i, offset + i + 1]);
            }
        }
        CAP_SQUARE => {
            let ahead = direction.map(|value| value * thickness);
            push(center, none, [0.5, 0.5]);
            push(corner2, none, [0.0, 0.0]);
            push(add3(corner2, ahead), none, [0.0, 0.33333]);
            push(add3(corner1, ahead), none, [0.0, 0.66666]);
            push(corner1, none, [0.0, 1.0]);
            for i in 1..4 {
                indices.extend_from_slice(&[offset, offset + i, offset + i + 1]);
            }
        }
        _ => {}
    }
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `CQuaternion::FromAxisAngle` then `Rotate`, as Rodrigues' formula.
fn rotate(vector: [f32; 3], axis: [f32; 3], angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    let across = cross(axis, vector);
    let along = dot3(axis, vector) * (1.0 - cos);
    [
        vector[0] * cos + across[0] * sin + axis[0] * along,
        vector[1] * cos + across[1] * sin + axis[1] * along,
        vector[2] * cos + across[2] * sin + axis[2] * along,
    ]
}

/// A border line to draw, before it is laid on the ground.
struct Line {
    color: u32,
    blinking: bool,
    points: Vec<[f32; 2]>,
}

/// The ground lines and what draws them.
pub struct Borders {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    /// The territory generation and palette `lines` were made from.
    made_from: Option<(u32, Vec<u32>)>,
    lines: Vec<Line>,
    /// Set when `lines` or the ground changed since the last upload.
    stale: bool,
}

impl Borders {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        view_layout: &wgpu::BindGroupLayout,
    ) -> Result<Self, String> {
        let (width, height, base, mask) = decode_textures(BORDER_TEXTURES)?;
        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        let base = upload_texture(device, queue, "territory border", width, height, rgba, base);
        let mask = upload_texture(
            device,
            queue,
            "territory border mask",
            width,
            height,
            rgba,
            mask,
        );
        // Across the line the texture is clamped, as the engine's
        // CLAMP_TO_BORDER, which every edge texel here already is.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("territory border"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: ANISOTROPY,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("territory border params"),
            size: PARAMS_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("territory border"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(PARAMS_SIZE),
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("territory border"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&base),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&mask),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("territory border"),
            source: wgpu::ShaderSource::Wgsl(include_str!("borders.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("territory border"),
            bind_group_layouts: &[Some(view_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("territory border"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            // An overlay on the ground: hidden behind what stands in front,
            // hiding nothing itself.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                // Seen at a grazing angle, a widened line lies so close to
                // the ground it is lifted off that the ground would win.
                bias: wgpu::DepthBiasState {
                    constant: -2,
                    slope_scale: -1.0,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let vertices = create_buffer(
            device,
            queue,
            "territory border vertices",
            &[],
            wgpu::BufferUsages::VERTEX,
        );
        let indices = create_buffer(
            device,
            queue,
            "territory border indices",
            &[],
            wgpu::BufferUsages::INDEX,
        );
        Ok(Borders {
            pipeline,
            bind_group,
            params,
            vertices,
            indices,
            index_count: 0,
            made_from: None,
            lines: Vec::new(),
            stale: false,
        })
    }

    /// Traces the lines again if the territory or the colours changed, as
    /// `CCmpTerritoryManager::UpdateBoundaryLines`.
    pub fn set_territory(&mut self, territory: &Territory, generation: u32, palette: &[u32]) {
        if self
            .made_from
            .as_ref()
            .is_some_and(|(made, colors)| *made == generation && colors == palette)
        {
            return;
        }
        self.made_from = Some((generation, palette.to_vec()));
        self.lines = boundaries(territory)
            .into_iter()
            .filter(|boundary| !boundary.points.is_empty())
            .map(|mut boundary| {
                smooth_points_average(&mut boundary.points);
                interpolate_points_rns(&mut boundary.points);
                Line {
                    color: palette
                        .get(usize::from(boundary.owner))
                        .copied()
                        .unwrap_or(MISSING_COLOR),
                    blinking: boundary.blinking,
                    points: boundary.points,
                }
            })
            .collect();
        self.stale = true;
    }

    /// The lines follow the ground, so new ground needs new lines.
    pub fn ground_changed(&mut self) {
        self.stale = true;
    }

    /// Lays the lines on `image` if they or it changed, and sets this
    /// frame's widths and blink phase.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: Option<&TerrainImage>,
        metres_per_pixel: f32,
        device_pixel_ratio: f32,
        seconds: f32,
    ) {
        if self.stale {
            let Some(image) = image else {
                self.index_count = 0;
                return;
            };
            self.stale = false;
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            for line in &self.lines {
                ribbon(
                    &line.points,
                    line.color,
                    if line.blinking { 1.0 } else { 0.0 },
                    image,
                    &mut vertices,
                    &mut indices,
                );
            }
            let index_bytes: Vec<u8> = indices
                .iter()
                .flat_map(|index| index.to_le_bytes())
                .collect();
            self.vertices = create_buffer(
                device,
                queue,
                "territory border vertices",
                &vertices,
                wgpu::BufferUsages::VERTEX,
            );
            self.indices = create_buffer(
                device,
                queue,
                "territory border indices",
                &index_bytes,
                wgpu::BufferUsages::INDEX,
            );
            self.index_count = indices.len() as u32;
        }
        let params = [
            metres_per_pixel,
            MIN_PIXELS * device_pixel_ratio,
            seconds,
            0.0,
        ];
        let bytes: Vec<u8> = params
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        queue.write_buffer(&self.params, 0, &bytes);
    }

    /// Expects the view bind group at 0.
    pub fn draw(&self, pass: &mut wgpu::RenderPass) {
        if self.index_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// `CCmpTerritoryManager::Interpolate`'s blink period, for a clock that
/// stays precise in f32.
pub fn blink_seconds(milliseconds: f64) -> f32 {
    ((milliseconds / 1000.0) % 2.0) as f32
}

/// (width, height, base RGBA8, mask RGBA8).
fn decode_textures(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>, Vec<u8>), String> {
    let (&version, rest) = bytes.split_first().ok_or("territory-border.bin is empty")?;
    if version != BORDER_VERSION {
        return Err("unknown territory-border.bin version".into());
    }
    let size = |at: usize| -> Result<u32, String> {
        rest.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| "territory-border.bin is cut short".into())
    };
    let width = size(0)?;
    let height = size(4)?;
    let length = width as usize * height as usize * 4;
    let pixels = &rest[8..];
    if pixels.len() != 2 * length {
        return Err("territory-border.bin has the wrong length".into());
    }
    Ok((
        width,
        height,
        pixels[..length].to_vec(),
        pixels[length..].to_vec(),
    ))
}

/// With a box-filtered mip chain: a line a few pixels wide shows the
/// texture's 32 texels minified. `format` is `Rgba8Unorm` or `R8Unorm`.
pub fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    pixels: Vec<u8>,
) -> wgpu::TextureView {
    let channels = if format == wgpu::TextureFormat::R8Unorm {
        1
    } else {
        4
    };
    let mut levels = vec![(width, height, pixels)];
    while let Some((w, h, pixels)) = levels.last().filter(|(w, h, _)| *w > 1 || *h > 1) {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * channels) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for channel in 0..channels {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (2 * x + dx).min(w - 1);
                        let sy = (2 * y + dy).min(h - 1);
                        sum += u32::from(pixels[((sy * w + sx) * channels + channel) as usize]);
                    }
                    next[((y * nw + x) * channels + channel) as usize] = ((sum + 2) / 4) as u8;
                }
            }
        }
        levels.push((nw, nh, next));
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (w, h, pixels)) in levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * channels),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: *w,
                height: *h,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&Default::default())
}
