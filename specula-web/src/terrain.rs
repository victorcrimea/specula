//! Decoder for specula-server's terrain resource, format v0 (layout in
//! `specula-server/src/terrain.rs`), and the minimap-style picture made
//! from it.

const VERSION: u8 = 0;
/// Engine heights are u16 in these units (HEIGHT_UNITS_PER_METRE).
const HEIGHT_UNITS_PER_METRE: f32 = 92.0;
const NO_TEXTURE: u16 = 0xFFFF;
/// The minimap's fallback for a tile without a texture.
const UNKNOWN_COLOR: u32 = 0xFF_FF_FF;
/// CMiniMapTexture draws water in fixed colours "for consistency on
/// different maps", as RGBA bytes.
const SHALLOW_WATER: [u8; 4] = [0x70, 0x98, 0xC0, 0xFF];
const DEEP_WATER: [u8; 4] = [0x50, 0x78, 0xA0, 0xFF];

#[derive(Clone)]
pub struct TerrainImage {
    /// Tiles per side; the image is size x size RGBA8, row 0 at z = 0.
    pub size: u32,
    pub circular: bool,
    pub pixels: Vec<u8>,
    /// (size + 1)^2 engine heights, row-major, x fastest, for the 3D view.
    pub heights: Vec<u16>,
    /// In metres.
    pub water_level: f32,
}

/// `TERRAIN_TILE_SIZE`, in metres.
pub const TILE_SIZE: f32 = 4.0;

impl TerrainImage {
    fn height(&self, i: usize, j: usize) -> f32 {
        f32::from(self.heights[j * (self.size as usize + 1) + i])
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

/// Follows CMiniMapTexture::RebuildTerrainTexture, so the background looks
/// like the in-game minimap.
pub fn build(resource: &[u8]) -> Result<TerrainImage, &'static str> {
    let mut cursor = Cursor(resource);
    if cursor.u8()? != VERSION {
        return Err("unknown terrain version");
    }
    let tiles = cursor.u32()? as usize;
    let water_level = f32::from_bits(cursor.u32()?);
    let shallow_depth = f32::from_bits(cursor.u32()?);
    let circular = cursor.u8()? != 0;
    let palette_size = cursor.u32()? as usize;
    let palette = cursor.take(palette_size * 4)?;
    let vertices = tiles + 1;
    let heights = cursor.take(vertices * vertices * 2)?;
    let tile_textures = cursor.take(tiles * tiles * 2)?;
    if !cursor.0.is_empty() {
        return Err("terrain resource has trailing bytes");
    }

    let height = |i: usize, j: usize| {
        let at = (j * vertices + i) * 2;
        u16::from_le_bytes([heights[at], heights[at + 1]])
    };
    let base_color = |tile: usize| {
        let at = tile * 2;
        let index = u16::from_le_bytes([tile_textures[at], tile_textures[at + 1]]);
        if index == NO_TEXTURE || usize::from(index) >= palette_size {
            return UNKNOWN_COLOR;
        }
        let at = usize::from(index) * 4;
        u32::from_le_bytes(palette[at..at + 4].try_into().unwrap())
    };

    let mut pixels = Vec::with_capacity(tiles * tiles * 4);
    for j in 0..tiles {
        for i in 0..tiles {
            let average = (f32::from(height(i, j))
                + f32::from(height(i + 1, j))
                + f32::from(height(i, j + 1))
                + f32::from(height(i + 1, j + 1)))
                / HEIGHT_UNITS_PER_METRE
                / 4.0;
            if average < water_level && average > water_level - shallow_depth {
                pixels.extend_from_slice(&SHALLOW_WATER);
            } else if average < water_level {
                pixels.extend_from_slice(&DEEP_WATER);
            } else {
                // Higher ground is lighter: 170..255 out of 255.
                let shade = u32::from(height(i, j) >> 8) / 3 + 170;
                let scale = shade as f32 / 255.0;
                let [b, g, r, _] = base_color(j * tiles + i).to_le_bytes();
                pixels.extend_from_slice(&[
                    (f32::from(r) * scale) as u8,
                    (f32::from(g) * scale) as u8,
                    (f32::from(b) * scale) as u8,
                    0xFF,
                ]);
            }
        }
    }
    Ok(TerrainImage {
        size: tiles as u32,
        circular,
        pixels,
        heights: heights
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect(),
        water_level,
    })
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], &'static str> {
        if self.0.len() < length {
            return Err("terrain resource truncated");
        }
        let (bytes, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, &'static str> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}
