//! What the engine's water derives from the whole heightmap, which a
//! seat's client cannot work out from the terrain it was sent: the shore
//! waves of `WaterManager::CreateWaveMeshes` and the wind strength of
//! `RecomputeWindStrength`. The server makes them for a seat; the client
//! makes them itself for a whole terrain.

use std::collections::BTreeSet;
use std::collections::VecDeque;

use crate::wire::HEIGHT_UNITS_PER_METRE;

/// `TERRAIN_TILE_SIZE`, in metres.
pub const TILE_SIZE: f32 = 4.0;
/// `RecomputeDistanceHeightmap`'s lookahead.
const MAX_SHORE_DISTANCE: usize = 5;
/// `CreateWaveMeshes`' widest wave, in coastal points.
pub const WAVE_POINTS: usize = 14;
/// f32 x 3 base, apex, splash and retreat positions, f32 x 2 the coast's
/// perpendicular, f32 x 2 uv, f32 the wave's time offset and f32 its
/// width (waves.wgsl takes as attributes what the engine sets per wave).
pub const WAVE_VERTEX_SIZE: usize = 72;
/// The rows of a wave, from the outermost in.
pub const WAVE_ROWS: usize = 9;
/// The floats of a wave vertex; the 15th is its column `a`.
pub const WAVE_FLOATS: usize = 18;

/// The engine's heights: (size + 1)^2 vertices, row-major, x fastest.
pub struct Heightmap<'a> {
    /// Tiles per side.
    pub size: u32,
    pub heights: &'a [u16],
}

impl Heightmap<'_> {
    /// In height units, unclamped.
    fn height(&self, i: usize, j: usize) -> f32 {
        f32::from(self.heights[j * (self.size as usize + 1) + i])
    }

    /// The engine's ground height in metres, clamped to the map as
    /// `CTerrain::GetVertexGroundLevel` clamps.
    pub fn vertex(&self, i: isize, j: isize) -> f32 {
        let side = self.size as isize + 1;
        let i = i.clamp(0, side - 1) as usize;
        let j = j.clamp(0, side - 1) as usize;
        self.height(i, j) / HEIGHT_UNITS_PER_METRE
    }

    /// `CTerrain::GetTriangulationDir`: the diagonal whose midpoint is
    /// highest, so ridges stay straight.
    pub fn triangulation_dir(&self, i: usize, j: usize) -> bool {
        let last = self.size as usize - 1;
        let (i, j) = (i.min(last), j.min(last));
        let mid1 = self.height(i, j) + self.height(i + 1, j + 1);
        let mid2 = self.height(i, j + 1) + self.height(i + 1, j);
        mid1 < mid2
    }

    /// `CTerrain::GetExactGroundLevel`, in metres.
    pub fn ground(&self, x: f32, z: f32) -> f32 {
        if self.size == 0 {
            return 0.0;
        }
        let last = self.size as isize - 1;
        let xi = ((x / TILE_SIZE).floor() as isize).clamp(0, last) as usize;
        let zi = ((z / TILE_SIZE).floor() as isize).clamp(0, last) as usize;
        let xf = (x / TILE_SIZE - xi as f32).clamp(0.0, 1.0);
        let zf = (z / TILE_SIZE - zi as f32).clamp(0.0, 1.0);
        let h00 = self.height(xi, zi);
        let h01 = self.height(xi, zi + 1);
        let h10 = self.height(xi + 1, zi);
        let h11 = self.height(xi + 1, zi + 1);
        let units = if self.triangulation_dir(xi, zi) {
            if xf + zf <= 1.0 {
                h00 + (h10 - h00) * xf + (h01 - h00) * zf
            } else {
                h11 + (h01 - h11) * (1.0 - xf) + (h10 - h11) * (1.0 - zf)
            }
        } else if xf <= zf {
            h00 + (h11 - h01) * xf + (h01 - h00) * zf
        } else {
            h00 + (h10 - h00) * xf + (h11 - h10) * zf
        };
        units / HEIGHT_UNITS_PER_METRE
    }
}

/// One wave: its columns along the coast, each `WAVE_ROWS` vertices of
/// `WAVE_FLOATS` floats; a seat's lacks the columns it was not sent.
#[derive(Clone)]
pub struct Wave {
    pub columns: Vec<Option<[[f32; WAVE_FLOATS]; WAVE_ROWS]>>,
}

/// The waves' vertices and indices, as waves.wgsl draws them; only
/// neighbouring columns that are both there are joined.
pub fn wave_mesh(waves: &[Wave]) -> (Vec<u8>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for wave in waves {
        let mut previous: Option<u32> = None;
        for column in &wave.columns {
            let Some(column) = column else {
                previous = None;
                continue;
            };
            let first = (vertices.len() / WAVE_VERTEX_SIZE) as u32;
            for value in column.iter().flatten() {
                vertices.extend_from_slice(&value.to_le_bytes());
            }
            if let Some(before) = previous {
                for rect in 0..7 {
                    let at = before + rect;
                    indices.extend_from_slice(&[at, at + 9, at + 1, at + 9, at + 10, at + 1]);
                }
            }
            previous = Some(first);
        }
    }
    (vertices, indices)
}

/// `WaterManager::RecomputeDistanceHeightmap`: per vertex, the distance
/// along its row or its column to the nearest vertex at or above the
/// water, up to `MAX_SHORE_DISTANCE`. Ahead, the map's edge counts as
/// such a vertex, as `ComputeDirection`'s lookahead stops there.
fn distance_heightmap(map: &Heightmap, level: f32) -> Vec<f32> {
    let side = map.size as usize + 1;
    let mut distance = vec![MAX_SHORE_DISTANCE as f32; side * side];
    for transpose in [false, true] {
        for id1 in 0..side {
            let at = |id2: usize| if transpose { (id1, id2) } else { (id2, id1) };
            let above = |id2: usize| {
                let (x, z) = at(id2);
                map.vertex(x as isize, z as isize) >= level
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
pub fn waves(map: &Heightmap, level: f32, waviness: f32, kind: &str) -> Vec<Wave> {
    let mut out = Vec::new();
    if waviness < 5.0 && kind != "ocean" {
        return out;
    }
    let side = map.size as usize + 1;
    let distance = distance_heightmap(map, level);

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
    let ground_at = |at: [f32; 2]| map.ground(at[0], at[1]);
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
            outmost = -2.5 + outmost * waviness / 10.0;
            average_depth /= width as f32;
            if average_depth > -1.3 {
                j += 4;
                continue;
            }

            let wave_offset = time_offset;
            time_offset += (random.next() % 100) as f32 / 25.0 + 4.0;
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
            out.push(Wave {
                columns: columns.into_iter().map(Some).collect(),
            });
            j += width / 2;
        }
    }
    out
}

/// `WaterManager::RecomputeWindStrength`: wind marched across the map
/// from the edges it blows in from, slowed by rising ground and sped up
/// over falling ground, drifting back to 1 in between.
pub fn wind_strength(map: &Heightmap, level: f32, angle: f32) -> Vec<f32> {
    let side = map.size as usize + 1;
    let mut strength = vec![0.0; side * side];
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
        let depth = level - map.vertex(x, y);
        let mut wind = if depth > 0.0 && depth < 2.0 {
            depth / 2.0
        } else {
            1.0
        };
        strength[at(x, y)] = wind;
        'march: loop {
            for &(step_x, step_y) in &movement {
                wind = 0.15 + wind * 0.85;
                let height_diff =
                    map.vertex(x + step_x, y + step_y).max(level) - map.vertex(x, y).max(level);
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
