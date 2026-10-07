//! The terrain textures, materials and planar mappings, as
//! `CTerrainTextureManager` loads them, and its composite of the blend
//! masks.

use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::Path;

use specula_assets::files::Texture;
use specula_assets::files::TextureFormat;

use crate::material;
use crate::material::Material;

const TERRAIN_DIR: &str = "art/terrains";
const ALPHA_MAP_DIR: &str = "art/textures/terrain/alphamaps/standard";
/// `CTerrainTextureManager::LoadAlphaMap`'s order, which
/// `CAlphaMapCalculator` indexes.
const ALPHA_MAPS: [&str; 14] = [
    "blendcircle.png",
    "blendlshape.png",
    "blendedge.png",
    "blendedgecorner.png",
    "blendedgetwocorners.png",
    "blendfourcorners.png",
    "blendtwooppositecorners.png",
    "blendlshapecorner.png",
    "blendtwocorners.png",
    "blendcorner.png",
    "blendtwoedges.png",
    "blendthreecorners.png",
    "blendushape.png",
    "blendbad.png",
];
/// Each mask's border, which keeps bilinear filtering off its neighbours.
const BORDER: usize = 2;

pub struct TerrainDef {
    /// The file stem, which maps name terrains by.
    pub tag: String,
    /// Under the mod, `art/textures/terrain/...`.
    pub base_texture: String,
    pub normal_texture: Option<String>,
    pub specular_texture: Option<String>,
    pub material: Material,
    pub angle: f32,
    pub size: f32,
}

/// `CTerrainProperties`' defaults. A new properties object takes only the
/// groups from its parent, so every `terrains.xml` and `<props>` starts
/// again from these.
#[derive(Clone, Copy)]
struct Mapping {
    angle: f32,
    size: f32,
}

impl Default for Mapping {
    fn default() -> Self {
        Mapping {
            angle: std::f32::consts::FRAC_PI_4,
            size: 32.0,
        }
    }
}

impl Mapping {
    fn with(node: roxmltree::Node) -> Self {
        let mut mapping = Mapping::default();
        if let Some(angle) = node
            .attribute("angle")
            .and_then(|value| value.parse::<f32>().ok())
        {
            mapping.angle = angle.to_radians();
        }
        if let Some(size) = node.attribute("size").and_then(|value| value.parse().ok()) {
            mapping.size = size;
        }
        mapping
    }
}

/// Every terrain under art/terrains, the first of a tag winning, as
/// `terrain-colors` keeps them. Also returns those that could not be read.
pub fn load(mod_dir: &Path) -> (Vec<TerrainDef>, Vec<String>) {
    let mut terrains = Vec::new();
    let mut failed = Vec::new();
    walk(
        mod_dir,
        &mod_dir.join(TERRAIN_DIR),
        Mapping::default(),
        &mut terrains,
        &mut failed,
    );
    let mut seen = HashSet::new();
    terrains.retain(|terrain: &TerrainDef| seen.insert(terrain.tag.clone()));
    (terrains, failed)
}

fn walk(
    mod_dir: &Path,
    dir: &Path,
    inherited: Mapping,
    out: &mut Vec<TerrainDef>,
    failed: &mut Vec<String>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        failed.push(format!("{}: cannot list", dir.display()));
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    // AddTextureDirCallback: a directory without terrains.xml keeps the
    // properties it was handed.
    let settings = dir.join("terrains.xml");
    let mapping = if settings.is_file() {
        match directory_mapping(&settings) {
            Ok(mapping) => mapping,
            Err(error) => {
                failed.push(format!("{}: {error}", settings.display()));
                inherited
            }
        }
    } else {
        inherited
    };
    for path in &paths {
        if path.is_dir() {
            walk(mod_dir, path, mapping, out, failed);
        } else if path.extension().is_some_and(|extension| extension == "xml")
            && path.file_stem().is_some_and(|stem| stem != "terrains")
        {
            match terrain(mod_dir, path, mapping) {
                Ok(terrain) => out.push(terrain),
                Err(error) => failed.push(format!("{}: {error}", path.display())),
            }
        }
    }
}

fn directory_mapping(path: &Path) -> Result<Mapping, String> {
    let xml = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let document = parse(&xml)?;
    Ok(document
        .root_element()
        .children()
        .find(|node| node.has_tag_name("Terrain"))
        .map(Mapping::with)
        .unwrap_or_default())
}

fn terrain(mod_dir: &Path, path: &Path, directory: Mapping) -> Result<TerrainDef, String> {
    let xml = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let document = parse(&xml)?;
    let root = document.root_element();
    let texture = |name: &str| {
        root.descendants()
            .find(|node| node.has_tag_name("texture") && node.attribute("name") == Some(name))
            .and_then(|node| node.attribute("file"))
            .map(|file| format!("art/textures/terrain/{file}"))
    };
    let base_texture = texture("baseTex").ok_or("no baseTex")?;
    // Without a <material> the entry keeps an empty one, which names no
    // effect.
    let material = root
        .children()
        .find(|node| node.has_tag_name("material"))
        .and_then(|node| node.text())
        .map(|name| material::load(mod_dir, name.trim()))
        .unwrap_or_default();
    let mapping = root
        .children()
        .find(|node| node.has_tag_name("props"))
        .map_or(directory, Mapping::with);
    Ok(TerrainDef {
        tag: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        base_texture,
        normal_texture: texture("normTex"),
        specular_texture: texture("specTex"),
        material,
        angle: mapping.angle,
        size: mapping.size,
    })
}

/// Some terrains.xml name the engine's DTD, which Xeromyces ignores.
fn parse(xml: &str) -> Result<roxmltree::Document<'_>, String> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    roxmltree::Document::parse_with_options(xml, options).map_err(|error| error.to_string())
}

/// `CTerrainTextureManager::LoadAlphaMap`'s composite: the masks side by
/// side, each with its edge pixels repeated `BORDER` times, the width
/// rounded up to a power of two. The mask is in alpha.
pub fn alpha_maps(mod_dir: &Path) -> Result<Texture, String> {
    let mut masks = Vec::new();
    let mut base = 0;
    for name in ALPHA_MAPS {
        let path = mod_dir.join(ALPHA_MAP_DIR).join(name);
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let (width, height, values) =
            first_channel(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
        if width != height || (base != 0 && width != base) {
            return Err(format!("{}: alpha maps differ in size", path.display()));
        }
        base = width;
        masks.push(values);
    }
    let tile_width = BORDER + base + BORDER;
    let total_width = (tile_width * ALPHA_MAPS.len()).next_power_of_two();
    let mut pixels = vec![0u8; total_width * base * 4];
    for (i, mask) in masks.iter().enumerate() {
        for row in 0..base {
            let source = &mask[row * base..(row + 1) * base];
            let at = row * total_width + i * tile_width;
            for x in 0..tile_width {
                let value = source[x.saturating_sub(BORDER).min(base - 1)];
                let pixel = (at + x) * 4;
                pixels[pixel..pixel + 4].copy_from_slice(&[0xFF, 0xFF, 0xFF, value]);
            }
        }
    }
    Ok(Texture {
        format: TextureFormat::Rgba8,
        width: total_width as u32,
        height: base as u32,
        levels: vec![pixels],
        details: [None; 3],
    })
}

/// The engine copies the first byte of each pixel.
fn first_channel(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let size = reader.output_buffer_size().ok_or("image too large")?;
    let mut buffer = vec![0; size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| error.to_string())?;
    let channels = info.color_type.samples();
    let (width, height) = (info.width as usize, info.height as usize);
    let values = buffer[..info.buffer_size()]
        .chunks_exact(info.line_size)
        .flat_map(|line| line.chunks_exact(channels).map(|pixel| pixel[0]))
        .collect();
    Ok((width, height, values))
}
