//! The session portraits as lossy WebP for the details panel's `<img>`.

use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;

use fast_image_resize::PixelType;
use fast_image_resize::Resizer;
use fast_image_resize::images::Image;

pub const DIR: &str = "art/textures/ui/session/portraits";
const TECHNOLOGIES: &str = "simulation/data/technologies";
/// Larger sources are scaled down to this, smaller ones kept: one file per
/// portrait, sharp enough for the panel on a high density screen.
const SIDE: u32 = 128;
/// About 2.6 KB a portrait (CONTROL-IMPL.md 5.3), against 20 KB lossless.
const QUALITY: f32 = 85.0;
/// Holds only the sheets for the GUI's own sprites, no PNG portraits.
const SHEETS: &str = "sheets";

/// Every portrait under `mod_dir` by its path relative to `DIR`, as WebP,
/// and why the others could not be baked.
pub fn bake(mod_dir: &Path) -> (Vec<(String, Vec<u8>)>, Vec<String>) {
    let root = mod_dir.join(DIR);
    let mut names = Vec::new();
    let mut errors = Vec::new();
    if let Err(error) = list(&root, "", &mut names) {
        errors.push(error);
    }
    names.sort();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Encoded>>> = Mutex::new((0..names.len()).map(|_| None).collect());
    let workers = thread::available_parallelism().map_or(1, |count| count.get());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some(name) = names.get(at) else {
                        break;
                    };
                    let result = encode(&root.join(name));
                    results.lock().unwrap()[at] = Some(result);
                }
            });
        }
    });
    let mut portraits = Vec::new();
    for (name, result) in names.into_iter().zip(results.into_inner().unwrap()) {
        match result.unwrap_or_else(|| Err("not encoded".into())) {
            Ok(bytes) => portraits.push((name, bytes)),
            Err(error) => errors.push(format!("{DIR}/{name}: {error}")),
        }
    }
    (portraits, errors)
}

fn list(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        if entry.path().is_dir() {
            if path != SHEETS {
                list(&entry.path(), &format!("{path}/"), out)?;
            }
        } else if name.ends_with(".png") {
            out.push(path);
        }
    }
    Ok(())
}

/// Each technology under `mod_dir` with an icon, by its path under
/// `TECHNOLOGIES` without `.json`, -> the icon relative to `DIR`, as
/// `GetTechnologyData` names it; and the files that could not be read.
pub fn technology_icons(mod_dir: &Path) -> (Vec<(String, String)>, Vec<String>) {
    let root = mod_dir.join(TECHNOLOGIES);
    let mut names = Vec::new();
    let mut errors = Vec::new();
    if let Err(error) = list_json(&root, "", &mut names) {
        errors.push(error);
    }
    names.sort();
    let mut icons = Vec::new();
    for name in names {
        let path = root.join(format!("{name}.json"));
        let value = fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<serde_json::Value>(&bytes)
                    .map_err(|error| error.to_string())
            });
        match value {
            Ok(value) => {
                if let Some(icon) = value.get("icon").and_then(serde_json::Value::as_str) {
                    icons.push((name, format!("technologies/{icon}")));
                }
            }
            Err(error) => errors.push(format!("{TECHNOLOGIES}/{name}.json: {error}")),
        }
    }
    (icons, errors)
}

fn list_json(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() {
            list_json(&entry.path(), &format!("{prefix}{name}/"), out)?;
        } else if let Some(stem) = name.strip_suffix(".json") {
            out.push(format!("{prefix}{stem}"));
        }
    }
    Ok(())
}

/// A portrait as WebP, or why it could not be.
type Encoded = Result<Vec<u8>, String>;

fn encode(path: &Path) -> Encoded {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.starts_with(b"version https://git-lfs") {
        return Err("a Git LFS pointer, the file was not fetched".into());
    }
    let (width, height, alpha, pixels) = decode_png(&bytes)?;
    let pixel_type = if alpha {
        PixelType::U8x4
    } else {
        PixelType::U8x3
    };
    let (width, height, pixels) = if width.max(height) > SIDE {
        // Rounded, at least 1, so a non-square source keeps its aspect.
        let scale = |side: u32| ((side * SIDE + width.max(height) / 2) / width.max(height)).max(1);
        let (to_width, to_height) = (scale(width), scale(height));
        let source = Image::from_vec_u8(width, height, pixels, pixel_type)
            .map_err(|error| error.to_string())?;
        let mut resized = Image::new(to_width, to_height, pixel_type);
        Resizer::new()
            .resize(&source, &mut resized, None)
            .map_err(|error| error.to_string())?;
        (to_width, to_height, resized.into_vec())
    } else {
        (width, height, pixels)
    };
    let encoder = if alpha {
        webp::Encoder::from_rgba(&pixels, width, height)
    } else {
        webp::Encoder::from_rgb(&pixels, width, height)
    };
    Ok(encoder.encode(QUALITY).to_vec())
}

/// Width, height, whether it has alpha, and its pixels as RGB or RGBA.
fn decode_png(bytes: &[u8]) -> Result<(u32, u32, bool, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let size = reader.output_buffer_size().ok_or("image too large")?;
    let mut pixels = vec![0; size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|error| error.to_string())?;
    pixels.truncate(info.buffer_size());
    let alpha = match info.color_type {
        png::ColorType::Rgb => false,
        png::ColorType::Rgba => true,
        png::ColorType::Grayscale => {
            pixels = pixels.iter().flat_map(|&grey| [grey; 3]).collect();
            false
        }
        png::ColorType::GrayscaleAlpha => {
            pixels = pixels
                .chunks_exact(2)
                .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
                .collect();
            true
        }
        png::ColorType::Indexed => return Err("palette left unexpanded".into()),
    };
    Ok((info.width, info.height, alpha, pixels))
}
