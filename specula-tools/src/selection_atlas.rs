//! Packs the textures the engine draws selection overlays with
//! (`CCmpSelectable`) and status bars with (`StatusBars.js`) into one file
//! the web client loads, so the browser needs no PNG decoder.
//!
//! Usage: selection-atlas [mod dir] [output]
//! Defaults: ../0ad/binaries/data/mods/public and specula-web/selection.bin.
//! A brotli copy is written next to the output, with `.br` appended.
//!
//! Every `X.png` under art/textures/selection/ (except `auras/`) is paired
//! with its `X_mask.png`. Those in a subdirectory are the quads drawn under
//! units and go into one atlas; those at the top level are line textures
//! (outlines, range overlays) and are kept whole, since a line repeats its
//! texture along its length.
//!
//! The status bar sprites are the bar textures under
//! art/textures/ui/session/icons/ and every rank icon in its ranks/. They
//! have no mask, as `AddSprite` tints a plain texture, and go into an atlas
//! of their own. A sprite one texel wide (or tall) is repeated across a
//! square cell: stretched along that side, it looks the same, and the mips
//! do not blend it with its neighbour.
//!
//! Output, format v1, little-endian: u8 version, u32 atlas width, u32
//! atlas height, u32 quad count, count x {u16 name length, UTF-8 name, u16
//! mask name length, UTF-8 mask name, u16 x, u16 y, u16 width, u16 height},
//! u32 line count, count x {u16 name length, UTF-8 name, u16 mask name
//! length, UTF-8 mask name, u32 width, u32 height}, then the atlas as
//! width * height RGBA8 pixels and width * height R8 mask values, then
//! each line's RGBA8 pixels and R8 mask values, then u32 sprite atlas
//! width, u32 sprite atlas height, u32 sprite count, count x {u16 name
//! length, UTF-8 name, u16 x, u16 y, u16 width, u16 height}, then the
//! sprite atlas as width * height RGBA8 pixels, all row-major. Only the
//! mask's red channel is kept, as the engine's `overlayline.fs` reads only
//! that. Quad and line names are relative to art/textures/selection/, as
//! the observer hook sends them; sprite names to
//! art/textures/ui/session/icons/, as `StatusBars.js` builds its paths.

use std::fs;
use std::io::Cursor;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

const DEFAULT_MOD: &str = "../0ad/binaries/data/mods/public";
const DEFAULT_OUTPUT: &str = "specula-web/selection.bin";
const VERSION: u8 = 1;
const TEXTURE_DIRECTORY: &str = "art/textures/selection";
const SPRITE_DIRECTORY: &str = "art/textures/ui/session/icons";
/// Every bar `StatusBars.js` draws asks for one of these, so each must exist.
const BAR_SPRITES: [&str; 9] = [
    "capture_bar.png",
    "health_bg.png",
    "health_fg.png",
    "pack_bg.png",
    "pack_fg.png",
    "supply_bg.png",
    "supply_fg.png",
    "upgrade_bg.png",
    "upgrade_fg.png",
];
/// `Identity`'s rank is free text, so all of them.
const RANK_DIRECTORY: &str = "ranks";
/// Aura range textures are 1024 px squares, larger than all the rest
/// together, and nothing draws auras yet.
const SKIPPED_DIRECTORY: &str = "auras";
const MASK_SUFFIX: &str = "_mask.png";
const ATLAS_WIDTH: u32 = 1024;
/// The tallest quad, so the atlas height stays a multiple of every quad
/// height and the mip chain divides evenly.
const ATLAS_HEIGHT_STEP: u32 = 512;
const BROTLI_QUALITY: u32 = 11;
const BROTLI_WINDOW_BITS: u32 = 22;

struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

struct Texture {
    name: String,
    mask_name: String,
    base: Image,
    mask: Vec<u8>,
}

struct Sprite {
    name: String,
    image: Image,
}

struct Placed<T> {
    item: T,
    x: u32,
    y: u32,
}

struct SpriteAtlas {
    width: u32,
    height: u32,
    sprites: Vec<Placed<Sprite>>,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mod_dir = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_MOD.into()));
    let output = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_OUTPUT.into()));
    let root = mod_dir.join(TEXTURE_DIRECTORY);

    let mut quad_names = Vec::new();
    let mut line_names = Vec::new();
    let Ok(entries) = fs::read_dir(&root) else {
        eprintln!("cannot read {}", root.display());
        return ExitCode::FAILURE;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if file_name != SKIPPED_DIRECTORY {
                collect_bases(&root, &file_name, &mut quad_names);
            }
        } else if is_base(&file_name) {
            line_names.push(file_name);
        }
    }
    quad_names.sort();
    line_names.sort();

    let mut quads = Vec::new();
    let mut lines = Vec::new();
    for (names, out) in [(quad_names, &mut quads), (line_names, &mut lines)] {
        for name in names {
            match load(&root, &name) {
                Ok(Some(texture)) => out.push(texture),
                Ok(None) => eprintln!("skipped {name}: no {MASK_SUFFIX}"),
                Err(error) => {
                    eprintln!("{name}: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    let sprites = match load_sprites(&mod_dir.join(SPRITE_DIRECTORY)) {
        Ok(sprites) => sprites,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let (placed, atlas_height) = match pack(
        quads,
        |quad| (quad.base.width, quad.base.height),
        |quad| &quad.name,
        ATLAS_WIDTH,
        ATLAS_HEIGHT_STEP,
    ) {
        Ok(packed) => packed,
        Err(error) => {
            eprintln!("cannot pack the atlas: {error}");
            return ExitCode::FAILURE;
        }
    };
    // The largest cell, so every cell divides both sides.
    let sprite_side = sprites
        .iter()
        .map(|sprite| sprite.image.width)
        .max()
        .unwrap_or(1);
    let (sprites, sprite_height) = match pack(
        sprites,
        |sprite| (sprite.image.width, sprite.image.height),
        |sprite| &sprite.name,
        sprite_side,
        sprite_side,
    ) {
        Ok(packed) => packed,
        Err(error) => {
            eprintln!("cannot pack the sprites: {error}");
            return ExitCode::FAILURE;
        }
    };
    let sprite_atlas = SpriteAtlas {
        width: sprite_side,
        height: sprite_height,
        sprites,
    };
    let bytes = match encode(&placed, atlas_height, &lines, &sprite_atlas) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("cannot write the atlas: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = fs::write(&output, &bytes) {
        eprintln!("cannot write {}: {error}", output.display());
        return ExitCode::FAILURE;
    }

    let mut copy = output.clone().into_os_string();
    copy.push(".br");
    let copy = PathBuf::from(copy);
    let mut writer =
        brotli::CompressorWriter::new(Vec::new(), 4096, BROTLI_QUALITY, BROTLI_WINDOW_BITS);
    if let Err(error) = writer.write_all(&bytes) {
        eprintln!("cannot compress: {error}");
        return ExitCode::FAILURE;
    }
    let packed = writer.into_inner();
    if let Err(error) = fs::write(&copy, &packed) {
        eprintln!("cannot write {}: {error}", copy.display());
        return ExitCode::FAILURE;
    }

    println!(
        "{} quads in a {ATLAS_WIDTH}x{atlas_height} atlas, {} lines, {} sprites in a {}x{} atlas, {} bytes written to {}, {} to {}",
        placed.len(),
        lines.len(),
        sprite_atlas.sprites.len(),
        sprite_atlas.width,
        sprite_atlas.height,
        bytes.len(),
        output.display(),
        packed.len(),
        copy.display()
    );
    ExitCode::SUCCESS
}

fn is_base(file_name: &str) -> bool {
    file_name.ends_with(".png") && !file_name.ends_with(MASK_SUFFIX)
}

fn collect_bases(root: &Path, directory: &str, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root.join(directory)) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_file() && is_base(&file_name) {
            out.push(format!("{directory}/{file_name}"));
        }
    }
}

/// None when the base has no mask; the engine would draw it without player
/// colour, which no template asks for.
fn load(root: &Path, name: &str) -> Result<Option<Texture>, String> {
    let mask_name = format!("{}{MASK_SUFFIX}", name.trim_end_matches(".png"));
    let mask_path = root.join(&mask_name);
    if !mask_path.is_file() {
        return Ok(None);
    }
    let base = read_png(&root.join(name))?;
    let mask = read_png(&mask_path)?;
    // The shader samples both at the same coordinates.
    if (base.width, base.height) != (mask.width, mask.height) {
        return Err(format!(
            "the base is {}x{} but the mask {}x{}",
            base.width, base.height, mask.width, mask.height
        ));
    }
    let mask = mask.rgba.chunks_exact(4).map(|pixel| pixel[0]).collect();
    Ok(Some(Texture {
        name: name.to_string(),
        mask_name,
        base,
        mask,
    }))
}

fn load_sprites(root: &Path) -> Result<Vec<Sprite>, String> {
    let mut names: Vec<String> = BAR_SPRITES.iter().map(|name| name.to_string()).collect();
    let ranks = root.join(RANK_DIRECTORY);
    let entries = fs::read_dir(&ranks).map_err(|error| format!("{}: {error}", ranks.display()))?;
    let mut rank_names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|file_name| file_name.ends_with(".png"))
        .map(|file_name| format!("{RANK_DIRECTORY}/{file_name}"))
        .collect();
    rank_names.sort();
    names.extend(rank_names);
    names
        .into_iter()
        .map(|name| {
            let image =
                square(read_png(&root.join(&name))?).map_err(|error| format!("{name}: {error}"))?;
            Ok(Sprite { name, image })
        })
        .collect()
}

/// Repeats a one texel wide or tall image across a square.
fn square(image: Image) -> Result<Image, String> {
    let (width, height) = (image.width, image.height);
    if width == height {
        return Ok(image);
    }
    let side = width.max(height);
    let rgba = match (width, height) {
        (1, _) => image
            .rgba
            .chunks_exact(4)
            .flat_map(|pixel| pixel.repeat(side as usize))
            .collect(),
        (_, 1) => image.rgba.repeat(side as usize),
        _ => {
            return Err(format!(
                "{width}x{height} is neither square nor one texel thin"
            ));
        }
    };
    Ok(Image {
        width: side,
        height: side,
        rgba,
    })
}

fn read_png(path: &Path) -> Result<Image, String> {
    fs::read(path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| decode_png(&bytes))
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn decode_png(bytes: &[u8]) -> Result<Image, String> {
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
    let rgba = pixels
        .chunks_exact(channels)
        .flat_map(|pixel| match channels {
            1 => [pixel[0], pixel[0], pixel[0], 0xFF],
            2 => [pixel[0], pixel[0], pixel[0], pixel[1]],
            3 => [pixel[0], pixel[1], pixel[2], 0xFF],
            _ => [pixel[0], pixel[1], pixel[2], pixel[3]],
        })
        .collect();
    Ok(Image {
        width: info.width,
        height: info.height,
        rgba,
    })
}

/// Each item sits at a multiple of its own width and height, and the sizes
/// are powers of two, so down the mip chain a texel never averages two
/// shapes until the shape itself is a single texel.
fn pack<T>(
    mut items: Vec<T>,
    size: impl Fn(&T) -> (u32, u32),
    name: impl Fn(&T) -> &str,
    atlas_width: u32,
    height_step: u32,
) -> Result<(Vec<Placed<T>>, u32), String> {
    items.sort_by_key(|item| {
        let (width, height) = size(item);
        std::cmp::Reverse(width * height)
    });
    let mut placed: Vec<Placed<T>> = Vec::new();
    for item in items {
        let (width, height) = size(&item);
        if width > atlas_width || !width.is_power_of_two() || !height.is_power_of_two() {
            return Err(format!("{} is {width}x{height}", name(&item)));
        }
        let free = |x: u32, y: u32| {
            placed.iter().all(|other| {
                let (other_width, other_height) = size(&other.item);
                x + width <= other.x
                    || other.x + other_width <= x
                    || y + height <= other.y
                    || other.y + other_height <= y
            })
        };
        let (x, y) = (0..)
            .map(|row| row * height)
            .find_map(|y| {
                (0..atlas_width / width)
                    .map(|column| column * width)
                    .find(|&x| free(x, y))
                    .map(|x| (x, y))
            })
            .expect("rows below every placed item are free");
        placed.push(Placed { item, x, y });
    }
    let bottom = placed
        .iter()
        .map(|placed| placed.y + size(&placed.item).1)
        .max()
        .unwrap_or(0);
    let height = bottom.div_ceil(height_step) * height_step;
    if height > u32::from(u16::MAX) {
        return Err(format!(
            "{atlas_width}x{height} does not fit u16 rectangles"
        ));
    }
    Ok((placed, height))
}

fn blit(
    atlas: &mut [u8],
    atlas_width: u32,
    x: u32,
    y: u32,
    image: &[u8],
    width: u32,
    channels: usize,
) {
    let width = width as usize;
    let row_bytes = width * channels;
    for (row, source) in image.chunks_exact(row_bytes).enumerate() {
        let start = ((y as usize + row) * atlas_width as usize + x as usize) * channels;
        atlas[start..start + row_bytes].copy_from_slice(source);
    }
}

fn push_name(out: &mut Vec<u8>, name: &str) -> Result<(), String> {
    let length = u16::try_from(name.len()).map_err(|_| format!("name too long: {name}"))?;
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    Ok(())
}

fn encode(
    placed: &[Placed<Texture>],
    atlas_height: u32,
    lines: &[Texture],
    sprites: &SpriteAtlas,
) -> Result<Vec<u8>, String> {
    let mut out = vec![VERSION];
    out.extend_from_slice(&ATLAS_WIDTH.to_le_bytes());
    out.extend_from_slice(&atlas_height.to_le_bytes());
    out.extend_from_slice(&(placed.len() as u32).to_le_bytes());
    let pixels = (ATLAS_WIDTH * atlas_height) as usize;
    let mut atlas = vec![0u8; pixels * 4];
    let mut atlas_mask = vec![0u8; pixels];
    for quad in placed {
        let texture = &quad.item;
        push_name(&mut out, &texture.name)?;
        push_name(&mut out, &texture.mask_name)?;
        for value in [quad.x, quad.y, texture.base.width, texture.base.height] {
            out.extend_from_slice(&(value as u16).to_le_bytes());
        }
        let width = texture.base.width;
        blit(
            &mut atlas,
            ATLAS_WIDTH,
            quad.x,
            quad.y,
            &texture.base.rgba,
            width,
            4,
        );
        blit(
            &mut atlas_mask,
            ATLAS_WIDTH,
            quad.x,
            quad.y,
            &texture.mask,
            width,
            1,
        );
    }

    out.extend_from_slice(&(lines.len() as u32).to_le_bytes());
    for line in lines {
        push_name(&mut out, &line.name)?;
        push_name(&mut out, &line.mask_name)?;
        out.extend_from_slice(&line.base.width.to_le_bytes());
        out.extend_from_slice(&line.base.height.to_le_bytes());
    }
    out.extend_from_slice(&atlas);
    out.extend_from_slice(&atlas_mask);
    for line in lines {
        out.extend_from_slice(&line.base.rgba);
        out.extend_from_slice(&line.mask);
    }

    out.extend_from_slice(&sprites.width.to_le_bytes());
    out.extend_from_slice(&sprites.height.to_le_bytes());
    out.extend_from_slice(&(sprites.sprites.len() as u32).to_le_bytes());
    let mut sprite_atlas = vec![0u8; (sprites.width * sprites.height) as usize * 4];
    for sprite in &sprites.sprites {
        let image = &sprite.item.image;
        push_name(&mut out, &sprite.item.name)?;
        for value in [sprite.x, sprite.y, image.width, image.height] {
            out.extend_from_slice(&(value as u16).to_le_bytes());
        }
        blit(
            &mut sprite_atlas,
            sprites.width,
            sprite.x,
            sprite.y,
            &image.rgba,
            image.width,
            4,
        );
    }
    out.extend_from_slice(&sprite_atlas);
    Ok(out)
}
