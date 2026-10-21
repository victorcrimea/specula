//! The textures and fonts the session GUI's panels are drawn with, for the
//! page's DOM panels: each file named by its content, so it is kept for
//! good, and `session.css`, which names them in custom properties
//! (`--gui-<key>`) and declares the engine's fonts.
//!
//! Usage: session-gui [mod dir] [output dir]
//! Defaults: ../0ad/binaries/data/mods/public and specula-web/gui. The
//! fonts come from the engine's own `mod` next to the mod dir, as the VFS
//! mounts it under the public mod.
//!
//! Most textures are copied as they are, since a browser decodes PNG. Only
//! what the engine changes when it draws, or what a browser cannot read,
//! is converted: civ emblems turned grey as `canvas2d.fs` does for a
//! `grayscale:` sprite, the DDS portrait background as PNG, and the
//! minimap's civ backgrounds, 512 px squares drawn in under 200 px, scaled
//! to twice that as lossy WebP.
//!
//! Old files are left in the output directory, so a page that still names
//! them keeps working until the next deploy clears it.

use std::fs;
use std::hash::DefaultHasher;
use std::hash::Hasher;
use std::io::Cursor;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use fast_image_resize::PixelType;
use fast_image_resize::Resizer;
use fast_image_resize::images::Image;

const DEFAULT_MOD: &str = "../0ad/binaries/data/mods/public";
const DEFAULT_OUTPUT: &str = "specula-web/gui";
const UI: &str = "art/textures/ui";
const PLAYERS: &str = "simulation/templates/special/players";
const CSS: &str = "session.css";
/// Twice the minimap's 192 px background, for high density screens.
const MINIMAP_BACKGROUND_SIDE: u32 = 384;
const WEBP_QUALITY: f32 = 85.0;
const BROTLI_QUALITY: u32 = 11;
const BROTLI_WINDOW_BITS: u32 = 22;

/// (custom property key, path under `UI`), copied as they are.
const TEXTURES: [(&str, &str); 51] = [
    ("hud-panels", "session/hud_panels.png"),
    ("line-horiz", "global/border/line_horiz.png"),
    ("line-vert", "global/border/line_vert.png"),
    (
        "line-corner-top-left",
        "global/border/line_corner_top_left.png",
    ),
    (
        "line-corner-top-right",
        "global/border/line_corner_top_right.png",
    ),
    (
        "line-corner-bottom-left",
        "global/border/line_corner_bottom_left.png",
    ),
    (
        "line-corner-bottom-right",
        "global/border/line_corner_bottom_right.png",
    ),
    (
        "line-corner-middle-left",
        "global/border/line_corner_middle_left.png",
    ),
    (
        "line-corner-middle-right",
        "global/border/line_corner_middle_right.png",
    ),
    (
        "line-corner-bottom-middle",
        "global/border/line_corner_bottom_middle.png",
    ),
    ("panel-shader", "session/panel_shader.png"),
    ("panel-shader-top-edge", "session/panel_shader_top_edge.png"),
    (
        "panel-shader-bottom-edge",
        "session/panel_shader_bottom_edge.png",
    ),
    (
        "bar-lighting-horizontal",
        "session/bar_lighting_horizontal.png",
    ),
    ("bar-lighting-vertical", "session/bar_lighting_vertical.png"),
    ("groups", "session/icons/groups.png"),
    ("resource-food", "session/icons/resources/food.png"),
    ("resource-wood", "session/icons/resources/wood.png"),
    ("resource-stone", "session/icons/resources/stone.png"),
    ("resource-metal", "session/icons/resources/metal.png"),
    (
        "resource-population",
        "session/icons/resources/population.png",
    ),
    ("ribbon-bg", "session/ribbon_bg.png"),
    ("repair", "session/icons/repair.png"),
    ("production", "session/icons/production.png"),
    ("stop", "session/icons/stop.png"),
    ("vertical-pair", "session/icons/vertical_pair.png"),
    ("rank-basic", "session/icons/ranks/Basic.png"),
    ("rank-advanced", "session/icons/ranks/Advanced.png"),
    ("rank-elite", "session/icons/ranks/Elite.png"),
    ("phase-village", "session/panel_phase_emblems_village.png"),
    ("phase-town", "session/panel_phase_emblems_town.png"),
    ("phase-city", "session/panel_phase_emblems_city.png"),
    ("phase-hidden", "session/panel_phase_emblems_hidden.png"),
    ("minimap-circle", "session/minimap_circle_modern.png"),
    ("stance-defensive", "session/icons/stances/defensive.png"),
    // The `<icon>`s of gui/common/resources/, which tooltips name as
    // `[icon="icon_<code>"]`.
    ("icon-food", "session/icons/resources/food_small.png"),
    ("icon-wood", "session/icons/resources/wood_small.png"),
    ("icon-stone", "session/icons/resources/stone_small.png"),
    ("icon-metal", "session/icons/resources/metal_small.png"),
    (
        "icon-population",
        "session/icons/resources/population_small.png",
    ),
    ("icon-time", "session/icons/resources/time_small.png"),
    ("icon-xp", "session/icons/promote.png"),
    ("icon-food_fish", "session/icons/resources/fish_small.png"),
    ("icon-food_fruit", "session/icons/resources/fruit_small.png"),
    ("icon-food_grain", "session/icons/resources/grain_small.png"),
    ("icon-food_meat", "session/icons/resources/meat_small.png"),
    ("icon-food_rice", "session/icons/resources/rice_small.png"),
    ("icon-metal_ore", "session/icons/resources/metal_small.png"),
    ("icon-stone_rock", "session/icons/resources/stone_small.png"),
    ("icon-wood_tree", "session/icons/resources/wood_small.png"),
    ("portrait-black", "session/icons/bkg/portrait_black.dds"),
];
/// The engine's `sans` (config/default.cfg, [fonts.sans]): (family, CSS
/// weight, file under fonts/). FreeSans is the engine's fallback for what
/// Biolinum lacks, which a browser does by the family list as well.
const FONTS: [(&str, u16, &str); 4] = [
    ("0ad sans", 400, "LinBiolinum_Rah.ttf"),
    ("0ad sans", 700, "LinBiolinum_RBah.ttf"),
    ("0ad sans fallback", 400, "FreeSans.ttf"),
    ("0ad sans fallback", 700, "FreeSansBold.ttf"),
];

struct Rgba {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mod_dir = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_MOD.into()));
    let output = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_OUTPUT.into()));
    match run(&mod_dir, &output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(mod_dir: &Path, output: &Path) -> Result<(), String> {
    fs::create_dir_all(output).map_err(|error| format!("{}: {error}", output.display()))?;
    let ui = mod_dir.join(UI);
    let mut properties = Vec::new();
    let mut written = 0usize;
    for (key, path) in TEXTURES {
        let source = ui.join(path);
        let bytes = read(&source)?;
        let bytes = if path.ends_with(".dds") {
            encode_png(&decode_dds(&bytes).map_err(|error| format!("{path}: {error}"))?)?
        } else {
            bytes
        };
        let name = put(output, key, "png", &bytes, false)?;
        written += bytes.len();
        properties.push((key.to_string(), name));
    }
    for (civ, icon) in civ_emblems(mod_dir)? {
        let path = format!("session/portraits/{icon}");
        let mut image =
            decode_png(&read(&ui.join(&path))?).map_err(|error| format!("{path}: {error}"))?;
        grayscale(&mut image);
        let bytes = encode_png(&image)?;
        let key = format!("emblem-{civ}");
        let name = put(output, &key, "png", &bytes, false)?;
        written += bytes.len();
        properties.push((key, name));
    }
    for (civ, path) in minimap_backgrounds(&ui)? {
        let image =
            decode_png(&read(&ui.join(&path))?).map_err(|error| format!("{path}: {error}"))?;
        let bytes = encode_webp(&resize(image, MINIMAP_BACKGROUND_SIDE)?)?;
        let key = format!("minimap-background-{civ}");
        let name = put(output, &key, "webp", &bytes, false)?;
        written += bytes.len();
        properties.push((key, name));
    }
    let fonts_dir = mod_dir
        .parent()
        .ok_or("the mod dir has no parent")?
        .join("mod/fonts");
    let mut faces = Vec::new();
    for (family, weight, file) in FONTS {
        let bytes = read(&fonts_dir.join(file))?;
        let stem = file
            .trim_end_matches(".ttf")
            .to_ascii_lowercase()
            .replace('_', "-");
        let name = put(output, &stem, "ttf", &bytes, true)?;
        written += bytes.len();
        faces.push((family, weight, name));
    }

    let mut css = String::from(
        "/* Written by specula-tools session-gui; textures of 0 A.D. by Wildfire\n   Games, CC-BY-SA 3.0, fonts under their own licences (gui.LICENSE). */\n",
    );
    for (family, weight, name) in &faces {
        css.push_str(&format!(
            "@font-face {{ font-family: \"{family}\"; font-weight: {weight}; font-display: swap;\n  src: url(\"/gui/{name}\") format(\"truetype\"); }}\n"
        ));
    }
    css.push_str(":root {\n");
    for (key, name) in &properties {
        css.push_str(&format!("  --gui-{key}: url(\"/gui/{name}\");\n"));
    }
    css.push_str("}\n");
    let css_path = output.join(CSS);
    write(&css_path, css.as_bytes())?;
    write_brotli(&css_path, css.as_bytes())?;
    println!(
        "{} textures and {} fonts, {written} bytes, named in {}",
        properties.len(),
        faces.len(),
        css_path.display()
    );
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.starts_with(b"version https://git-lfs") {
        return Err(format!(
            "{}: a Git LFS pointer, run git lfs pull in ../0ad",
            path.display()
        ));
    }
    Ok(bytes)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn write_brotli(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut writer =
        brotli::CompressorWriter::new(Vec::new(), 4096, BROTLI_QUALITY, BROTLI_WINDOW_BITS);
    writer
        .write_all(bytes)
        .map_err(|error| format!("cannot compress: {error}"))?;
    let mut copy = path.as_os_str().to_owned();
    copy.push(".br");
    write(&PathBuf::from(copy), &writer.into_inner())
}

/// Writes `<key>-<hash>.<extension>`, with a brotli copy if asked, and
/// returns its name.
fn put(
    output: &Path,
    key: &str,
    extension: &str,
    bytes: &[u8],
    brotli: bool,
) -> Result<String, String> {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    let name = format!("{key}-{:016x}.{extension}", hasher.finish());
    let path = output.join(&name);
    write(&path, bytes)?;
    if brotli {
        write_brotli(&path, bytes)?;
    }
    Ok(name)
}

/// (civ code, Identity/Icon) of every player template but the abstract
/// ones, as `Templates.js` names a civ's `Emblem`.
fn civ_emblems(mod_dir: &Path) -> Result<Vec<(String, String)>, String> {
    let dir = mod_dir.join(PLAYERS);
    let entries = fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut emblems = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(civ) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".xml"))
        else {
            continue;
        };
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let document = roxmltree::Document::parse(&text)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let icon = document
            .descendants()
            .find(|node| node.has_tag_name("Identity"))
            .and_then(|identity| identity.children().find(|node| node.has_tag_name("Icon")))
            .and_then(|icon| icon.text());
        if let Some(icon) = icon {
            emblems.push((civ.to_string(), icon.trim().to_string()));
        }
    }
    emblems.sort();
    Ok(emblems)
}

/// (civ code, path under `UI`) of each `background_circle_<civ>.png`, as
/// `MiniMapPanel.setCivBackgroundTexture` names them.
fn minimap_backgrounds(ui: &Path) -> Result<Vec<(String, String)>, String> {
    let dir = ui.join("session/icons/bkg");
    let entries = fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut backgrounds = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(civ) = name
            .strip_prefix("background_circle_")
            .and_then(|rest| rest.strip_suffix(".png"))
        {
            backgrounds.push((civ.to_string(), format!("session/icons/bkg/{name}")));
        }
    }
    backgrounds.sort();
    Ok(backgrounds)
}

/// `canvas2d.fs` with a grayscale factor of 1, on the texture's values.
fn grayscale(image: &mut Rgba) {
    for pixel in image.pixels.chunks_exact_mut(4) {
        let grey =
            0.3 * f32::from(pixel[0]) + 0.59 * f32::from(pixel[1]) + 0.11 * f32::from(pixel[2]);
        let grey = grey.round().clamp(0.0, 255.0) as u8;
        pixel[..3].fill(grey);
    }
}

fn resize(image: Rgba, side: u32) -> Result<Rgba, String> {
    if image.width.max(image.height) <= side {
        return Ok(image);
    }
    let scale = |length: u32| (length * side / image.width.max(image.height)).max(1);
    let (width, height) = (scale(image.width), scale(image.height));
    let source = Image::from_vec_u8(image.width, image.height, image.pixels, PixelType::U8x4)
        .map_err(|error| error.to_string())?;
    let mut resized = Image::new(width, height, PixelType::U8x4);
    Resizer::new()
        .resize(&source, &mut resized, None)
        .map_err(|error| error.to_string())?;
    Ok(Rgba {
        width,
        height,
        pixels: resized.into_vec(),
    })
}

fn encode_webp(image: &Rgba) -> Result<Vec<u8>, String> {
    Ok(
        webp::Encoder::from_rgba(&image.pixels, image.width, image.height)
            .encode(WEBP_QUALITY)
            .to_vec(),
    )
}

fn encode_png(image: &Rgba) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer
        .write_image_data(&image.pixels)
        .map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(out)
}

fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let size = reader.output_buffer_size().ok_or("image too large")?;
    let mut pixels = vec![0; size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|error| error.to_string())?;
    pixels.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgba => pixels,
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        png::ColorType::Grayscale => pixels
            .iter()
            .flat_map(|&grey| [grey, grey, grey, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks_exact(2)
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        png::ColorType::Indexed => return Err("palette left unexpanded".into()),
    };
    Ok(Rgba {
        width: info.width,
        height: info.height,
        pixels,
    })
}

/// An uncompressed DDS with channel masks, the only kind this step reads.
fn decode_dds(bytes: &[u8]) -> Result<Rgba, &'static str> {
    const HEADER: usize = 128;
    const FOURCC_FLAG: u32 = 4;
    if bytes.len() < HEADER || &bytes[..4] != b"DDS " {
        return Err("not a DDS file");
    }
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let height = u32_at(12);
    let width = u32_at(16);
    if u32_at(80) & FOURCC_FLAG != 0 {
        return Err("compressed DDS, not expected here");
    }
    let bits = u32_at(88);
    if bits != 32 {
        return Err("only 32-bit uncompressed DDS is read");
    }
    let masks = [u32_at(92), u32_at(96), u32_at(100), u32_at(104)];
    let count = width as usize * height as usize;
    let data = bytes
        .get(HEADER..HEADER + count * 4)
        .ok_or("DDS truncated")?;
    let mut pixels = Vec::with_capacity(count * 4);
    for texel in data.chunks_exact(4) {
        let value = u32::from_le_bytes(texel.try_into().unwrap());
        for mask in masks {
            pixels.push(if mask == 0 {
                255
            } else {
                ((value & mask) >> mask.trailing_zeros()) as u8
            });
        }
    }
    Ok(Rgba {
        width,
        height,
        pixels,
    })
}
