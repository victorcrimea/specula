//! Decodes the unit skins to RGBA8, top row first like the engine's
//! default `TEX_TOP_DOWN` orientation, so mesh UVs apply unchanged.
//! Skins are PNG or DDS; every DDS among them is DXT1, DXT3 or DXT5.

use std::io::Cursor;

use specula_assets::bc;
use specula_assets::bc::Format;

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
        b"DXT1" => Format::Bc1,
        b"DXT3" => Format::Bc2,
        b"DXT5" => Format::Bc3,
        other => {
            return Err(format!(
                "DDS format {} is not supported",
                String::from_utf8_lossy(other)
            ));
        }
    };
    let data = &bytes[DDS_HEADER_SIZE..];
    if data.len() < format.size(width, height) {
        return Err("DDS truncated".into());
    }
    let rgba = bc::decode(format, width, height, data)?;
    Ok(Image {
        width,
        height,
        rgba,
    })
}
