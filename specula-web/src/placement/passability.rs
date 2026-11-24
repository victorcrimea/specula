//! What the pathfinder's terrain-only grid says of a navcell, worked out
//! for one navcell at a time from the heightmap the seat holds, in the
//! engine's raw 16.16 integers (heights, slopes and the shore grid are
//! integers already, so these match the engine exactly).

use crate::stream::Ground;

const ONE: i64 = 1 << 16;
/// `MAP_EDGE_TILES` in navcells.
const EDGE_NAVCELLS: i64 = 3 * 4;
/// Avoids overflow in the relaxation, as the engine's `shoreMax`.
const SHORE_MAX: u32 = 32767;

/// A passability class of `pathfinder.xml`, each limit raw 16.16; a limit
/// the file does not give is the extreme.
#[derive(Clone, Copy)]
pub struct PassClass {
    min_depth: i64,
    max_depth: i64,
    max_slope: i64,
    min_shore: i64,
    max_shore: i64,
}

impl PassClass {
    /// `PathfinderPassability::IsPassable`.
    fn passable(&self, depth: i64, slope: i64, shore: i64) -> bool {
        self.min_depth <= depth
            && depth <= self.max_depth
            && slope < self.max_slope
            && self.min_shore <= shore
            && shore <= self.max_shore
    }
}

/// `building-land`: most buildings.
pub const BUILDING_LAND: PassClass = PassClass {
    min_depth: i64::MIN,
    max_depth: 0,
    max_slope: ONE,
    min_shore: 4 * ONE,
    max_shore: i64::MAX,
};

/// `building-shore`: docks.
pub const BUILDING_SHORE: PassClass = PassClass {
    min_depth: i64::MIN,
    max_depth: i64::MAX,
    max_slope: ONE + ONE / 4,
    min_shore: i64::MIN,
    max_shore: 8 * ONE,
};

/// `default-terrain-only`: walls' pieces on land and shallows.
pub const DEFAULT_TERRAIN_ONLY: PassClass = PassClass {
    min_depth: i64::MIN,
    max_depth: 2 * ONE,
    max_slope: ONE,
    min_shore: i64::MIN,
    max_shore: i64::MAX,
};

/// `CCmpPathfinder::ComputeShoreGrid`: per terrain tile, how many tiles it
/// is from the water, 0 on and under it.
pub struct ShoreGrid {
    tiles: usize,
    distance: Vec<u16>,
    /// The terrain it was computed for.
    key: (u64, u32),
}

impl ShoreGrid {
    pub fn key(&self) -> (u64, u32) {
        self.key
    }

    pub fn compute(ground: &Ground) -> Self {
        let tiles = ground.tiles() as usize;
        let water = ground.water_level;
        let mut wet = vec![false; tiles * tiles];
        for j in 0..tiles {
            for i in 0..tiles {
                // `TerrainTileCenter`.
                let x = (i as i64 * 4 + 2) * ONE;
                let z = (j as i64 * 4 + 2) * ONE;
                wet[j * tiles + i] = i64::from(water) > ground.at_fixed(x, z);
            }
        }
        let is_wet = |i: usize, j: usize| wet[j * tiles + i];
        let mut distance = vec![0u32; tiles * tiles];
        for j in 0..tiles {
            for i in 0..tiles {
                if is_wet(i, j) {
                    continue;
                }
                let bordered = (-1i32..=1)
                    .flat_map(|dj| (-1i32..=1).map(move |di| (di, dj)))
                    .filter(|&(di, dj)| (di, dj) != (0, 0))
                    .any(|(di, dj)| {
                        let (ni, nj) = (i as i32 + di, j as i32 + dj);
                        ni >= 0
                            && nj >= 0
                            && (ni as usize) < tiles
                            && (nj as usize) < tiles
                            && is_wet(ni as usize, nj as usize)
                    });
                distance[j * tiles + i] = if bordered { 0 } else { SHORE_MAX };
            }
        }
        // The running minimum is not reset between the forward and the
        // backward sweep, nor across water, as the engine's.
        for j in 0..tiles {
            let mut minimum = SHORE_MAX;
            for i in (0..tiles).chain((0..tiles).rev()) {
                if is_wet(i, j) {
                    continue;
                }
                let value = distance[j * tiles + i];
                if value > minimum {
                    distance[j * tiles + i] = minimum;
                } else if value < minimum {
                    minimum = value;
                }
                minimum += 1;
            }
        }
        for i in 0..tiles {
            let mut minimum = SHORE_MAX;
            for j in (0..tiles).chain((0..tiles).rev()) {
                if is_wet(i, j) {
                    continue;
                }
                let value = distance[j * tiles + i];
                if value > minimum {
                    distance[j * tiles + i] = minimum;
                } else if value < minimum {
                    minimum = value;
                }
                minimum += 1;
            }
        }
        ShoreGrid {
            tiles,
            distance: distance.into_iter().map(|value| value as u16).collect(),
            key: (ground.id, ground.revision),
        }
    }

    fn at(&self, i: usize, j: usize) -> u16 {
        self.distance[j.min(self.tiles - 1) * self.tiles + i.min(self.tiles - 1)]
    }
}

/// What `TerrainUpdateHelper` leaves of navcell `(i, j)` for `class`,
/// whether a building of that class may stand there, with the map edge's
/// closing as `ComputeOffWorld` makes it.
pub fn navcell_passable(
    ground: &Ground,
    shore: &ShoreGrid,
    class: &PassClass,
    i: i64,
    j: i64,
) -> bool {
    // `NavcellCenter`.
    let x = i * ONE + ONE / 2;
    let z = j * ONE + ONE / 2;
    let depth = i64::from(ground.water_level) - ground.at_fixed(x, z);
    let (tile_i, tile_j) = (i / 4, j / 4);
    let slope = ground.slope(tile_i, tile_j);
    let distance = i64::from(shore.at(tile_i as usize, tile_j as usize)) * 4 * ONE;
    if !class.passable(depth, slope, distance) {
        return false;
    }
    !off_world(ground, i, j)
}

/// The border the engine marks impassable for every class.
fn off_world(ground: &Ground, i: i64, j: i64) -> bool {
    let size = ground.tiles() * 4;
    let edge = EDGE_NAVCELLS;
    if ground.circular {
        // Doubled, so half-navcell centres stay integers.
        let dist2 = (i * 2 + 1 - size).pow(2) + (j * 2 + 1 - size).pow(2);
        return dist2 >= (size - 2 * edge) * (size - 2 * edge);
    }
    // The engine's loops stop one short of the right and bottom bands.
    let inner = edge..size - edge + 1;
    i < edge || i > size - edge || ((j < edge || j > size - edge) && inner.contains(&i))
}
