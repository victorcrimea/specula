//! The part of a DDS header both bakes read; what follows it differs.

use specula_assets::bc::Format;

pub const HEADER_SIZE: usize = 128;
pub const DDPF_FOURCC: u32 = 0x4;

/// The little-endian u32 at `offset` of a DDS file `check` accepted.
pub fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

pub fn check(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < HEADER_SIZE || &bytes[..4] != b"DDS " {
        return Err("not a DDS file".into());
    }
    Ok(())
}

/// The block format named by the pixel format's FourCC.
pub fn block_format(bytes: &[u8]) -> Result<Format, String> {
    match &bytes[84..88] {
        b"DXT1" => Ok(Format::Bc1),
        b"DXT3" => Ok(Format::Bc2),
        b"DXT5" => Ok(Format::Bc3),
        other => Err(format!(
            "DDS format {} is not supported",
            String::from_utf8_lossy(other)
        )),
    }
}
