//! The map environment's textures: `WaterManager`'s normal map frames and
//! shore wave textures, and `SkyManager`'s cube maps. Both managers look
//! them up in the VFS, where the public mod overrides the engine's own
//! `mod`, which holds the ocean frames, the shore textures and the default
//! sky.

use std::fs;
use std::path::PathBuf;

use specula_assets::bc;
use specula_assets::files::Texture;
use specula_assets::files::TextureFormat;

use crate::texture;

const WATER_DIR: &str = "art/textures/animated/water";
/// `WaterManager::ReloadWaterNormalTextures` loads exactly this many.
const WATER_FRAMES: usize = 60;
pub const FOAM: &str = "art/textures/terrain/types/water/foam.png";
pub const COASTAL_WAVE: &str = "art/textures/terrain/types/water/coastalWave.png";
const SKY_DIR: &str = "art/textures/skies";
/// `SkyManager::LoadAndUploadSkyTexturesIfNeeded`'s images, one per cube
/// layer (+X, -X, +Y, -Y, +Z, -Z).
const SKY_FACES: [&str; 6] = ["front", "back", "top", "top", "right", "left"];

/// The first of `mods` that has `path`, as the VFS resolves it.
pub fn find(mods: &[PathBuf], path: &str) -> Option<PathBuf> {
    mods.iter()
        .map(|dir| dir.join(path))
        .find(|path| path.exists())
}

/// The names of the directories under `dir` in any of `mods`, sorted.
fn directories(mods: &[PathBuf], dir: &str) -> Vec<String> {
    let mut names: Vec<String> = mods
        .iter()
        .filter_map(|root| fs::read_dir(root.join(dir)).ok())
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Every water type with all of its frames, by name, each with the
/// frames' source paths in order. `usable` tells which sources the
/// converter can take; a type missing any frame is reported and left
/// out, as the engine would fail to load it.
pub fn water_sets(
    mods: &[PathBuf],
    usable: impl Fn(&str) -> Result<(), String>,
) -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let mut sets = Vec::new();
    let mut rejected = Vec::new();
    for name in directories(mods, WATER_DIR) {
        // `WaterManager.cpp`: "normal00%02d.png", counting from 1.
        let frames: Vec<String> = (1..=WATER_FRAMES)
            .map(|frame| format!("{WATER_DIR}/{name}/normal00{frame:02}.png"))
            .collect();
        if find(mods, &frames[0]).is_none() {
            // A directory of other textures (the simple water's diffuse
            // frames, sprites), not a water type.
            continue;
        }
        match frames.iter().try_for_each(|frame| usable(frame)) {
            Ok(()) => sets.push((name, frames)),
            Err(reason) => rejected.push(format!("water {name}: {reason}")),
        }
    }
    (sets, rejected)
}

/// Every sky set with all of its faces.
pub fn sky_sets(mods: &[PathBuf]) -> Vec<String> {
    directories(mods, SKY_DIR)
        .into_iter()
        .filter(|name| {
            SKY_FACES
                .iter()
                .all(|face| find(mods, &format!("{SKY_DIR}/{name}/{face}.dds")).is_some())
        })
        .collect()
}

/// The cube's layers as `SkyManager` uploads them: each face decoded to
/// RGBA8 without mips and turned bottom-up (`TEX_BOTTOM_UP`; DDS is read
/// top-down), the up and down layers, both the top image, turned a
/// quarter. The engine reads the DDS files as they are, not through the
/// converter.
pub fn sky_layers(mods: &[PathBuf], name: &str) -> Result<[Texture; 6], String> {
    let mut size = None;
    let mut layers = Vec::with_capacity(SKY_FACES.len());
    for (layer, face) in SKY_FACES.iter().enumerate() {
        let source = format!("{SKY_DIR}/{name}/{face}.dds");
        let path = find(mods, &source).ok_or_else(|| format!("{source}: missing"))?;
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let image = texture::convert(&bytes).map_err(|error| format!("{source}: {error}"))?;
        let (width, height) = (image.width, image.height);
        if width != height || size.is_some_and(|size| size != width) {
            return Err(format!("{source}: the faces are not squares of one size"));
        }
        size = Some(width);
        let pixels = match image.format.bc() {
            Some(format) => bc::decode(format, width, height, &image.levels[0])
                .map_err(|error| format!("{source}: {error}"))?,
            None => image.levels[0].clone(),
        };
        let row = width as usize * 4;
        let flipped: Vec<u8> = pixels.chunks_exact(row).rev().flatten().copied().collect();
        let pixels = if layer == 2 || layer == 3 {
            turn(&flipped, width as usize)
        } else {
            flipped
        };
        layers.push(Texture {
            format: TextureFormat::Rgba8,
            width,
            height,
            levels: vec![pixels],
            details: [None; 3],
        });
    }
    layers
        .try_into()
        .map_err(|_| format!("sky {name}: not six layers"))
}

/// `SkyManager`'s rotation of the up and down faces.
fn turn(pixels: &[u8], side: usize) -> Vec<u8> {
    let mut turned = vec![0; pixels.len()];
    for y in 0..side {
        for x in 0..side {
            let (from_x, from_y) = (y, side - x - 1);
            let to = (y * side + x) * 4;
            let from = (from_y * side + from_x) * 4;
            turned[to..to + 4].copy_from_slice(&pixels[from..from + 4]);
        }
    }
    turned
}
