//! The lobby's thumbnail of a running match (M41, D61): the minimap's
//! terrain, territory and dots drawn on the CPU, because the server has no
//! GPU, from a snapshot the hub released after the viewers' delay.

use crate::feed::Terrain;
use crate::terrain;
use crate::view::Thumb;

const SIZE: usize = 256;
const QUALITY: f32 = 75.0;
/// The same colours as the client's minimap terrain picture.
const SHALLOW_WATER: [f32; 3] = [112.0, 152.0, 192.0];
const DEEP_WATER: [f32; 3] = [80.0, 120.0, 160.0];
const HEIGHT_UNITS_PER_METRE: f32 = 92.0;
/// Raw 16.16 fixed point.
const FIXED_ONE: f32 = 65536.0;
const OWNER_MASK: u8 = 0x1F;
const TERRITORY_FILL: f32 = 0.30;
const TERRITORY_BORDER: f32 = 0.60;
const MISSING_TILE: u16 = 0xFFFF;
/// Metres per tile, which the heightmap's slope is measured over.
const TILE_METRES: f32 = 4.0;
/// Toward the north-west and above, so ridges read as raised.
const LIGHT: [f32; 3] = [-0.5, 0.7, 0.5];

/// WebP bytes, or None when the terrain is empty or the encoder fails.
pub fn render(thumb: &Thumb, terrain: &Terrain) -> Option<Vec<u8>> {
    let tiles = terrain.tiles_per_side as usize;
    if tiles == 0 || terrain.tiles.len() < tiles * tiles {
        return None;
    }
    let vertices = tiles + 1;
    if terrain.heights.len() < vertices * vertices {
        return None;
    }
    let palette: Vec<u32> = terrain
        .textures
        .iter()
        .map(|name| terrain::color(name).0)
        .collect();
    let height = |i: usize, j: usize| {
        f32::from(terrain.heights[j.min(tiles) * vertices + i.min(tiles)]) / HEIGHT_UNITS_PER_METRE
    };
    let water = terrain.water_level as f32 / FIXED_ONE;
    let length = (LIGHT[0] * LIGHT[0] + LIGHT[1] * LIGHT[1] + LIGHT[2] * LIGHT[2]).sqrt();

    let mut rgb = vec![0u8; SIZE * SIZE * 3];
    for py in 0..SIZE {
        // The map's +z is up on the minimap.
        let v = 1.0 - (py as f32 + 0.5) / SIZE as f32;
        let j = ((v * tiles as f32) as usize).min(tiles - 1);
        for px in 0..SIZE {
            let u = (px as f32 + 0.5) / SIZE as f32;
            let i = ((u * tiles as f32) as usize).min(tiles - 1);
            let average =
                (height(i, j) + height(i + 1, j) + height(i, j + 1) + height(i + 1, j + 1)) / 4.0;
            let unknown = thumb
                .known
                .as_ref()
                .is_some_and(|known| !known.get(j * tiles + i).copied().unwrap_or(false));
            let mut color = if unknown {
                [0.0; 3]
            } else if average < water {
                if average > water - terrain.shallow_depth {
                    SHALLOW_WATER
                } else {
                    DEEP_WATER
                }
            } else {
                let index = terrain.tiles[j * tiles + i];
                let base = if index == MISSING_TILE {
                    0xFF_FF_FF
                } else {
                    palette
                        .get(usize::from(index))
                        .copied()
                        .unwrap_or(0xFF_FF_FF)
                };
                let slope_x =
                    (height(i + 1, j) - height(i.saturating_sub(1), j)) / (2.0 * TILE_METRES);
                let slope_z =
                    (height(i, j + 1) - height(i, j.saturating_sub(1))) / (2.0 * TILE_METRES);
                let normal_length = (slope_x * slope_x + 1.0 + slope_z * slope_z).sqrt();
                let lit =
                    (-slope_x * LIGHT[0] + LIGHT[1] - slope_z * LIGHT[2]) / normal_length / length;
                let shade = (0.75 + 0.5 * lit).clamp(0.6, 1.25);
                [
                    ((base >> 16) & 0xFF) as f32 * shade,
                    ((base >> 8) & 0xFF) as f32 * shade,
                    (base & 0xFF) as f32 * shade,
                ]
            };
            if !unknown
                && let Some(territory) = &thumb.territory
                && territory.width > 0
                && territory.height > 0
            {
                let (w, h) = (territory.width as usize, territory.height as usize);
                let cx = ((u * w as f32) as usize).min(w - 1);
                let cy = ((v * h as f32) as usize).min(h - 1);
                let owner_at = |x: usize, y: usize| territory.tiles[y * w + x] & OWNER_MASK;
                let owner = owner_at(cx, cy);
                if owner != 0 {
                    let edge = cx == 0
                        || cy == 0
                        || cx + 1 == w
                        || cy + 1 == h
                        || owner_at(cx - 1, cy) != owner
                        || owner_at(cx + 1, cy) != owner
                        || owner_at(cx, cy - 1) != owner
                        || owner_at(cx, cy + 1) != owner;
                    let weight = if edge {
                        TERRITORY_BORDER
                    } else {
                        TERRITORY_FILL
                    };
                    let tint = thumb.colors.get(usize::from(owner)).copied().unwrap_or(0);
                    mix(&mut color, tint, weight);
                }
            }
            let at = (py * SIZE + px) * 3;
            for (channel, value) in color.iter().enumerate() {
                rgb[at + channel] = value.clamp(0.0, 255.0) as u8;
            }
        }
    }

    if thumb.map_size > 0 {
        for dot in &thumb.dots {
            // Positions are in 1/16 m.
            let u = dot.x as f32 / 16.0 / thumb.map_size as f32;
            let v = dot.z as f32 / 16.0 / thumb.map_size as f32;
            if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                continue;
            }
            let cx = (u * SIZE as f32) as i32;
            let cy = ((1.0 - v) * SIZE as f32) as i32;
            // A unit is 2x2 pixels and a structure 3x3, so both show at 256.
            let (before, after) = if dot.structure { (1, 1) } else { (0, 1) };
            for y in (cy - before).max(0)..=(cy + after).min(SIZE as i32 - 1) {
                for x in (cx - before).max(0)..=(cx + after).min(SIZE as i32 - 1) {
                    let at = (y as usize * SIZE + x as usize) * 3;
                    rgb[at] = (dot.color >> 16) as u8;
                    rgb[at + 1] = (dot.color >> 8) as u8;
                    rgb[at + 2] = dot.color as u8;
                }
            }
        }
    }

    let encoded = webp::Encoder::from_rgb(&rgb, SIZE as u32, SIZE as u32).encode(QUALITY);
    Some(encoded.to_vec())
}

fn mix(color: &mut [f32; 3], tint: u32, weight: f32) {
    let tint = [
        ((tint >> 16) & 0xFF) as f32,
        ((tint >> 8) & 0xFF) as f32,
        (tint & 0xFF) as f32,
    ];
    for (value, tint) in color.iter_mut().zip(tint) {
        *value += (tint - *value) * weight;
    }
}
