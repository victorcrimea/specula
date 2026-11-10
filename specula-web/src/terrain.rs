//! Decoder for specula-server's terrain resource, format v5 and v4 (layout
//! in `specula-server/src/terrain.rs`), whole or as a seat has had it
//! revealed, and the minimap-style picture made from it.

use specula_assets::bytes::ByteReader;
use specula_assets::shore;
use specula_assets::shore::Heightmap;
use specula_assets::shore::WAVE_FLOATS;
use specula_assets::shore::WAVE_ROWS;
use specula_assets::shore::Wave;
use specula_assets::wire::HEIGHT_UNITS_PER_METRE;
use specula_assets::wire::TERRAIN_VERSION;

const NO_TEXTURE: u16 = 0xFFFF;
/// The minimap's fallback for a tile without a texture.
const UNKNOWN_COLOR: u32 = 0xFF_FF_FF;
/// CMiniMapTexture draws water in fixed colours "for consistency on
/// different maps", as RGBA bytes.
const SHALLOW_WATER: [u8; 4] = [0x70, 0x98, 0xC0, 0xFF];
const DEEP_WATER: [u8; 4] = [0x50, 0x78, 0xA0, 0xFF];
/// v4 had no kind, and was always whole.
const OLDEST_VERSION: u8 = 4;
const KIND_WHOLE: u8 = 0;
const KIND_PARTIAL: u8 = 1;

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
    pub water: Water,
    /// The `art/textures/skies` set; empty for none.
    pub sky: String,
    pub light: Light,
    /// The terrain names the tiles index.
    pub names: Vec<String>,
    /// size^2 indices into `names`, row-major, x fastest; 0xFFFF for
    /// none.
    pub tiles: Vec<u16>,
    /// size^2 blend priorities, likewise.
    pub priorities: Vec<u32>,
    /// Each name's minimap colour, 0xRRGGBB.
    pub palette: Vec<u32>,
    /// In metres.
    pub shallow_depth: f32,
    /// A seat's terrain: what of it has been revealed.
    pub seat: Option<SeatTerrain>,
}

/// What a seat holds of the terrain; the rest is unknown, height 0 and no
/// texture, which the LOS draws black.
#[derive(Clone)]
pub struct SeatTerrain {
    /// The reveals it holds, as the stream counts them.
    pub revision: u32,
    /// By vertex: known.
    pub known: Vec<bool>,
    /// What the server made of the whole heightmap, which the seat cannot:
    /// the wind strength at each known vertex, and the shore waves' columns
    /// over known tiles.
    pub wind: Vec<f32>,
    pub waves: Vec<Wave>,
}

/// The map's `WaterManager` settings.
#[derive(Clone)]
pub struct Water {
    pub color: [f32; 3],
    pub tint: [f32; 3],
    pub waviness: f32,
    pub murkiness: f32,
    /// Radians.
    pub wind_angle: f32,
    /// The `art/textures/animated/water` set of normal maps.
    pub kind: String,
}

/// The map's `CLightEnv`, and its post effect.
#[derive(Clone, Copy)]
pub struct Light {
    /// Unit vector towards the sun.
    pub sun: [f32; 3],
    /// `CLightEnv::m_Rotation`, radians, which also turns the sky.
    pub rotation: f32,
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
            rotation: 0.0,
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
        self.heightmap().triangulation_dir(i, j)
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

    /// `CTerrain::GetExactGroundLevel`, in metres, as the server's shore
    /// waves take it.
    pub fn ground(&self, x: f32, z: f32) -> f32 {
        self.heightmap().ground(x, z)
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
    let mut cursor = ByteReader(resource);
    let version = cursor.u8()?;
    if !(OLDEST_VERSION..=TERRAIN_VERSION).contains(&version) {
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
        rotation,
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
    let mut floats = [0.0; 9];
    for value in &mut floats {
        *value = f32::from_bits(cursor.u32()?);
    }
    let [
        r,
        g,
        b,
        tint_r,
        tint_g,
        tint_b,
        waviness,
        murkiness,
        wind_angle,
    ] = floats;
    let water = Water {
        color: [r, g, b],
        tint: [tint_r, tint_g, tint_b],
        waviness,
        murkiness,
        wind_angle,
        kind: cursor.string16()?,
    };
    let sky = cursor.string16()?;
    let kind = if version >= 5 {
        cursor.u8()?
    } else {
        KIND_WHOLE
    };
    let vertices = tiles + 1;
    let mut image = TerrainImage {
        size: tiles as u32,
        circular,
        pixels: Vec::new(),
        heights: vec![0; vertices * vertices],
        water_level,
        water,
        sky,
        light,
        names: Vec::new(),
        tiles: vec![NO_TEXTURE; tiles * tiles],
        priorities: vec![0; tiles * tiles],
        palette: Vec::new(),
        shallow_depth,
        seat: None,
    };
    match kind {
        KIND_WHOLE => {
            let palette_size = cursor.u32()? as usize;
            for _ in 0..palette_size {
                image.palette.push(cursor.u32()?);
                let length = cursor.u16()? as usize;
                let name = std::str::from_utf8(cursor.take(length)?)
                    .map_err(|_| "terrain name not UTF-8")?;
                image.names.push(name.to_owned());
            }
            image.heights = cursor
                .take(vertices * vertices * 2)?
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            image.tiles = cursor
                .take(tiles * tiles * 2)?
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            image.priorities = cursor
                .take(tiles * tiles * 4)?
                .chunks_exact(4)
                .map(|quad| u32::from_le_bytes(quad.try_into().unwrap()))
                .collect();
            if !cursor.0.is_empty() {
                return Err("terrain resource has trailing bytes");
            }
        }
        KIND_PARTIAL => {
            image.seat = Some(SeatTerrain {
                revision: cursor.u32()?,
                known: vec![false; vertices * vertices],
                wind: vec![0.0; vertices * vertices],
                waves: Vec::new(),
            });
            image.read_reveal(&mut cursor)?;
            if !cursor.0.is_empty() {
                return Err("terrain resource has trailing bytes");
            }
        }
        _ => return Err("unknown terrain kind"),
    }
    image.paint();
    Ok(image)
}

impl TerrainImage {
    /// Adds a reveal of a seat's stream (`terrain.rs`) to a seat's terrain;
    /// one it already holds changes nothing.
    pub fn apply_reveal(&mut self, revision: u32, reveal: &[u8]) -> Result<(), &'static str> {
        let mut cursor = ByteReader(reveal);
        self.read_reveal(&mut cursor)?;
        if !cursor.0.is_empty() {
            return Err("reveal has trailing bytes");
        }
        if let Some(seat) = &mut self.seat {
            seat.revision = seat.revision.max(revision);
        }
        self.paint();
        Ok(())
    }

    fn read_reveal(&mut self, cursor: &mut ByteReader) -> Result<(), &'static str> {
        let Some(seat) = &mut self.seat else {
            return Err("a reveal for a whole terrain");
        };
        for _ in 0..cursor.varint_u32()? {
            let index = cursor.varint()? as usize;
            let color = cursor.u32()?;
            let name = cursor.text()?;
            if index > usize::from(NO_TEXTURE) {
                return Err("palette index out of range");
            }
            if self.names.len() <= index {
                self.names.resize(index + 1, String::new());
                self.palette.resize(index + 1, UNKNOWN_COLOR);
            }
            self.names[index] = name;
            self.palette[index] = color;
        }
        let vertices = self.heights.len();
        let mut at = 0usize;
        for _ in 0..cursor.varint_u32()? {
            at += cursor.varint()? as usize;
            let count = cursor.varint()? as usize;
            if at + count > vertices {
                return Err("revealed vertices past the map");
            }
            for vertex in at..at + count {
                self.heights[vertex] = cursor.u16()?;
                seat.wind[vertex] = cursor.f32()?;
                seat.known[vertex] = true;
            }
            at += count;
        }
        let tiles = self.tiles.len();
        let mut at = 0usize;
        for _ in 0..cursor.varint_u32()? {
            at += cursor.varint()? as usize;
            let count = cursor.varint()? as usize;
            if at + count > tiles {
                return Err("revealed tiles past the map");
            }
            for tile in at..at + count {
                self.tiles[tile] = cursor.u16()?;
                self.priorities[tile] = cursor.u32()?;
            }
            at += count;
        }
        for _ in 0..cursor.varint_u32()? {
            let number = cursor.varint()? as usize;
            let width = cursor.varint()? as usize;
            if number > tiles || width > shore::WAVE_POINTS {
                return Err("wave out of range");
            }
            if seat.waves.len() <= number {
                seat.waves.resize(
                    number + 1,
                    Wave {
                        columns: Vec::new(),
                    },
                );
            }
            let wave = &mut seat.waves[number];
            wave.columns.resize(width, None);
            for _ in 0..cursor.varint_u32()? {
                let k = cursor.varint()? as usize;
                let mut column = [[0.0; WAVE_FLOATS]; WAVE_ROWS];
                for value in column.iter_mut().flatten() {
                    *value = cursor.f32()?;
                }
                *wave.columns.get_mut(k).ok_or("wave column out of range")? = Some(column);
            }
        }
        Ok(())
    }

    /// `CMiniMapTexture::RebuildTerrainTexture`, so the background looks
    /// like the in-game minimap.
    fn paint(&mut self) {
        let tiles = self.size as usize;
        let vertices = tiles + 1;
        let height = |i: usize, j: usize| self.heights[j * vertices + i];
        let base_color = |tile: usize| {
            let index = self.tiles[tile];
            if index == NO_TEXTURE {
                return UNKNOWN_COLOR;
            }
            self.palette
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
                if average < self.water_level && average > self.water_level - self.shallow_depth {
                    pixels.extend_from_slice(&SHALLOW_WATER);
                } else if average < self.water_level {
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
        self.pixels = pixels;
    }

    fn heightmap(&self) -> Heightmap<'_> {
        Heightmap {
            size: self.size,
            heights: &self.heights,
        }
    }

    /// `WaterManager::RecomputeWindStrength`'s, which a seat is sent.
    pub fn wind_strength(&self) -> Vec<f32> {
        match &self.seat {
            Some(seat) => seat.wind.clone(),
            None => {
                shore::wind_strength(&self.heightmap(), self.water_level, self.water.wind_angle)
            }
        }
    }

    /// `WaterManager::CreateWaveMeshes`', which a seat is sent.
    pub fn waves(&self) -> Vec<Wave> {
        match &self.seat {
            Some(seat) => seat.waves.clone(),
            None => shore::waves(
                &self.heightmap(),
                self.water_level,
                self.water.waviness,
                &self.water.kind,
            ),
        }
    }
}
