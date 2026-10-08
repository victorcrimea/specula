//! Decoder for specula-server's terrain resource, format v3 (layout in
//! `specula-server/src/terrain.rs`), and the minimap-style picture made
//! from it.

pub const VERSION: u8 = 3;
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
    pub light: Light,
    /// The terrain names the tiles index.
    pub names: Vec<String>,
    /// size^2 indices into `names`, row-major, x fastest; 0xFFFF for
    /// none.
    pub tiles: Vec<u16>,
    /// size^2 blend priorities, likewise.
    pub priorities: Vec<u32>,
}

/// The map's `CLightEnv`, and its post effect.
#[derive(Clone, Copy)]
pub struct Light {
    /// Unit vector towards the sun.
    pub sun: [f32; 3],
    pub sun_color: [f32; 3],
    pub ambient_color: [f32; 3],
    pub fog_color: [f32; 3],
    pub fog_factor: f32,
    pub fog_max: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub bloom: f32,
    pub effect: PostEffect,
}

/// The map's post effect, `shaders/effects/postproc/<name>.xml`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PostEffect {
    Default,
    Hdr,
    Dof,
}

impl Light {
    /// `CLightEnv`'s defaults, with the given sun.
    pub fn plain(sun: [f32; 3], sun_color: [f32; 3], ambient_color: [f32; 3]) -> Self {
        Light {
            sun,
            sun_color,
            ambient_color,
            fog_color: [
                0xCC as f32 / 255.0,
                0xCC as f32 / 255.0,
                0xE5 as f32 / 255.0,
            ],
            fog_factor: 0.0,
            fog_max: 0.5,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 0.99,
            bloom: 0.1999,
            effect: PostEffect::Default,
        }
    }
}

/// `CLightEnv::CalculateSunDirection`, negated: the engine's points from
/// the sun.
fn sun_direction(elevation: f32, rotation: f32) -> [f32; 3] {
    let y = -elevation.sin();
    let scale = 1.0 + y;
    let dir = [scale * rotation.sin(), y, scale * rotation.cos()];
    let length = dir.iter().map(|v| v * v).sum::<f32>().sqrt();
    dir.map(|v| -v / length)
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

    /// The lowest and highest vertex, in metres.
    pub fn height_range(&self) -> (f32, f32) {
        let low = self.heights.iter().min().copied().unwrap_or(0);
        let high = self.heights.iter().max().copied().unwrap_or(0);
        (
            f32::from(low) / HEIGHT_UNITS_PER_METRE,
            f32::from(high) / HEIGHT_UNITS_PER_METRE,
        )
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

    /// `CTerrain::CalcExactNormal`: the normal of the ground triangle under
    /// `x`, `z`.
    pub fn normal(&self, x: f32, z: f32) -> [f32; 3] {
        if self.size < 2 {
            return [0.0, 1.0, 0.0];
        }
        let last = self.size as isize - 2;
        let xi = ((x / TILE_SIZE).floor() as isize).clamp(0, last) as usize;
        let zi = ((z / TILE_SIZE).floor() as isize).clamp(0, last) as usize;
        let xf = (x / TILE_SIZE - xi as f32).clamp(0.0, 1.0);
        let zf = (z / TILE_SIZE - zi as f32).clamp(0.0, 1.0);
        let h00 = self.height(xi, zi);
        let h01 = self.height(xi, zi + 1);
        let h10 = self.height(xi + 1, zi);
        let h11 = self.height(xi + 1, zi + 1);
        // The triangle's rises along x and along z, then the negated cross
        // product of (TILE_SIZE, dx, 0) and (0, dz, TILE_SIZE).
        let (dx, dz) = if self.triangulation_dir(xi, zi) {
            if xf + zf <= 1.0 {
                (h10 - h00, h01 - h00)
            } else {
                (h11 - h01, h11 - h10)
            }
        } else if xf <= zf {
            (h11 - h01, h01 - h00)
        } else {
            (h10 - h00, h11 - h10)
        };
        let (dx, dz) = (dx / HEIGHT_UNITS_PER_METRE, dz / HEIGHT_UNITS_PER_METRE);
        let normal = [-dx * TILE_SIZE, TILE_SIZE * TILE_SIZE, -dz * TILE_SIZE];
        let length = normal.iter().map(|v| v * v).sum::<f32>().sqrt();
        normal.map(|v| v / length)
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
    let mut floats = [0.0; 17];
    for value in &mut floats {
        *value = f32::from_bits(cursor.u32()?);
    }
    let [
        elevation,
        rotation,
        sun_r,
        sun_g,
        sun_b,
        ambient_r,
        ambient_g,
        ambient_b,
        fog_r,
        fog_g,
        fog_b,
        fog_factor,
        fog_max,
        brightness,
        contrast,
        saturation,
        bloom,
    ] = floats;
    let light = Light {
        sun: sun_direction(elevation, rotation),
        sun_color: [sun_r, sun_g, sun_b],
        ambient_color: [ambient_r, ambient_g, ambient_b],
        fog_color: [fog_r, fog_g, fog_b],
        fog_factor,
        fog_max,
        brightness,
        contrast,
        saturation,
        bloom,
        effect: match cursor.u8()? {
            1 => PostEffect::Hdr,
            2 => PostEffect::Dof,
            _ => PostEffect::Default,
        },
    };
    let palette_size = cursor.u32()? as usize;
    let mut palette = Vec::with_capacity(palette_size);
    let mut names = Vec::with_capacity(palette_size);
    for _ in 0..palette_size {
        palette.push(cursor.u32()?);
        let length = cursor.u16()? as usize;
        let name =
            std::str::from_utf8(cursor.take(length)?).map_err(|_| "terrain name not UTF-8")?;
        names.push(name.to_owned());
    }
    let vertices = tiles + 1;
    let heights = cursor.take(vertices * vertices * 2)?;
    let tile_textures = cursor.take(tiles * tiles * 2)?;
    let priorities = cursor.take(tiles * tiles * 4)?;
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
        if index == NO_TEXTURE {
            return UNKNOWN_COLOR;
        }
        palette
            .get(usize::from(index))
            .copied()
            .unwrap_or(UNKNOWN_COLOR)
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
        light,
        names,
        tiles: tile_textures
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect(),
        priorities: priorities
            .chunks_exact(4)
            .map(|quad| u32::from_le_bytes(quad.try_into().unwrap()))
            .collect(),
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

    fn u16(&mut self) -> Result<u16, &'static str> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}
