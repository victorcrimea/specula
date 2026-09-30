//! Draws the terrain picture, then every entity on top as a screen-space
//! square, one instance per dot.

use web_sys::HtmlCanvasElement;

use crate::stream::Scene;
use crate::terrain::TerrainImage;

/// In CSS pixels; scaled by devicePixelRatio so dots look the same on any
/// screen.
const DOT_CSS_PIXELS: f32 = 3.0;
/// f32x2 from, f32x2 to, unorm8x4 colour.
const INSTANCE_SIZE: u64 = 20;
/// Map size, width, height, dot size, progress, circular, padded to 16
/// bytes.
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
    circular: bool,
    map_size: f32,
    device_pixel_ratio: f32,
}

impl Renderer {
    pub async fn new(canvas: HtmlCanvasElement, device_pixel_ratio: f32) -> Result<Self, String> {
        let (width, height) = (canvas.width(), canvas.height());
        // WebGPU where the browser has it, otherwise WebGL2, from one binary.
        let instance = wgpu::util::new_instance_with_webgpu_detection(
            wgpu::InstanceDescriptor::new_without_display_handle(),
        )
        .await;
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|error| format!("cannot create a surface: {error}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("no graphics adapter: {error}"))?;
        crate::log(&format!(
            "graphics backend: {:?}",
            adapter.get_info().backend
        ));
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                // WebGL2 limits keep the fallback working; the resolution
                // limits come from the adapter so large canvases still fit.
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("no graphics device: {error}"))?;

        let capabilities = surface.get_capabilities(&adapter);
        // Minimap colours are sRGB bytes already; an sRGB target would
        // encode them a second time and wash them out.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or("the surface supports no format")?;
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or("the surface is not supported by the adapter")?;
        config.format = format;
        surface.configure(&device, &config);

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
        let terrain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain"),
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
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        // Linear, so a zoomed-out map does not shimmer and tile edges blend
        // like the in-game minimap's.
        let terrain_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

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
            0.0,
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
            if self.count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..4, 0..self.count);
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
        let size = wgpu::Extent3d {
            width: image.size,
            height: image.size,
            depth_or_array_layers: 1,
        };
        // Not sRGB: the colours are sRGB bytes already, like the dots'.
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain"),
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
            &image.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.size * 4),
                rows_per_image: None,
            },
            size,
        );
        let view = texture.create_view(&Default::default());
        self.terrain = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain"),
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
        }));
        self.circular = image.circular;
    }

    /// Uploads the dots once per turn; frames in between only move them
    /// with the progress uniform.
    pub fn set_scene(&mut self, scene: &Scene) {
        self.map_size = scene.map_size as f32;
        // Nothing to place dots on before the first keyframe.
        if scene.map_size == 0 {
            self.count = 0;
            return;
        }
        self.instance_bytes.clear();
        for dot in scene.dots.values() {
            let [b, g, r, a] = dot.color.to_le_bytes();
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
