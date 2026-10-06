//! BC1-3 (DXT1, DXT3, DXT5) decoding to RGBA8, for the bakes and for
//! browsers whose device lacks BC textures.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Bc1,
    Bc2,
    Bc3,
}

impl Format {
    pub fn block_size(self) -> usize {
        match self {
            Format::Bc1 => 8,
            Format::Bc2 | Format::Bc3 => 16,
        }
    }

    /// Bytes of a `width` x `height` image, whole blocks.
    pub fn size(self, width: u32, height: u32) -> usize {
        width.div_ceil(4) as usize * height.div_ceil(4) as usize * self.block_size()
    }
}

/// `data` is the blocks row by row, top first; so is the result.
pub fn decode(
    format: Format,
    width: u32,
    height: u32,
    data: &[u8],
) -> Result<Vec<u8>, &'static str> {
    let blocks_wide = width.div_ceil(4) as usize;
    let blocks_high = height.div_ceil(4) as usize;
    let block_size = format.block_size();
    if data.len() < blocks_wide * blocks_high * block_size {
        return Err("BC data truncated");
    }
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
            let offset = (block_y * blocks_wide + block_x) * block_size;
            let texels = decode_block(format, &data[offset..offset + block_size]);
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
    Ok(rgba)
}

fn decode_block(format: Format, block: &[u8]) -> [[u8; 4]; 16] {
    match format {
        Format::Bc1 => decode_color(block, true),
        Format::Bc2 => {
            let mut texels = decode_color(&block[8..], false);
            let alpha = u64::from_le_bytes(block[..8].try_into().unwrap());
            for (i, texel) in texels.iter_mut().enumerate() {
                texel[3] = ((alpha >> (4 * i)) & 0xF) as u8 * 17;
            }
            texels
        }
        Format::Bc3 => {
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
