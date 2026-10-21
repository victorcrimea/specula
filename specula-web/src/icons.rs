//! Decoder for the minimap icon atlas, format v0, built by specula-tools'
//! minimap-icons (layout documented there) and served as `/icons.bin`.

use std::collections::HashMap;

use specula_assets::bytes::ByteReader;

const VERSION: u8 = 0;

pub struct IconAtlas {
    pub width: u32,
    pub height: u32,
    /// RGBA8, row-major, straight alpha.
    pub pixels: Vec<u8>,
    /// Icon name to its texture coordinates: left, top, right, bottom in
    /// 0..1.
    pub rects: HashMap<String, [f32; 4]>,
}

pub fn decode(bytes: &[u8]) -> Result<IconAtlas, &'static str> {
    let mut cursor = ByteReader(bytes);
    if cursor.u8()? != VERSION {
        return Err("unknown icon atlas version");
    }
    let width = cursor.u32()?;
    let height = cursor.u32()?;
    let count = cursor.u32()?;
    let mut rects = HashMap::with_capacity(count as usize);
    for _ in 0..count {
        let length = usize::from(cursor.u16()?);
        let name = std::str::from_utf8(cursor.take(length)?)
            .map_err(|_| "icon name not UTF-8")?
            .to_string();
        let [x, y, w, h] =
            [cursor.u16()?, cursor.u16()?, cursor.u16()?, cursor.u16()?].map(f32::from);
        rects.insert(
            name,
            [
                x / width as f32,
                y / height as f32,
                (x + w) / width as f32,
                (y + h) / height as f32,
            ],
        );
    }
    let pixels = cursor.take(width as usize * height as usize * 4)?.to_vec();
    if !cursor.0.is_empty() {
        return Err("icon atlas has trailing bytes");
    }
    Ok(IconAtlas {
        width,
        height,
        pixels,
        rects,
    })
}
