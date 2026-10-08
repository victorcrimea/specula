//! The engine's fancy water: the surface `CPatchRData::BuildWater` lays
//! over every tile near water, with `WaterManager`'s wind strength and
//! depth at each vertex, drawn with water.wgsl as
//! `TerrainRenderer::RenderFancyWater` draws it, animated through the
//! water type's 60 normal map frames, and with the fancy effects the
//! shore waves `WaterManager::CreateWaveMeshes` lays along the coasts.
//!
//! `WaterManager` works the wind and the shore distance out against the
//! water height it held before the map set its own (CCmpWaterManager's
//! `RecomputeWaterData` runs before it stores the new height), so in the
//! game they depend on the map played before. This takes the map's own.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::rc::Rc;

use specula_assets::files::FileRef;
use specula_assets::files::Texture;

use crate::terrain::TILE_SIZE;
use crate::terrain::TerrainImage;

/// Engine heights are u16 in these units.
const HEIGHT_UNITS_PER_METRE: f32 = 92.0;
/// `WaterManager`'s m_NormalMap.
pub const FRAMES: usize = 60;
/// `GetCurrentTextureIndex(8.0)`: 60 frames over 8 s of sim time.
const FRAME_PERIOD: f64 = 8.0;
/// f32 x 3 position, f32 x 2 wind strength and depth.
pub const VERTEX_SIZE: usize = 20;
/// water.wgsl's `Water`.
const UNIFORM_SIZE: u64 = 288;
/// `RecomputeDistanceHeightmap`'s lookahead.
const MAX_SHORE_DISTANCE: usize = 5;
/// `CreateWaveMeshes`' widest wave, in coastal points.
const WAVE_POINTS: usize = 14;
/// f32 x 3 base, apex, splash and retreat positions, f32 x 2 the coast's
/// perpendicular, f32 x 2 uv, f32 the wave's time offset and f32 its
/// width (waves.wgsl takes as attributes what the engine sets per wave).
pub const WAVE_VERTEX_SIZE: usize = 72;

/// The water surface's vertices and indices, and with the fancy effects
/// the shore tiles: the tiles partly over land, laid on the ground, which
/// the waves may run up.
pub struct Surface {
    pub vertices: Vec<u8>,
    pub indices: Vec<u32>,
    pub shore_vertices: Vec<u8>,
    pub shore_indices: Vec<u32>,
}

/// The engine's ground height in metres, clamped to the map as
/// `CTerrain::GetVertexGroundLevel` clamps.
fn ground(image: &TerrainImage, i: isize, j: isize) -> f32 {
    let side = image.size as isize + 1;
    let i = i.clamp(0, side - 1) as usize;
    let j = j.clamp(0, side - 1) as usize;
    f32::from(image.heights[j * side as usize + i]) / HEIGHT_UNITS_PER_METRE
}

/// `CPatchRData::BuildWater` over the whole map: a quad for every tile
/// with water at one of ten vertices around it, each vertex at the water
/// level, carrying its wind strength and its depth.
pub fn surface(image: &TerrainImage) -> Surface {
    let size = image.size as isize;
    let side = image.size as usize + 1;
    let level = image.water_level;
    let wind = wind_strength(image);
    // Where to look for water around a tile.
    const CHECK: [(isize, isize); 10] = [
        (0, 0),
        (1, 0),
        (2, 0),
        (0, 1),
        (0, 2),
        (1, 1),
        (2, 2),
        (-1, 0),
        (0, -1),
        (-1, -1),
    ];
    let mut index_of = vec![u32::MAX; side * side];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut shore_index_of = vec![u32::MAX; side * side];
    let mut shore_vertices = Vec::new();
    let mut shore_indices = Vec::new();
    for z in 0..size {
        for x in 0..size {
            if !CHECK
                .iter()
                .any(|(dx, dz)| ground(image, x + dx, z + dz) < level)
            {
                continue;
            }
            let mut corner = |dx: isize, dz: isize| {
                let (i, j) = ((x + dx) as usize, (z + dz) as usize);
                let at = j * side + i;
                if index_of[at] == u32::MAX {
                    index_of[at] = (vertices.len() / VERTEX_SIZE) as u32;
                    let depth = level - ground(image, i as isize, j as isize);
                    for value in [
                        i as f32 * TILE_SIZE,
                        level,
                        j as f32 * TILE_SIZE,
                        wind[at],
                        depth,
                    ] {
                        vertices.extend_from_slice(&value.to_le_bytes());
                    }
                }
                index_of[at]
            };
            let quad = [corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1)];
            indices.extend_from_slice(&[quad[2], quad[0], quad[1], quad[1], quad[3], quad[2]]);

            if [(0, 0), (1, 0), (0, 1), (1, 1)]
                .iter()
                .all(|(dx, dz)| ground(image, x + dx, z + dz) < level)
            {
                continue;
            }
            // Just over the ground, carrying a depth of -5 that
            // water_high reads as shore.
            let mut corner = |dx: isize, dz: isize| {
                let (i, j) = ((x + dx) as usize, (z + dz) as usize);
                let at = j * side + i;
                if shore_index_of[at] == u32::MAX {
                    shore_index_of[at] = (shore_vertices.len() / VERTEX_SIZE) as u32;
                    for value in [
                        i as f32 * TILE_SIZE,
                        ground(image, i as isize, j as isize) + 0.02,
                        j as f32 * TILE_SIZE,
                        0.0,
                        -5.0,
                    ] {
                        shore_vertices.extend_from_slice(&value.to_le_bytes());
                    }
                }
                shore_index_of[at]
            };
            let quad = [corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1)];
            if image.triangulation_dir(x as usize, z as usize) {
                shore_indices
                    .extend_from_slice(&[quad[2], quad[0], quad[1], quad[1], quad[3], quad[2]]);
            } else {
                shore_indices
                    .extend_from_slice(&[quad[3], quad[2], quad[0], quad[3], quad[0], quad[1]]);
            }
        }
    }
    Surface {
        vertices,
        indices,
        shore_vertices,
        shore_indices,
    }
}

/// `WaterManager::RecomputeDistanceHeightmap`: per vertex, the distance
/// along its row or its column to the nearest vertex at or above the
/// water, up to `MAX_SHORE_DISTANCE`. Ahead, the map's edge counts as
/// such a vertex, as `ComputeDirection`'s lookahead stops there.
fn distance_heightmap(image: &TerrainImage) -> Vec<f32> {
    let side = image.size as usize + 1;
    let level = image.water_level;
    let mut distance = vec![MAX_SHORE_DISTANCE as f32; side * side];
    for transpose in [false, true] {
        for id1 in 0..side {
            let at = |id2: usize| if transpose { (id1, id2) } else { (id2, id1) };
            let above = |id2: usize| {
                let (x, z) = at(id2);
                ground(image, x as isize, z as isize) >= level
            };
            let advance = |lookahead: &mut usize, id2: usize| {
                while *lookahead <= id2 + MAX_SHORE_DISTANCE
                    && *lookahead < side
                    && !above(*lookahead)
                {
                    *lookahead += 1;
                }
            };
            let mut shore = if above(0) { 0 } else { MAX_SHORE_DISTANCE };
            let mut lookahead = usize::from(shore > 0);
            advance(&mut lookahead, 0);
            for id2 in 0..side {
                shore = if above(id2) {
                    0
                } else {
                    (shore + 1).min(MAX_SHORE_DISTANCE)
                };
                if lookahead == id2 {
                    lookahead += 1;
                }
                advance(&mut lookahead, id2);
                let (x, z) = at(id2);
                let cell = &mut distance[z * side + x];
                *cell = cell.min((lookahead - id2).min(shore) as f32);
            }
        }
    }
    distance
}

/// The C library's `rand` as the engine calls it for the waves' time
/// offsets: any sequence will do, so a fixed one keeps a map's waves the
/// same on every viewer.
struct Random(u32);

impl Random {
    fn next(&mut self) -> u32 {
        // Numerical Recipes' LCG, its high bits.
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0 >> 16
    }
}

/// `WaterManager::CreateWaveMeshes`: waves along every coast long enough,
/// each a strip `WAVE_POINTS` coastal points wide at most and 9 vertices
/// deep, every vertex with its four positions the wave moves through.
/// None where the water is calm, as the engine skips them.
pub fn waves(image: &TerrainImage) -> Surface {
    let mut out = Surface {
        vertices: Vec::new(),
        indices: Vec::new(),
        shore_vertices: Vec::new(),
        shore_indices: Vec::new(),
    };
    let water = &image.water;
    if water.waviness < 5.0 && water.kind != "ocean" {
        return out;
    }
    let side = image.size as usize + 1;
    let level = image.water_level;
    let distance = distance_heightmap(image);

    // The points near the coast, on the water's side.
    let mut coastal = BTreeSet::new();
    for z in 1..side.saturating_sub(1) {
        for x in 1..side - 1 {
            let d = distance[z * side + x];
            if d > 0.5 && d < 1.5 {
                coastal.insert(z * side + x);
            }
        }
    }

    // Chains of them, each point at most two neighbours.
    const AROUND: [(isize, isize); 8] = [
        (-1, -1),
        (-1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
        (1, 0),
        (1, -1),
        (0, -1),
    ];
    let point = |index: usize| {
        let (x, y) = (index % side, index / side);
        [x as f32 * TILE_SIZE, y as f32 * TILE_SIZE]
    };
    let neighbour = |index: usize, (dx, dy): (isize, isize)| {
        let (x, y) = ((index % side) as isize, (index / side) as isize);
        ((x + dx) + (y + dy) * side as isize) as usize
    };
    let mut chains: Vec<Vec<[f32; 2]>> = Vec::new();
    while let Some(index) = coastal.pop_first() {
        let mut chain = VecDeque::from([point(index)]);
        let mut neighbours = [None; 2];
        let mut count = 0;
        for step in AROUND {
            let next = neighbour(index, step);
            if coastal.contains(&next) {
                if count < 2 {
                    neighbours[count] = Some(next);
                }
                count += 1;
            }
        }
        if count > 2 {
            continue;
        }
        'directions: for (i, next) in neighbours.into_iter().enumerate() {
            let Some(mut at) = next else {
                continue;
            };
            let push = |chain: &mut VecDeque<[f32; 2]>, at: usize| {
                if i == 0 {
                    chain.push_back(point(at));
                } else {
                    chain.push_front(point(at));
                }
            };
            push(&mut chain, at);
            // Gone already: the chain came round to its start.
            if !coastal.remove(&at) {
                break 'directions;
            }
            // On to the first neighbour left, in `AROUND`'s order, until
            // none is. (The engine's check for a third neighbour there
            // never runs, as it moves on at the first.)
            while let Some(next) = AROUND
                .into_iter()
                .map(|step| neighbour(at, step))
                .find(|next| coastal.contains(next))
            {
                at = next;
                push(&mut chain, at);
                coastal.remove(&at);
            }
        }
        if chain.len() > 10 {
            chains.push(chain.into());
        }
    }

    // Smoothed, three times over, in place.
    for chain in &mut chains {
        for _ in 0..3 {
            for j in 1..chain.len() - 1 {
                let middle = [
                    (chain[j - 1][0] + chain[j + 1][0]) / 2.0,
                    (chain[j - 1][1] + chain[j + 1][1]) / 2.0,
                ];
                chain[j] = [
                    (chain[j][0] + middle[0]) / 2.0,
                    (chain[j][1] + middle[1]) / 2.0,
                ];
            }
        }
    }

    // The waves, along stretches of the chains.
    let mut random = Random(1);
    let mut time_offset = (random.next() % 50) as f32 / 5.0;
    let ground_at = |at: [f32; 2]| image.ground(at[0], at[1]);
    // The coast's normal at `a` points into a stretch from `start`,
    // averaged over the segments either side within the widest wave.
    let perpendicular = |chain: &[[f32; 2]], start: usize, a: usize| {
        let pos = chain[start + a];
        let mut sum = [0.0, 0.0];
        let mut count = 0.0;
        if a > 0 {
            count += 1.0;
            let before = chain[start + a - 1];
            sum = [sum[0] + pos[0] - before[0], sum[1] + pos[1] - before[1]];
        }
        if a < WAVE_POINTS - 1 {
            count += 1.0;
            let after = chain[start + a + 1];
            sum = [sum[0] + after[0] - pos[0], sum[1] + after[1] - pos[1]];
        }
        let along = [sum[0] / count, sum[1] / count];
        let length = along[0].hypot(along[1]);
        if length > 0.0 {
            [-along[1] / length, along[0] / length]
        } else {
            [0.0, 0.0]
        }
    };
    let dot = |a: [f32; 2], b: [f32; 2]| a[0] * b[0] + a[1] * b[1];
    // Offsets along the normal, and heights, of each of a wave's 9 rows
    // at its four stages.
    const PERP_BASE: [f32; 9] = [6.0, 6.05, 6.1, 6.2, 6.3, 6.4, 6.5, 6.6, 9.7];
    const PERP_APEX: [f32; 9] = [2.0, 2.1, 2.2, 2.3, 2.4, 3.0, 3.3, 3.6, 9.5];
    const PERP_SPLASH: [f32; 9] = [1.1, 0.7, -0.2, 0.0, 0.6, 1.3, 2.2, 3.6, 9.0];
    const PERP_RETREAT: [f32; 9] = [2.0, 2.1, 1.2, 1.5, 1.7, 1.9, 2.7, 3.8, 9.0];
    const HEIGHT_BASE: [f32; 9] = [0.0, 0.2, 0.5, 0.8, 0.9, 0.85, 0.6, 0.2, 0.0];
    const HEIGHT_SPLASH: [f32; 9] = [-0.8, -0.4, 0.0, 0.1, 0.1, 0.03, 0.0, 0.0, 0.0];
    const BASE_HEIGHT: f32 = 0.04;
    for chain in &chains {
        let mut j = 0;
        while j + WAVE_POINTS < chain.len() {
            let mut width = WAVE_POINTS;
            let mut outmost = 0.0;
            let mut average_depth = 0.0;
            let mut sign = 1.0;
            let mut first = [0.0, 0.0];
            let mut perp = [0.0, 0.0];
            for a in 0..WAVE_POINTS {
                let last = perp;
                perp = perpendicular(chain, j, a);
                let pos = chain[j + a];
                if a == 0 {
                    first = perp;
                }
                // The coast turns too much: a narrower wave.
                if a > 1 && dot(perp, last) < 0.90 && dot(perp, first) < 0.70 {
                    width = a + 1;
                    break;
                }
                if ground_at([pos[0] + perp[0] * 1.5, pos[1] + perp[1] * 1.5]) > level {
                    sign = -1.0;
                }
                average_depth += ground_at([
                    pos[0] + sign * perp[0] * 20.0,
                    pos[1] + sign * perp[1] * 20.0,
                ]) - level;
                let mut local_outmost = -2.0_f32;
                while local_outmost < 0.0 {
                    let depth = ground_at([
                        pos[0] + sign * perp[0] * local_outmost,
                        pos[1] + sign * perp[1] * local_outmost,
                    ]) - level;
                    if !(0.0..=0.6).contains(&depth) {
                        local_outmost += 0.2;
                    } else {
                        break;
                    }
                }
                outmost += local_outmost;
            }
            if width < 5 {
                j += 7;
                continue;
            }
            outmost /= width as f32;
            if outmost > -0.5 {
                j += 4;
                continue;
            }
            outmost = -2.5 + outmost * water.waviness / 10.0;
            average_depth /= width as f32;
            if average_depth > -1.3 {
                j += 4;
                continue;
            }

            let wave_offset = time_offset;
            time_offset += (random.next() % 100) as f32 / 25.0 + 4.0;
            let first_vertex = (out.vertices.len() / WAVE_VERTEX_SIZE) as u32;
            let mut columns: Vec<[[f32; 18]; 9]> = Vec::with_capacity(width);
            for a in 0..width {
                let perp = perpendicular(chain, j, a);
                let pos = chain[j + a];
                let half_width = (width as f32 - 1.0) / 2.0;
                let sideness = ((half_width - (a as f32 - half_width).abs()) / 3.0)
                    .clamp(0.0, 1.0)
                    .sqrt();
                let at = |offset: f32, height: f32| {
                    let x = pos[0] + sign * perp[0] * offset;
                    let z = pos[1] + sign * perp[1] * offset;
                    let ground = 0.05 + ground_at([x, z]);
                    [x, BASE_HEIGHT + height * sideness + level.max(ground), z]
                };
                // Rows from the outermost, the engine's point[8], in.
                let mut column = [[0.0; 18]; 9];
                for (row, t) in (0..9).rev().enumerate() {
                    let base = at(PERP_BASE[t] + outmost, HEIGHT_BASE[t]);
                    let apex = at(PERP_APEX[t] + outmost, HEIGHT_BASE[t]);
                    let splash = at(PERP_SPLASH[t] + outmost * sideness, HEIGHT_SPLASH[t]);
                    let retreat = at(PERP_RETREAT[t] + outmost, 0.0);
                    column[row] = [
                        base[0],
                        base[1],
                        base[2],
                        apex[0],
                        apex[1],
                        apex[2],
                        splash[0],
                        splash[1],
                        splash[2],
                        retreat[0],
                        retreat[1],
                        retreat[2],
                        perp[0],
                        perp[1],
                        a as f32,
                        (8 - t) as f32,
                        wave_offset,
                        width as f32,
                    ];
                }
                columns.push(column);
            }
            if sign > 0.0 {
                columns.reverse();
            }
            for value in columns.iter().flatten().flatten() {
                out.vertices.extend_from_slice(&value.to_le_bytes());
            }
            for a in 0..width as u32 - 1 {
                for rect in 0..7 {
                    let at = first_vertex + a * 9 + rect;
                    out.indices
                        .extend_from_slice(&[at, at + 9, at + 1, at + 9, at + 10, at + 1]);
                }
            }
            j += width / 2;
        }
    }
    out
}

/// `WaterManager::RecomputeWindStrength`: wind marched across the map
/// from the edges it blows in from, slowed by rising ground and sped up
/// over falling ground, drifting back to 1 in between.
fn wind_strength(image: &TerrainImage) -> Vec<f32> {
    let side = image.size as usize + 1;
    let level = image.water_level;
    let mut strength = vec![0.0; side * side];
    let angle = image.water.wind_angle;
    let (dir_x, dir_y) = (angle.cos(), angle.sin());
    let step_size = 10.0;
    let mut wind_x = -(step_size * dir_x).round() as isize;
    let mut wind_y = -(step_size * dir_y).round() as isize;
    let mut starts: Vec<(isize, isize)> = Vec::new();
    let mut movement: Vec<(isize, isize)> = Vec::new();
    let last = side as isize - 1;
    if dir_x.abs() < 0.01 {
        movement.push((0, if wind_y > 0 { 1 } else { -1 }));
        let start = if wind_y > 0 { 0 } else { last };
        starts.extend((0..side as isize).map(|x| (x, start)));
    } else if dir_y.abs() < 0.01 {
        movement.push((if wind_x > 0 { 1 } else { -1 }, 0));
        let start = if wind_x > 0 { 0 } else { last };
        starts.extend((0..side as isize).map(|z| (start, z)));
    } else {
        let start = if wind_y > 0 { 0 } else { last };
        starts.extend((0..side as isize).map(|x| (x, start)));
        // Without repeating the corner.
        let start = if wind_x > 0 { 0 } else { last };
        if wind_y > 0 {
            starts.extend((1..side as isize).map(|z| (start, z)));
        } else {
            starts.extend((0..last).map(|z| (start, z)));
        }
        while wind_x != 0 || wind_y != 0 {
            let step = (wind_x.signum(), wind_y.signum());
            wind_x -= step.0;
            wind_y -= step.1;
            movement.push(step);
        }
    }
    for (mut x, mut y) in starts {
        let at = |x: isize, y: isize| y as usize * side + x as usize;
        // Starting at 1 unless in shallow water.
        let depth = level - ground(image, x, y);
        let mut wind = if depth > 0.0 && depth < 2.0 {
            depth / 2.0
        } else {
            1.0
        };
        strength[at(x, y)] = wind;
        'march: loop {
            for &(step_x, step_y) in &movement {
                wind = 0.15 + wind * 0.85;
                let height_diff = ground(image, x + step_x, y + step_y).max(level)
                    - ground(image, x, y).max(level);
                wind = if height_diff > 0.0 {
                    (wind + height_diff.min(4.0) / 40.0).min(2.0)
                } else {
                    (wind + height_diff.max(-4.0) / 5.0).max(0.0)
                };
                x += step_x;
                y += step_y;
                if x < 0 || x > last || y < 0 || y > last {
                    break 'march;
                }
                strength[at(x, y)] = wind;
            }
        }
    }
    strength
}

/// The waveParams uniforms `RenderFancyWater` sets by water type.
fn wave_params(kind: &str) -> [[f32; 4]; 2] {
    match kind {
        "clap" => [[30.0, 1.5, 20.0, 0.03], [0.5, 0.0, 0.0, 0.0]],
        "lake" => [[8.5, 1.5, 15.0, 0.03], [0.2, 0.0, 0.0, 0.07]],
        _ => [[15.0, 0.8, 10.0, 0.1], [0.3, 0.0, 0.1, 0.3]],
    }
}

/// `WaterManager::GetCurrentTextureIndex(8.0)` and the frame after it.
fn frames(time: f64) -> (usize, usize) {
    let current = (time * FRAMES as f64 / FRAME_PERIOD) as usize % FRAMES;
    (current, (current + 1) % FRAMES)
}

/// The camera the refraction is drawn from: the view's, as
/// `ComputeRefractionCamera` makes it, and the reflection's view-projection.
pub struct WaterCamera {
    /// Its view-projection, to project the water onto its texture.
    pub view_proj: [[f32; 4]; 4],
    /// The view's axes and where it is, and its projection's x and y
    /// scale, to rebuild a world position from the texture's depth.
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    pub scale: [f32; 2],
    pub near: f32,
    pub far: f32,
    /// `ComputeReflectionCamera`'s: the refraction's, of the scene
    /// mirrored in the water.
    pub reflection: [[f32; 4]; 4],
}

/// The uniform for water.wgsl's `Water`.
pub fn uniform(
    image: &TerrainImage,
    time: f64,
    sky_rotation: f32,
    refraction: &WaterCamera,
) -> Vec<u8> {
    let water = &image.water;
    let [wave1, wave2] = wave_params(&water.kind);
    let (current, next) = frames(time);
    let (wind_sin, wind_cos) = (-water.wind_angle).sin_cos();
    let (sky_sin, sky_cos) = (std::f32::consts::PI + sky_rotation).sin_cos();
    let [r, g, b] = water.color;
    let [tint_r, tint_g, tint_b] = water.tint;
    let values = [
        r,
        g,
        b,
        water.waviness,
        tint_r,
        tint_g,
        tint_b,
        water.murkiness,
        wave1[0],
        wave1[1],
        wave1[2],
        wave1[3],
        wave2[0],
        wave2[1],
        wave2[2],
        wave2[3],
        // The engine passes its double timer as a float too.
        time as f32,
        wind_cos,
        wind_sin,
        0.0,
        current as f32,
        next as f32,
        sky_cos,
        sky_sin,
    ];
    let mut bytes: Vec<u8> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    // WGSL matrices are column-major.
    for column in 0..4 {
        for row in &refraction.view_proj {
            bytes.extend_from_slice(&row[column].to_le_bytes());
        }
    }
    let [right_x, right_y, right_z] = refraction.right;
    let [up_x, up_y, up_z] = refraction.up;
    let [forward_x, forward_y, forward_z] = refraction.forward;
    for value in [
        right_x,
        right_y,
        right_z,
        refraction.scale[0],
        up_x,
        up_y,
        up_z,
        refraction.scale[1],
        forward_x,
        forward_y,
        forward_z,
        0.0,
        refraction.near,
        refraction.far,
        0.0,
        0.0,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for column in 0..4 {
        for row in &refraction.reflection {
            bytes.extend_from_slice(&row[column].to_le_bytes());
        }
    }
    bytes
}

/// A water type's frames as one array texture, uploaded as they arrive;
/// drawn only once all are in, as a missing frame would flash.
pub struct Normals {
    pub kind: String,
    pub files: Vec<FileRef>,
    texture: Option<wgpu::Texture>,
    /// The frames uploaded, by layer.
    uploaded: Vec<bool>,
    pub view: Option<wgpu::TextureView>,
}

impl Normals {
    pub fn new(kind: String, files: Vec<FileRef>) -> Self {
        let uploaded = vec![false; files.len()];
        Normals {
            kind,
            files,
            texture: None,
            uploaded,
            view: None,
        }
    }

    /// Uploads `frame`, whole as the texture quality has it; the first one
    /// in sets the array's size, format and mips, which the others must
    /// share.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: usize,
        texture: &Rc<Texture>,
        bc_supported: bool,
    ) -> Result<(), String> {
        if self.uploaded[frame] {
            return Ok(());
        }
        let format = crate::field::gpu_format(texture, bc_supported);
        let gpu = self.texture.get_or_insert_with(|| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("water normals"),
                size: wgpu::Extent3d {
                    width: texture.width,
                    height: texture.height,
                    depth_or_array_layers: FRAMES as u32,
                },
                mip_level_count: texture.levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let size = gpu.size();
        if (texture.width, texture.height) != (size.width, size.height)
            || texture.levels.len() as u32 != gpu.mip_level_count()
            || format != gpu.format()
        {
            return Err(format!(
                "water {} frame {} differs from the first",
                self.kind,
                frame + 1
            ));
        }
        crate::field::write_levels(queue, gpu, texture, 0, frame as u32, bc_supported);
        self.uploaded[frame] = true;
        if self.uploaded.iter().all(|done| *done) {
            self.view = self.texture.as_ref().map(|gpu| {
                gpu.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("water normals"),
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                })
            });
        }
        Ok(())
    }
}

/// Which of water_high's options a pipeline is compiled with, as the
/// game's water settings choose them.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Options {
    pub refraction: bool,
    pub real_depth: bool,
    pub reflection: bool,
    pub fancy: bool,
}

/// The textures the refraction and reflection passes draw into, as
/// `WaterManager` makes them: square, the screen's height rounded up to a
/// power of two.
pub struct Targets {
    pub side: u32,
    pub refraction: wgpu::TextureView,
    pub refraction_depth: wgpu::TextureView,
    pub reflection: wgpu::TextureView,
    pub reflection_depth: wgpu::TextureView,
}

impl Targets {
    /// Their side for a screen `screen_height` high.
    pub fn side(device: &wgpu::Device, screen_height: u32) -> u32 {
        screen_height
            .max(1)
            .next_power_of_two()
            .min(device.limits().max_texture_dimension_2d)
    }

    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, screen_height: u32) -> Self {
        let side = Self::side(device, screen_height);
        let target = |label: &str, format: wgpu::TextureFormat| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: side,
                        height: side,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Targets {
            side,
            refraction: target("water refraction", format),
            refraction_depth: target("water refraction depth", crate::unit::DEPTH_FORMAT),
            reflection: target("water reflection", format),
            reflection_depth: target("water reflection depth", crate::unit::DEPTH_FORMAT),
        }
    }
}

/// The water's pipelines, made as the options are first asked for, and
/// what they bind that does not change with the map.
pub struct Water {
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    pipelines: HashMap<Options, wgpu::RenderPipeline>,
    pub layout: wgpu::BindGroupLayout,
    pub environment_layout: wgpu::BindGroupLayout,
    pub buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    refraction_sampler: wgpu::Sampler,
    /// Bound for the refraction while it is not drawn, as the shader then
    /// reads none; never a target of the view's pass.
    stand_in: wgpu::TextureView,
}

impl Water {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        view_layout: &wgpu::BindGroupLayout,
        light_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water"),
            source: wgpu::ShaderSource::Wgsl(include_str!("water.wgsl").into()),
        });
        let texture_entry = |binding, view_dimension, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                texture_entry(1, wgpu::TextureViewDimension::D2Array, true),
                sampler_entry(2),
            ],
        });
        // The refraction's depth is fetched as unfilterable float, which
        // WebGL2 can do with a depth texture, as the DOF pass does.
        let environment_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("water environment"),
                entries: &[
                    texture_entry(0, wgpu::TextureViewDimension::Cube, true),
                    sampler_entry(1),
                    texture_entry(2, wgpu::TextureViewDimension::D2, true),
                    texture_entry(3, wgpu::TextureViewDimension::D2, false),
                    sampler_entry(4),
                    texture_entry(5, wgpu::TextureViewDimension::D2, true),
                    texture_entry(6, wgpu::TextureViewDimension::D2, true),
                ],
            });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Repeating, mipmapped and anisotropic, as ReloadWaterNormalTextures
        // loads them.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water normals"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        // Linear and mirrored, as WaterManager makes the refraction and
        // reflection textures.
        let refraction_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water refraction"),
            address_mode_u: wgpu::AddressMode::MirrorRepeat,
            address_mode_v: wgpu::AddressMode::MirrorRepeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water"),
            bind_group_layouts: &[
                Some(view_layout),
                Some(&layout),
                Some(&environment_layout),
                Some(light_layout),
            ],
            immediate_size: 0,
        });
        let stand_in = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("water stand-in"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Water {
            shader,
            pipeline_layout,
            format,
            pipelines: HashMap::new(),
            layout,
            environment_layout,
            buffer,
            sampler,
            refraction_sampler,
            stand_in,
        }
    }

    /// Makes the pipeline for `options` if it is not yet; WebGL2 compiles
    /// each, so only those drawn are made.
    pub fn prepare(&mut self, device: &wgpu::Device, options: Options) {
        if self.pipelines.contains_key(&options) {
            return;
        }
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        let constants = [
            ("REFRACTION", flag(options.refraction)),
            ("REAL_DEPTH", flag(options.refraction && options.real_depth)),
            ("REFLECTION", flag(options.reflection)),
            ("FANCY", flag(options.fancy)),
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_water"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            // water_high.xml tests and writes depth.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::unit::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_water"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.insert(options, pipeline);
    }

    /// The pipeline `prepare` made for `options`.
    pub fn pipeline(&self, options: Options) -> Option<&wgpu::RenderPipeline> {
        self.pipelines.get(&options)
    }

    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        normals: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(normals),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// The sky cube, the refraction and reflection passes' textures and
    /// the fancy effects', while they are drawn.
    pub fn environment_group(
        &self,
        device: &wgpu::Device,
        sky: (&wgpu::TextureView, &wgpu::Sampler),
        targets: Option<&Targets>,
        effects: Option<&Effects>,
    ) -> wgpu::BindGroup {
        let effects = effects.map_or(&self.stand_in, |effects| &effects.color);
        let (refraction, refraction_depth, reflection) = match targets {
            Some(targets) => (
                &targets.refraction,
                &targets.refraction_depth,
                &targets.reflection,
            ),
            None => (&self.stand_in, &self.stand_in, &self.stand_in),
        };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water environment"),
            layout: &self.environment_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(sky.0),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sky.1),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(refraction),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(refraction_depth),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.refraction_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(reflection),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(effects),
                },
            ],
        })
    }
}

/// The fancy effects' textures, `WaterManager`'s m_FancyTexture and its
/// depth: the screen's size.
pub struct Effects {
    pub size: (u32, u32),
    pub color: wgpu::TextureView,
    pub depth: wgpu::TextureView,
}

impl Effects {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let target = |label: &str, format: wgpu::TextureFormat| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: width.max(1),
                        height: height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Effects {
            size: (width, height),
            color: target("water effects", wgpu::TextureFormat::Rgba8Unorm),
            depth: target("water effects depth", crate::unit::DEPTH_FORMAT),
        }
    }
}

/// The pipelines that draw the fancy effects: the shore waves
/// (water_waves.xml) and the shore tiles over them (the solid shader with
/// a depth test).
pub struct Waves {
    pub wave_pipeline: wgpu::RenderPipeline,
    pub occluder_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Waves {
    pub fn new(device: &wgpu::Device, view_layout: &wgpu::BindGroupLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water waves"),
            source: wgpu::ShaderSource::Wgsl(include_str!("waves.wgsl").into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
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
            label: Some("water waves"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
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
        // Repeating, as LoadWaterTextures loads both.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water waves"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let wave_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water waves"),
            bind_group_layouts: &[Some(view_layout), Some(&layout)],
            immediate_size: 0,
        });
        let occluder_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water foam occluders"),
            bind_group_layouts: &[Some(view_layout)],
            immediate_size: 0,
        });
        // `<blend src="src_alpha" dst="one_minus_src_alpha"/>` blends the
        // alpha the same way, which the water reads.
        let blend = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let wave_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water waves"),
            layout: Some(&wave_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_wave"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: WAVE_VERTEX_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x3,
                        2 => Float32x3,
                        3 => Float32x3,
                        4 => Float32x2,
                        5 => Float32x2,
                        6 => Float32x2,
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            // water_waves.xml: a depth test that always passes, and writes,
            // so the shore tiles test against the waves.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::unit::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_wave"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState {
                        color: blend,
                        alpha: blend,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let occluder_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water foam occluders"),
            layout: Some(&occluder_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_occluder"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::unit::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_occluder"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        Waves {
            wave_pipeline,
            occluder_pipeline,
            layout,
            sampler,
        }
    }

    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        water: &wgpu::Buffer,
        wave: &wgpu::TextureView,
        foam: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water waves"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: water.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(wave),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(foam),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}
