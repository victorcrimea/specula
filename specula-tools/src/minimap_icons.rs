//! Packs the minimap icons the templates use into one RGBA atlas that the
//! web client loads, so the browser needs no PNG decoder.
//!
//! Usage: minimap-icons [mod dir] [output]
//! Defaults: ../0ad/binaries/data/mods/public and specula-web/icons.bin.
//!
//! Output, format v0, little-endian: u8 version, u32 atlas width, u32
//! height, u32 count, count x {u16 name length, UTF-8 name, u16 x, u16 y,
//! u16 width, u16 height}, then width * height RGBA8 pixels, row-major.
//! Names are relative to art/textures/ui/session/icons/minimap/, as the
//! observer hook sends them.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use specula_tools::image::Image;
use specula_tools::image::decode_png;

const DEFAULT_MOD: &str = "../0ad/binaries/data/mods/public";
const DEFAULT_OUTPUT: &str = "specula-web/icons.bin";
const VERSION: u8 = 0;
const ICON_DIRECTORY: &str = "art/textures/ui/session/icons/minimap";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mod_dir = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_MOD.into()));
    let output = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_OUTPUT.into()));

    let mut templates = Vec::new();
    collect_xml(&mod_dir.join("simulation/templates"), &mut templates);
    let mut names = BTreeSet::new();
    for template in &templates {
        if let Err(error) = collect_icons(template, &mut names) {
            eprintln!("skipped {}: {error}", template.display());
        }
    }

    let mut icons = Vec::new();
    for name in names {
        let path = mod_dir.join(ICON_DIRECTORY).join(&name);
        match fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| decode_png(&bytes))
        {
            Ok(image) => icons.push((name, image)),
            Err(error) => eprintln!("skipped {}: {error}", path.display()),
        }
    }

    let bytes = match pack(&icons) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("cannot pack the atlas: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = fs::write(&output, &bytes) {
        eprintln!("cannot write {}: {error}", output.display());
        return ExitCode::FAILURE;
    }
    let width: u32 = icons.iter().map(|(_, image)| image.width).sum();
    let height = icons
        .iter()
        .map(|(_, image)| image.height)
        .max()
        .unwrap_or(0);
    println!(
        "{} icons, {width}x{height} atlas, written to {}",
        icons.len(),
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

/// A template may override only the file and inherit the size attribute,
/// so any non-empty Minimap/Icon counts; an extra unused icon costs a few
/// hundred bytes, a missing one a dot where the game shows an icon.
fn collect_icons(template: &Path, names: &mut BTreeSet<String>) -> Result<(), String> {
    let xml = fs::read_to_string(template).map_err(|error| error.to_string())?;
    let document = roxmltree::Document::parse(&xml).map_err(|error| error.to_string())?;
    for node in document.descendants() {
        if !node.has_tag_name("Icon")
            || !node
                .parent_element()
                .is_some_and(|parent| parent.has_tag_name("Minimap"))
        {
            continue;
        }
        if let Some(name) = node.text().map(str::trim).filter(|name| !name.is_empty()) {
            names.insert(name.to_string());
        }
    }
    Ok(())
}

/// One row, left to right: a handful of 16 px icons needs no bin packing.
fn pack(icons: &[(String, Image)]) -> Result<Vec<u8>, String> {
    let width: u32 = icons.iter().map(|(_, image)| image.width).sum();
    let height = icons
        .iter()
        .map(|(_, image)| image.height)
        .max()
        .unwrap_or(0);
    if width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return Err(format!("{width}x{height} does not fit u16 rectangles"));
    }

    let mut out = Vec::new();
    out.push(VERSION);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&(icons.len() as u32).to_le_bytes());
    let mut atlas = vec![0u8; (width * height * 4) as usize];
    let mut x = 0;
    for (name, image) in icons {
        let length = u16::try_from(name.len()).map_err(|_| format!("name too long: {name}"))?;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        for value in [x, 0, image.width, image.height] {
            out.extend_from_slice(&(value as u16).to_le_bytes());
        }
        let row_bytes = (image.width * 4) as usize;
        for (row, pixels) in image.rgba.chunks_exact(row_bytes).enumerate() {
            let start = (row * width as usize + x as usize) * 4;
            atlas[start..start + row_bytes].copy_from_slice(pixels);
        }
        x += image.width;
    }
    out.extend_from_slice(&atlas);
    Ok(out)
}
