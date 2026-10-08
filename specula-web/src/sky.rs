//! `SkyManager`'s cube: the map's sky set, which the water reflects, and
//! black until it is in, as the engine's black cube stands in while it
//! loads.

/// The cube's bind group layout and sampler, and the black stand-in.
pub struct Sky {
    pub layout: wgpu::BindGroupLayout,
    black: wgpu::BindGroup,
}

impl Sky {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // Linear and clamped, without mips, as SkyManager makes it.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sky"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let black = cube(device, queue, 1, &[&[0, 0, 0, 255]; 6]);
        let black = bind_group(device, &layout, &black, &sampler);
        Sky { layout, black }
    }

    /// The cube to bind: the map's once it is in, else black.
    pub fn group(&self) -> &wgpu::BindGroup {
        &self.black
    }
}

/// A cube of `side`-texel RGBA8 layers, +X, -X, +Y, -Y, +Z, -Z.
fn cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    side: u32,
    layers: &[&[u8]; 6],
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sky"),
        size: wgpu::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, pixels) in layers.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer as u32,
                },
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(side * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("sky"),
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sky"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
