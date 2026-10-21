//! Decodes the unit skins to RGBA8, top row first like the engine's
//! default `TEX_TOP_DOWN` orientation, so mesh UVs apply unchanged.
//! Skins are PNG or DDS; every DDS among them is DXT1, DXT3 or DXT5.

use specula_assets::bc;
use specula_tools::dds;
use specula_tools::image::Image;
use specula_tools::image::decode_png;

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

/// Only the top mip level is decoded; the client does not mipmap yet.
fn decode_dds(bytes: &[u8]) -> Result<Image, String> {
    dds::check(bytes)?;
    let height = dds::u32_at(bytes, 12);
    let width = dds::u32_at(bytes, 16);
    let pixel_flags = dds::u32_at(bytes, 80);
    if pixel_flags & dds::DDPF_FOURCC == 0 {
        return Err("uncompressed DDS is not supported".into());
    }
    let format = dds::block_format(bytes)?;
    let data = &bytes[dds::HEADER_SIZE..];
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
