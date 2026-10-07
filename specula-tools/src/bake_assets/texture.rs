//! The engine's cached DDS as the client uploads it: BC blocks kept as
//! they are where the GPU can take them, every mip kept as the engine
//! uploads them at its default texture quality, and where the coarse
//! file of the smallest mips starts. The viewer's quality caps what it
//! fetches.

use specula_assets::bc;
use specula_assets::files::Texture;
use specula_assets::files::TextureFormat;
use specula_assets::files::level_extent;

/// The largest mip of the coarse file: enough for a unit a few dozen
/// pixels tall and for its impostor cells, and a few KB.
const COARSE_SIDE: u32 = 64;
const HEADER_SIZE: usize = 128;
const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_FOURCC: u32 = 0x4;

pub fn convert(bytes: &[u8]) -> Result<Texture, String> {
    if bytes.len() < HEADER_SIZE || &bytes[..4] != b"DDS " {
        return Err("not a DDS file".into());
    }
    let u32_at = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let flags = u32_at(8);
    let height = u32_at(12);
    let width = u32_at(16);
    let mip_count = if flags & DDSD_MIPMAPCOUNT != 0 {
        u32_at(28).max(1)
    } else {
        1
    };
    let pixel_flags = u32_at(80);
    if width == 0 || height == 0 {
        return Err("empty DDS".into());
    }
    let format = if pixel_flags & DDPF_FOURCC != 0 {
        match &bytes[84..88] {
            b"DXT1" => Some(bc::Format::Bc1),
            b"DXT3" => Some(bc::Format::Bc2),
            b"DXT5" => Some(bc::Format::Bc3),
            other => {
                return Err(format!(
                    "DDS format {} is not supported",
                    String::from_utf8_lossy(other)
                ));
            }
        }
    } else {
        None
    };

    // Each level in RGBA8 if uncompressed, else as blocks.
    let mut data = &bytes[HEADER_SIZE..];
    let mut levels = Vec::new();
    for level in 0..mip_count {
        let (w, h) = level_extent(width, height, level);
        let level = match format {
            Some(format) => take(&mut data, format.size(w, h))?.to_vec(),
            None => {
                let bits = u32_at(88);
                let masks = [u32_at(92), u32_at(96), u32_at(100), u32_at(104)];
                let has_alpha = pixel_flags & DDPF_ALPHAPIXELS != 0 || masks[3] != 0;
                unpack(
                    take(&mut data, w as usize * h as usize * (bits as usize / 8))?,
                    bits,
                    masks,
                    has_alpha,
                )?
            }
        };
        levels.push(level);
    }

    // The client needs the chain down to 1x1; without it there is
    // nothing smaller to take, so decode and build the chain.
    let full_chain = levels.len() as u32 == 32 - width.max(height).leading_zeros();
    // WebGPU takes BC only when the top level is whole blocks.
    let blocks_whole = width % 4 == 0 && height % 4 == 0;
    if let Some(format) = format
        && full_chain
        && blocks_whole
    {
        return Ok(Texture {
            format: match format {
                bc::Format::Bc1 => TextureFormat::Bc1,
                bc::Format::Bc2 => TextureFormat::Bc2,
                bc::Format::Bc3 => TextureFormat::Bc3,
            },
            width,
            height,
            levels,
            details: [None; 3],
        });
    }

    let rgba = match format {
        Some(format) => bc::decode(format, width, height, &levels[0])?,
        None => levels.swap_remove(0),
    };
    let (mut w, mut h) = (width, height);
    let mut chain = vec![rgba];
    while w > 1 || h > 1 {
        let (smaller, sw, sh) = halve(chain.last().unwrap(), w, h);
        chain.push(smaller);
        (w, h) = (sw, sh);
    }
    Ok(Texture {
        format: TextureFormat::Rgba8,
        width,
        height,
        levels: chain,
        details: [None; 3],
    })
}

/// The first level of the coarse file.
pub fn coarse_level(texture: &Texture) -> usize {
    texture.first_level(COARSE_SIDE)
}

fn take<'a>(data: &mut &'a [u8], length: usize) -> Result<&'a [u8], String> {
    if data.len() < length {
        return Err("DDS truncated".into());
    }
    let (bytes, rest) = data.split_at(length);
    *data = rest;
    Ok(bytes)
}

/// Uncompressed DDS pixels by their channel masks; the converter writes
/// 32-bit RGBA and 8-bit alpha (`CTextureConverter::ConvertTexture`).
fn unpack(pixels: &[u8], bits: u32, masks: [u32; 4], has_alpha: bool) -> Result<Vec<u8>, String> {
    let bytes = bits as usize / 8;
    if !(1..=4).contains(&bytes) || !bits.is_multiple_of(8) {
        return Err(format!("uncompressed DDS of {bits} bits is not supported"));
    }
    let channel = |pixel: u32, mask: u32| -> Option<u8> {
        if mask == 0 {
            return None;
        }
        let shift = mask.trailing_zeros();
        let max = mask >> shift;
        Some((((pixel & mask) >> shift) * 255 / max) as u8)
    };
    let mut rgba = Vec::with_capacity(pixels.len() / bytes * 4);
    for chunk in pixels.chunks_exact(bytes) {
        let mut raw = [0u8; 4];
        raw[..bytes].copy_from_slice(chunk);
        let pixel = u32::from_le_bytes(raw);
        let alpha = if has_alpha {
            channel(pixel, masks[3]).unwrap_or(0xFF)
        } else {
            0xFF
        };
        // An alpha-only texture is white with that alpha.
        let [r, g, b] = [0, 1, 2].map(|i| channel(pixel, masks[i]).unwrap_or(0xFF));
        rgba.extend_from_slice(&[r, g, b, alpha]);
    }
    Ok(rgba)
}

/// A box filter to half size, each side down to 1.
fn halve(rgba: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let (w, h) = ((width / 2).max(1), (height / 2).max(1));
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            for c in 0..4 {
                let mut sum = 0u32;
                let mut count = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(width - 1);
                    let sy = (y * 2 + dy).min(height - 1);
                    sum += u32::from(rgba[((sy * width + sx) * 4 + c) as usize]);
                    count += 1;
                }
                out.push(((sum + count / 2) / count) as u8);
            }
        }
    }
    (out, w, h)
}
