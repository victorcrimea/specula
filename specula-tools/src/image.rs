//! PNG skins and icons the offline tools read, as RGBA8.

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

pub fn decode_png(bytes: &[u8]) -> Result<Image, String> {
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
