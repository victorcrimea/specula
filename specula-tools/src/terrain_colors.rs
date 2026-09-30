//! Builds the terrain texture -> minimap colour table that specula-server
//! embeds. The observer runs nonvisual and never loads textures, so the
//! colours the in-game minimap would use are computed here, once, from the
//! game data.
//!
//! Usage: terrain-colors [mod dir] [output]
//! Defaults: ../0ad/binaries/data/mods/public and
//! specula-server/data/terrain-colors.txt.

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

const DEFAULT_MOD: &str = "../0ad/binaries/data/mods/public";
const DEFAULT_OUTPUT: &str = "specula-server/data/terrain-colors.txt";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mod_dir = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_MOD.into()));
    let output = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_OUTPUT.into()));

    let mut definitions = Vec::new();
    collect_xml(&mod_dir.join("art/terrains"), &mut definitions);
    // The engine takes the first terrain with a tag; sorting makes "first"
    // at least stable here.
    definitions.sort();

    let mut colors = BTreeMap::new();
    let mut skipped = 0;
    for definition in &definitions {
        // terrains.xml holds shared properties, not a terrain.
        if definition
            .file_name()
            .is_some_and(|name| name == "terrains.xml")
        {
            continue;
        }
        let tag = definition
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if colors.contains_key(&tag) {
            eprintln!("duplicate tag {tag}: {} ignored", definition.display());
            continue;
        }
        match terrain_color(&mod_dir, definition) {
            Ok(color) => {
                colors.insert(tag, color);
            }
            Err(error) => {
                eprintln!("skipped {}: {error}", definition.display());
                skipped += 1;
            }
        }
    }

    let mut table = String::new();
    for (tag, [r, g, b]) in &colors {
        table.push_str(&format!("{tag} {r:02x}{g:02x}{b:02x}\n"));
    }
    if let Err(error) = fs::write(&output, table) {
        eprintln!("cannot write {}: {error}", output.display());
        return ExitCode::FAILURE;
    }
    println!(
        "{} colours written to {}, {skipped} skipped",
        colors.len(),
        output.display()
    );
    ExitCode::SUCCESS
}

fn collect_xml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_xml(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "xml") {
            out.push(path);
        }
    }
}

fn terrain_color(mod_dir: &Path, definition: &Path) -> Result<[u8; 3], String> {
    let xml = fs::read_to_string(definition).map_err(|error| error.to_string())?;
    let document = roxmltree::Document::parse(&xml).map_err(|error| error.to_string())?;
    let file = document
        .descendants()
        .find(|node| node.has_tag_name("texture") && node.attribute("name") == Some("baseTex"))
        .and_then(|node| node.attribute("file"))
        .ok_or("no baseTex")?;
    let path = mod_dir.join("art/textures/terrain").join(file);
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    match extension.as_ref() {
        "png" => png_mean(&bytes),
        "dds" => dds_mean(&bytes),
        _ => Err(format!("unsupported texture {file}")),
    }
}

/// The engine's base colour is the 1x1 mip of the converted texture. Any
/// normalised mip filter keeps the image mean, so the mean is that colour to
/// within rounding.
fn mean(sum: [u64; 3], count: u64) -> Result<[u8; 3], String> {
    if count == 0 {
        return Err("empty image".into());
    }
    Ok(sum.map(|channel| ((channel + count / 2) / count) as u8))
}

fn png_mean(bytes: &[u8]) -> Result<[u8; 3], String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let size = reader.output_buffer_size().ok_or("image too large")?;
    let mut pixels = vec![0; size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|error| error.to_string())?;
    let pixels = &pixels[..info.buffer_size()];
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("palette left unexpanded".into()),
    };
    let mut sum = [0u64; 3];
    for pixel in pixels.chunks_exact(channels) {
        let rgb = if channels < 3 {
            [pixel[0]; 3]
        } else {
            [pixel[0], pixel[1], pixel[2]]
        };
        for (total, value) in sum.iter_mut().zip(rgb) {
            *total += u64::from(value);
        }
    }
    mean(sum, (pixels.len() / channels) as u64)
}

/// Only the top level is read: the engine regenerates the mip chain when it
/// converts the texture, whatever the file carries.
fn dds_mean(bytes: &[u8]) -> Result<[u8; 3], String> {
    if bytes.len() < 128 || &bytes[..4] != b"DDS " {
        return Err("not a DDS file".into());
    }
    let u32_at = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let height = u32_at(12) as usize;
    let width = u32_at(16) as usize;
    let (block_size, dxt1) = match &bytes[84..88] {
        b"DXT1" => (8, true),
        b"DXT3" | b"DXT5" => (16, false),
        other => return Err(format!("unsupported DDS format {other:?}")),
    };
    let blocks = width.div_ceil(4) * height.div_ceil(4);
    let data = bytes
        .get(128..128 + blocks * block_size)
        .ok_or("DDS data truncated")?;

    let mut sum = [0u64; 3];
    for block in data.chunks_exact(block_size) {
        // DXT3 and DXT5 put their alpha first; the colour block is last.
        let color = &block[block_size - 8..];
        let c0 = u16::from_le_bytes([color[0], color[1]]);
        let c1 = u16::from_le_bytes([color[2], color[3]]);
        let palette = block_palette(c0, c1, dxt1);
        let indices = u32::from_le_bytes([color[4], color[5], color[6], color[7]]);
        for texel in 0..16 {
            let rgb = palette[(indices >> (texel * 2)) as usize & 3];
            for (total, value) in sum.iter_mut().zip(rgb) {
                *total += u64::from(value);
            }
        }
    }
    // Textures narrower than a block are padded; the padding counts, which
    // only matters for the two 4x4 files and not at all for their mean.
    mean(sum, (blocks * 16) as u64)
}

fn block_palette(c0: u16, c1: u16, dxt1: bool) -> [[u8; 3]; 4] {
    let a = expand_565(c0);
    let b = expand_565(c1);
    let mix = |wa: u16, wb: u16| {
        let total = wa + wb;
        [0, 1, 2].map(|i| ((u16::from(a[i]) * wa + u16::from(b[i]) * wb) / total) as u8)
    };
    // Only DXT1 has the 3-colour mode, whose fourth entry is transparent
    // black.
    if dxt1 && c0 <= c1 {
        [a, b, mix(1, 1), [0, 0, 0]]
    } else {
        [a, b, mix(2, 1), mix(1, 2)]
    }
}

fn expand_565(color: u16) -> [u8; 3] {
    let r = (color >> 11) & 0x1F;
    let g = (color >> 5) & 0x3F;
    let b = color & 0x1F;
    [
        ((r << 3) | (r >> 2)) as u8,
        ((g << 2) | (g >> 4)) as u8,
        ((b << 3) | (b >> 2)) as u8,
    ]
}
