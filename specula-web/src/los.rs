//! The engine's LOS texture, `CLOSTexture`, for the shown player: a texel
//! per LOS vertex, 255 where it sees, 127 where it has explored, 0 where
//! not, blurred by `GenerateBitmap`'s 7-tap filter, and SmoothLOS's fade
//! from the last texture to the new one over half a second
//! (`los_interp.fs`). The shaders darken by it as `getLOS` does
//! (`shared/los.wgsl`).

use crate::stream::LOS_EXPLORED;
use crate::stream::LOS_VISIBLE;

/// `g_BlurSize`'s binomial weights, which sum to 64.
const BLUR: [u32; 7] = [1, 6, 15, 20, 15, 6, 1];
/// `InterpolateLOS`: the fade takes this long, in ms.
const FADE_MS: f64 = 500.0;

/// `GenerateBitmap`: each vertex's texel, then the blur, horizontal and
/// vertical, each pass truncated to a byte and zero outside the grid, as
/// its padding is.
pub fn blur(grid: &[u8], side: usize) -> Vec<u8> {
    let texel = |state: u8| {
        if state & LOS_VISIBLE != 0 {
            255
        } else if state & LOS_EXPLORED != 0 {
            127
        } else {
            0
        }
    };
    let mut horizontal = vec![0u32; side * side];
    for j in 0..side {
        for i in 0..side {
            let sum: u32 = BLUR
                .iter()
                .enumerate()
                .filter_map(|(k, weight)| {
                    let x = (i + k).checked_sub(3).filter(|x| *x < side)?;
                    Some(weight * texel(grid[j * side + x]))
                })
                .sum();
            horizontal[j * side + i] = sum / 64;
        }
    }
    let mut out = vec![0u8; side * side];
    for j in 0..side {
        for i in 0..side {
            let sum: u32 = BLUR
                .iter()
                .enumerate()
                .filter_map(|(k, weight)| {
                    let z = (j + k).checked_sub(3).filter(|z| *z < side)?;
                    Some(weight * horizontal[z * side + i])
                })
                .sum();
            out[j * side + i] = (sum / 64) as u8;
        }
    }
    out
}

pub struct LosTexture {
    /// Texels per side; 0 when the view shows no LOS (an observer, or a
    /// stream without one), which leaves everything lit.
    side: u32,
    /// The texture as last recomputed, and what was shown when it was:
    /// the fade runs from that to this.
    now: Vec<u8>,
    before: Vec<u8>,
    /// When it was, on the frame clock.
    recomputed_at: f64,
    views: [wgpu::TextureView; 2],
    textures: [wgpu::Texture; 2],
    pub sampler: wgpu::Sampler,
}

impl LosTexture {
    pub fn new(device: &wgpu::Device) -> Self {
        let textures = [texture(device, 1), texture(device, 1)];
        LosTexture {
            side: 0,
            now: Vec::new(),
            before: Vec::new(),
            recomputed_at: 0.0,
            views: textures
                .each_ref()
                .map(|texture| texture.create_view(&Default::default())),
            textures,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("los"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    /// The texture now, then the one the fade starts from.
    pub fn views(&self) -> [&wgpu::TextureView; 2] {
        [&self.views[0], &self.views[1]]
    }

    /// `RecomputeTexture` from the shown player's LOS grid (`None`: no
    /// LOS); returns true when the textures were made anew, so the bind
    /// groups holding them must be too.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        grid: Option<(&[u8], u32)>,
        now_ms: f64,
    ) -> bool {
        let Some((grid, side)) =
            grid.filter(|(grid, side)| *side > 0 && grid.len() == *side as usize * *side as usize)
        else {
            self.side = 0;
            return false;
        };
        let texels = blur(grid, side as usize);
        let recreated = side != self.side || self.now.len() != texels.len();
        if recreated {
            // `RecomputeTexture` of a new texture fills both smooth ones.
            self.textures = [texture(device, side), texture(device, side)];
            self.views = self
                .textures
                .each_ref()
                .map(|texture| texture.create_view(&Default::default()));
            self.before = texels.clone();
        } else {
            // What the fade shows at this moment, as the 8-bit smooth
            // texture holds it, is where the next fade starts.
            let t = self.fade(now_ms);
            for (before, now) in self.before.iter_mut().zip(&self.now) {
                let shown = f64::from(*before) + (f64::from(*now) - f64::from(*before)) * t;
                *before = shown.round().clamp(0.0, 255.0) as u8;
            }
        }
        self.now = texels;
        self.side = side;
        self.recomputed_at = now_ms;
        for (texture, texels) in self.textures.iter().zip([&self.now, &self.before]) {
            queue.write_texture(
                texture.as_image_copy(),
                texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(side),
                    rows_per_image: Some(side),
                },
                wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: 1,
                },
            );
        }
        recreated
    }

    fn fade(&self, now_ms: f64) -> f64 {
        ((now_ms - self.recomputed_at) / FADE_MS).clamp(0.0, 1.0)
    }

    /// The light uniform's LOS pair: the fade, then texels per side (0 for
    /// none).
    pub fn uniform(&self, now_ms: f64) -> [f32; 2] {
        [self.fade(now_ms) as f32, self.side as f32]
    }
}

fn texture(device: &wgpu::Device, side: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("los"),
        size: wgpu::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}
