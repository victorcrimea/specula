//! `SkyManager`'s cube: the map's sky set, drawn behind everything and
//! reflected by the water, and black until it is in, as the engine's
//! black cube stands in while it loads and for a map without a set.

use std::cell::RefCell;
use std::rc::Rc;

use specula_assets::files::TextureFormat;

use crate::assets::Get;
use crate::assets::Store;

/// sky.wgsl's `SkyView`.
const VIEW_SIZE: u64 = 64;

pub struct Sky {
    pub layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    black: wgpu::BindGroup,
    /// The set asked for, and its cube once its layers are in.
    set: Option<(String, Option<wgpu::BindGroup>)>,
    pipeline: wgpu::RenderPipeline,
    view_buffer: wgpu::Buffer,
    view_group: wgpu::BindGroup,
}

impl Sky {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
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

        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(VIEW_SIZE),
                },
                count: None,
            }],
        });
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky view"),
            size: VIEW_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky view"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sky.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky"),
            bind_group_layouts: &[Some(&view_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            // sky_simple.xml: drawn first, writing no depth, so everything
            // after covers it.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::unit::DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        Sky {
            layout,
            sampler,
            black,
            set: None,
            pipeline,
            view_buffer,
            view_group,
        }
    }

    /// The cube to bind: the map's once it is in, else black.
    pub fn group(&self) -> &wgpu::BindGroup {
        self.set
            .as_ref()
            .and_then(|(_, group)| group.as_ref())
            .unwrap_or(&self.black)
    }

    /// Asks for `name`'s layers, and makes its cube once all are in. An
    /// empty name is a map without a set, black as in the engine.
    pub fn refresh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        store: &Rc<RefCell<Store>>,
        name: &str,
    ) {
        if self.set.as_ref().is_none_or(|(set, _)| set != name) {
            self.set = Some((name.to_string(), None));
        }
        if name.is_empty() || self.set.as_ref().is_some_and(|(_, group)| group.is_some()) {
            return;
        }
        let files = match store.borrow().sky(name) {
            Get::Ready(files) => files,
            Get::Pending => return,
            Get::Missing => {
                crate::log(&format!("sky {name} is not in the asset bake"));
                // Asked for no more: it stays black.
                self.set = Some((name.to_string(), Some(self.black.clone())));
                return;
            }
        };
        let mut layers = Vec::with_capacity(files.len());
        for file in files {
            // One mip each, so the coarse file is the whole layer.
            match store.borrow_mut().texture(file, false) {
                Get::Ready(texture) => layers.push(texture),
                Get::Pending => return,
                Get::Missing => {
                    self.set = Some((name.to_string(), Some(self.black.clone())));
                    return;
                }
            }
        }
        let side = layers[0].width;
        if layers.iter().any(|layer| {
            layer.format != TextureFormat::Rgba8
                || (layer.width, layer.height) != (side, side)
                || layer.levels.is_empty()
        }) {
            crate::log(&format!(
                "sky {name}: its layers are not RGBA8 squares of one size"
            ));
            self.set = Some((name.to_string(), Some(self.black.clone())));
            return;
        }
        let pixels: [&[u8]; 6] = std::array::from_fn(|layer| layers[layer].levels[0].as_slice());
        let view = cube(device, queue, side, &pixels);
        let group = bind_group(device, &self.layout, &view, &self.sampler);
        self.set = Some((name.to_string(), Some(group)));
    }

    /// The camera's axes for this frame: `right` and `up` scaled to the
    /// screen's edges at a `forward` of length 1, and the sun's rotation.
    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        right: [f32; 3],
        up: [f32; 3],
        forward: [f32; 3],
        rotation: f32,
    ) {
        let (sin, cos) = (std::f32::consts::PI + rotation).sin_cos();
        let values = [
            right[0], right[1], right[2], 0.0, up[0], up[1], up[2], 0.0, forward[0], forward[1],
            forward[2], 0.0, cos, sin, 0.0, 0.0,
        ];
        let bytes: Vec<u8> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        queue.write_buffer(&self.view_buffer, 0, &bytes);
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.view_group, &[]);
        pass.set_bind_group(1, self.group(), &[]);
        pass.draw(0..3, 0..1);
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
