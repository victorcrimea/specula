//! Draws the terrain picture and the territory over it, then every entity
//! on top as a screen-space square, one instance per dot, then the minimap
//! icons over the dots.

use std::collections::HashMap;

use web_sys::HtmlCanvasElement;

use crate::gpu;
use crate::gpu::Gpu;
use crate::icons::IconAtlas;
use crate::stream::Dot;
use crate::stream::Scene;
use crate::stream::Territory;
use crate::terrain::TerrainImage;
use crate::territory;

/// In CSS pixels; scaled by devicePixelRatio so dots look the same on any
/// screen.
const DOT_CSS_PIXELS: f32 = 3.0;
/// f32x2 from, f32x2 to, unorm8x4 colour.
const INSTANCE_SIZE: u64 = 20;
/// f32x2 from, f32x2 to, f32x4 atlas rect, f32 size, unorm8x4 colour.
const ICON_INSTANCE_SIZE: u64 = 40;
/// f32x2 from, f32x2 to of one outline segment, in metres.
const OUTLINE_INSTANCE_SIZE: u64 = 16;
/// Map size, width, height, dot size, progress, circular, pixel ratio,
/// padded to 16 bytes.
const VIEW_SIZE: u64 = 32;
const BACKGROUND: wgpu::Color = wgpu::Color {
    r: 0.06,
    g: 0.07,
    b: 0.08,
    a: 1.0,
};
/// The stream sends positions in 1/16 m; the shader works in metres.
const POSITION_UNITS_PER_METRE: f32 = 16.0;

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    view_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    instance_bytes: Vec<u8>,
    count: u32,
    terrain_pipeline: wgpu::RenderPipeline,
    terrain_layout: wgpu::BindGroupLayout,
    terrain_sampler: wgpu::Sampler,
    terrain: Option<wgpu::BindGroup>,
    /// The terrain pipeline blending, for the territory over the terrain.
    territory_pipeline: wgpu::RenderPipeline,
    territory: Option<wgpu::BindGroup>,
    /// The territory generation and palette `territory` was made from.
    territory_made_from: Option<(u32, Vec<u32>)>,
    icon_pipeline: wgpu::RenderPipeline,
    icon_sampler: wgpu::Sampler,
    atlas: Option<wgpu::BindGroup>,
    icon_rects: HashMap<String, [f32; 4]>,
    icon_instances: wgpu::Buffer,
    icon_count: u32,
    outline_pipeline: wgpu::RenderPipeline,
    outline_instances: wgpu::Buffer,
    outline_count: u32,
    circular: bool,
    map_size: f32,
    device_pixel_ratio: f32,
}

impl Renderer {
    pub async fn new(canvas: HtmlCanvasElement, device_pixel_ratio: f32) -> Result<Self, String> {
        let Gpu {
            surface,
            device,
            queue,
            config,
        } = gpu::init(canvas).await?;
        let format = config.format;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dots"),
            source: wgpu::ShaderSource::Wgsl(include_str!("dots.wgsl").into()),
        });
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view"),
            size: VIEW_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                // The terrain's circular mask is decided per fragment.
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dots"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dots"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_SIZE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32x2,
                        2 => Unorm8x4,
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let instances = create_instance_buffer(&device, INSTANCE_SIZE);

        let terrain_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
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
        let terrain_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain"),
            source: wgpu::ShaderSource::Wgsl(include_str!("terrain.wgsl").into()),
        });
        let terrain_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("terrain"),
                bind_group_layouts: &[Some(&bind_group_layout), Some(&terrain_layout)],
                immediate_size: 0,
            });
        let picture_pipeline = |label: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&terrain_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &terrain_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &terrain_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let terrain_pipeline = picture_pipeline("terrain", None);
        // CMiniMapTexture::RenderFinalTexture: the territory goes over the
        // terrain, under the entities.
        let territory_pipeline =
            picture_pipeline("territory", Some(wgpu::BlendState::ALPHA_BLENDING));
        // Linear, so a zoomed-out map does not shimmer and tile edges blend
        // like the in-game minimap's.
        let terrain_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let icon_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("icons"),
            source: wgpu::ShaderSource::Wgsl(include_str!("icons.wgsl").into()),
        });
        // The atlas binds like the terrain picture: one texture, one sampler.
        let icon_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("icons"),
            bind_group_layouts: &[Some(&bind_group_layout), Some(&terrain_layout)],
            immediate_size: 0,
        });
        let icon_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("icons"),
            layout: Some(&icon_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &icon_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: ICON_INSTANCE_SIZE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32x2,
                        2 => Float32x4,
                        3 => Float32,
                        4 => Unorm8x4,
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &icon_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                // The icons are shapes on a transparent background.
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        // Nearest keeps the 16 px pixel art crisp at whole pixel ratios.
        let icon_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("icons"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let icon_instances = create_instance_buffer(&device, ICON_INSTANCE_SIZE);

        let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("outline"),
            source: wgpu::ShaderSource::Wgsl(include_str!("outline.wgsl").into()),
        });
        let outline_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("outline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &outline_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: OUTLINE_INSTANCE_SIZE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32x2,
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &outline_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let outline_instances = create_instance_buffer(&device, OUTLINE_INSTANCE_SIZE);

        Ok(Renderer {
            surface,
            device,
            queue,
            config,
            pipeline,
            view_buffer,
            bind_group,
            instances,
            instance_bytes: Vec::new(),
            count: 0,
            terrain_pipeline,
            terrain_layout,
            terrain_sampler,
            terrain: None,
            territory_pipeline,
            territory: None,
            territory_made_from: None,
            icon_pipeline,
            icon_sampler,
            atlas: None,
            icon_rects: HashMap::new(),
            icon_instances,
            icon_count: 0,
            outline_pipeline,
            outline_instances,
            outline_count: 0,
            circular: false,
            map_size: 0.0,
            device_pixel_ratio,
        })
    }

    pub fn max_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    pub fn resize(&mut self, width: u32, height: u32, device_pixel_ratio: f32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.device_pixel_ratio = device_pixel_ratio;
        self.surface.configure(&self.device, &self.config);
    }

    /// Draws the dots `progress` (0..=1) of the way from their `from` to
    /// their turn position. Returns false when no frame could be drawn, so
    /// the caller retries.
    pub fn draw(&mut self, progress: f32) -> bool {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            _ => return false,
        };

        let view = [
            self.map_size,
            self.config.width as f32,
            self.config.height as f32,
            DOT_CSS_PIXELS * self.device_pixel_ratio,
            progress,
            if self.circular { 1.0 } else { 0.0 },
            self.device_pixel_ratio,
            0.0,
        ];
        let view: Vec<u8> = view.iter().flat_map(|value| value.to_le_bytes()).collect();
        self.queue.write_buffer(&self.view_buffer, 0, &view);

        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("dots"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(BACKGROUND),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if let Some(terrain) = &self.terrain {
                pass.set_pipeline(&self.terrain_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_bind_group(1, terrain, &[]);
                pass.draw(0..4, 0..1);
            }
            if let Some(territory) = &self.territory {
                pass.set_pipeline(&self.territory_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_bind_group(1, territory, &[]);
                pass.draw(0..4, 0..1);
            }
            if self.count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..4, 0..self.count);
            }
            if let Some(atlas) = &self.atlas
                && self.icon_count > 0
            {
                pass.set_pipeline(&self.icon_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_bind_group(1, atlas, &[]);
                pass.set_vertex_buffer(0, self.icon_instances.slice(..));
                pass.draw(0..4, 0..self.icon_count);
            }
            if self.outline_count > 0 {
                pass.set_pipeline(&self.outline_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.outline_instances.slice(..));
                pass.draw(0..4, 0..self.outline_count);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        true
    }

    /// `None` goes back to a plain background, for a new match whose
    /// terrain has not arrived yet.
    pub fn set_terrain(&mut self, image: Option<&TerrainImage>) {
        let Some(image) = image else {
            self.terrain = None;
            return;
        };
        self.terrain = Some(self.picture("terrain", image.size, image.size, &image.pixels));
        self.circular = image.circular;
    }

    /// A map-sized RGBA8 picture, row 0 at z = 0, bound for terrain.wgsl.
    fn picture(&self, label: &str, width: u32, height: u32, pixels: &[u8]) -> wgpu::BindGroup {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        // Not sRGB: the colours are sRGB bytes already, like the dots'.
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
            },
            size,
        );
        let view = texture.create_view(&Default::default());
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.terrain_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.terrain_sampler),
                },
            ],
        })
    }

    /// Paints the territory again if it or the colours changed.
    fn set_territory(&mut self, territory: &Territory, generation: u32, palette: &[u32]) {
        if self
            .territory_made_from
            .as_ref()
            .is_some_and(|(made, colors)| *made == generation && colors == palette)
        {
            return;
        }
        self.territory_made_from = Some((generation, palette.to_vec()));
        self.territory = (territory.width > 0 && territory.height > 0).then(|| {
            let bitmap = territory::minimap_bitmap(territory, palette);
            self.picture("territory", territory.width, territory.height, &bitmap)
        });
    }

    /// The caller rebuilds the scene afterwards, so icons already in it
    /// appear.
    pub fn set_icon_atlas(&mut self, atlas: IconAtlas) {
        let size = wgpu::Extent3d {
            width: atlas.width,
            height: atlas.height,
            depth_or_array_layers: 1,
        };
        // Not sRGB: the tint is an sRGB byte colour, like the dots'.
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("icons"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            &atlas.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas.width * 4),
                rows_per_image: None,
            },
            size,
        );
        let view = texture.create_view(&Default::default());
        self.atlas = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("icons"),
            layout: &self.terrain_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.icon_sampler),
                },
            ],
        }));
        self.icon_rects = atlas.rects;
    }

    /// The segments, from and to in metres, drawn over everything else;
    /// none hides the outline.
    pub fn set_outline(&mut self, segments: &[[f32; 4]]) {
        let bytes: Vec<u8> = segments
            .iter()
            .flatten()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let needed = bytes.len() as u64;
        if needed > self.outline_instances.size() {
            self.outline_instances =
                create_instance_buffer(&self.device, needed.next_power_of_two());
        }
        self.queue.write_buffer(&self.outline_instances, 0, &bytes);
        self.outline_count = segments.len() as u32;
    }

    /// Uploads the dots once per turn; frames in between only move them
    /// with the progress uniform. `palette` is the colour of each player id.
    pub fn set_scene(&mut self, scene: &Scene, palette: &[u32]) {
        self.map_size = scene.map_size as f32;
        self.set_territory(&scene.territory, scene.territory_generation, palette);
        // Nothing to place dots on before the first keyframe.
        if scene.map_size == 0 {
            self.count = 0;
            self.icon_count = 0;
            return;
        }
        self.instance_bytes.clear();
        for dot in scene.dots.values() {
            let [b, g, r, a] = dot_color(dot, palette).to_le_bytes();
            let position = [dot.from_x, dot.from_z, dot.x as f32, dot.z as f32];
            for value in position {
                let metres = value / POSITION_UNITS_PER_METRE;
                self.instance_bytes.extend_from_slice(&metres.to_le_bytes());
            }
            self.instance_bytes.extend_from_slice(&[r, g, b, a]);
        }
        let needed = self.instance_bytes.len() as u64;
        if needed > self.instances.size() {
            // Doubling keeps reallocations rare while a match grows.
            self.instances = create_instance_buffer(&self.device, needed.next_power_of_two());
        }
        self.queue
            .write_buffer(&self.instances, 0, &self.instance_bytes);
        self.count = scene.dots.len() as u32;

        self.instance_bytes.clear();
        let mut icon_count = 0;
        for dot in scene.dots.values() {
            let Some(icon) = (dot.icon as usize)
                .checked_sub(1)
                .and_then(|index| scene.icons.get(index))
            else {
                continue;
            };
            // An icon the atlas lacks (from a mod) leaves just the dot.
            let Some(rect) = self.icon_rects.get(&icon.name) else {
                continue;
            };
            let [b, g, r, a] = dot_color(dot, palette).to_le_bytes();
            let position = [dot.from_x, dot.from_z, dot.x as f32, dot.z as f32];
            for value in position {
                let metres = value / POSITION_UNITS_PER_METRE;
                self.instance_bytes.extend_from_slice(&metres.to_le_bytes());
            }
            for value in rect.iter().chain([&icon.size]) {
                self.instance_bytes.extend_from_slice(&value.to_le_bytes());
            }
            self.instance_bytes.extend_from_slice(&[r, g, b, a]);
            icon_count += 1;
        }
        let needed = self.instance_bytes.len() as u64;
        if needed > self.icon_instances.size() {
            self.icon_instances = create_instance_buffer(&self.device, needed.next_power_of_two());
        }
        self.queue
            .write_buffer(&self.icon_instances, 0, &self.instance_bytes);
        self.icon_count = icon_count;
    }
}

/// Icons take it too: the engine tints them with the minimap colour.
fn dot_color(dot: &Dot, palette: &[u32]) -> u32 {
    match dot.player {
        0 => dot.color,
        player => palette.get(player as usize).copied().unwrap_or(dot.color),
    }
}

fn create_instance_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dots"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
