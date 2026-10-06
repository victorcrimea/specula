//! Decodes the unit skins to RGBA8, top row first like the engine's
//! default `TEX_TOP_DOWN` orientation, so mesh UVs apply unchanged.
//! Skins are PNG or DDS; every DDS among them is DXT1, DXT3 or DXT5.

use std::io::Cursor;

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Stands in for a missing texture so the part still shows.
    pub fn white() -> Self {
        Image {
            width: 1,
            height: 1,
            rgba: vec![0xFF; 4],
        }
    }
}

pub fn decode(name: &str, bytes: &[u8]) -> Result<Image, String> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        decode_png(bytes)
    } else if lower.ends_with(".dds") {
        decode_dds(bytes)
    } else {
        Err(format!("unknown texture format: {name}"))
    }
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

const DDS_HEADER_SIZE: usize = 128;
const DDPF_FOURCC: u32 = 0x4;

#[derive(Clone, Copy)]
enum Dxt {
    Dxt1,
    Dxt3,
    Dxt5,
}

/// Only the top mip level is decoded; the client does not mipmap yet.
fn decode_dds(bytes: &[u8]) -> Result<Image, String> {
    if bytes.len() < DDS_HEADER_SIZE || &bytes[..4] != b"DDS " {
        return Err("not a DDS file".into());
    }
    let u32_at = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let height = u32_at(12);
    let width = u32_at(16);
    let pixel_flags = u32_at(80);
    if pixel_flags & DDPF_FOURCC == 0 {
        return Err("uncompressed DDS is not supported".into());
    }
    let format = match &bytes[84..88] {
        b"DXT1" => Dxt::Dxt1,
        b"DXT3" => Dxt::Dxt3,
        b"DXT5" => Dxt::Dxt5,
        other => {
            return Err(format!(
                "DDS format {} is not supported",
                String::from_utf8_lossy(other)
            ));
        }
    };
    let block_size = match format {
        Dxt::Dxt1 => 8,
        Dxt::Dxt3 | Dxt::Dxt5 => 16,
    };
    let blocks_wide = width.div_ceil(4) as usize;
    let blocks_high = height.div_ceil(4) as usize;
    let data = &bytes[DDS_HEADER_SIZE..];
    if data.len() < blocks_wide * blocks_high * block_size {
        return Err("DDS truncated".into());
    }

    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
            let offset = (block_y * blocks_wide + block_x) * block_size;
            let block = &data[offset..offset + block_size];
            let texels = decode_block(format, block);
            for (i, texel) in texels.iter().enumerate() {
                let x = block_x * 4 + i % 4;
                let y = block_y * 4 + i / 4;
                // Blocks overhang images whose size is not a multiple of 4.
                if x < width as usize && y < height as usize {
                    let start = (y * width as usize + x) * 4;
                    rgba[start..start + 4].copy_from_slice(texel);
                }
            }
        }
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

fn decode_block(format: Dxt, block: &[u8]) -> [[u8; 4]; 16] {
    match format {
        Dxt::Dxt1 => decode_color(block, true),
        Dxt::Dxt3 => {
            let mut texels = decode_color(&block[8..], false);
            let alpha = u64::from_le_bytes(block[..8].try_into().unwrap());
            for (i, texel) in texels.iter_mut().enumerate() {
                texel[3] = ((alpha >> (4 * i)) & 0xF) as u8 * 17;
            }
            texels
        }
        Dxt::Dxt5 => {
            let mut texels = decode_color(&block[8..], false);
            let alphas = dxt5_alphas(block[0], block[1]);
            let mut indices = [0u8; 8];
            indices[..6].copy_from_slice(&block[2..8]);
            let indices = u64::from_le_bytes(indices);
            for (i, texel) in texels.iter_mut().enumerate() {
                texel[3] = alphas[((indices >> (3 * i)) & 0x7) as usize];
            }
            texels
        }
    }
}

/// `allow_transparent`: only DXT1 has the 3-colour mode with transparent
/// black; DXT3 and DXT5 colour blocks always use 4 colours.
fn decode_color(block: &[u8], allow_transparent: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let [r0, g0, b0] = expand_565(c0);
    let [r1, g1, b1] = expand_565(c1);
    let mix = |a: u8, b: u8, wa: u32, wb: u32| {
        ((u32::from(a) * wa + u32::from(b) * wb) / (wa + wb)) as u8
    };
    let palette = if c0 > c1 || !allow_transparent {
        [
            [r0, g0, b0, 0xFF],
            [r1, g1, b1, 0xFF],
            [
                mix(r0, r1, 2, 1),
                mix(g0, g1, 2, 1),
                mix(b0, b1, 2, 1),
                0xFF,
            ],
            [
                mix(r0, r1, 1, 2),
                mix(g0, g1, 1, 2),
                mix(b0, b1, 1, 2),
                0xFF,
            ],
        ]
    } else {
        [
            [r0, g0, b0, 0xFF],
            [r1, g1, b1, 0xFF],
            [
                mix(r0, r1, 1, 1),
                mix(g0, g1, 1, 1),
                mix(b0, b1, 1, 1),
                0xFF,
            ],
            [0, 0, 0, 0],
        ]
    };
    let indices = u32::from_le_bytes(block[4..8].try_into().unwrap());
    std::array::from_fn(|i| palette[((indices >> (2 * i)) & 0x3) as usize])
}

fn expand_565(color: u16) -> [u8; 3] {
    let r = ((color >> 11) & 0x1F) as u8;
    let g = ((color >> 5) & 0x3F) as u8;
    let b = (color & 0x1F) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

fn dxt5_alphas(a0: u8, a1: u8) -> [u8; 8] {
    let (a0, a1) = (u32::from(a0), u32::from(a1));
    let mut alphas = [a0, a1, 0, 0, 0, 0, 0, 0];
    if a0 > a1 {
        for i in 1..7 {
            alphas[i + 1] = (a0 * (7 - i as u32) + a1 * i as u32) / 7;
        }
    } else {
        for i in 1..5 {
            alphas[i + 1] = (a0 * (5 - i as u32) + a1 * i as u32) / 5;
        }
        alphas[6] = 0;
        alphas[7] = 255;
    }
    alphas.map(|alpha| alpha as u8)
}
