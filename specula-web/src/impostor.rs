//! Impostors for far units. At load the unit page draws the unit into an
//! atlas, from `DIRECTIONS` headings for up to `MAX_FRAMES` frames of each
//! animation, colour and model-space normal apart, and builds the atlas's
//! mip levels; a far unit is then one quad facing the camera, cut from the
//! cell nearest its heading and frame (impostor.wgsl).
//!
//! `Mips` and `cell_view` are shared with the observer page's impostors
//! (far.rs), which capture with their own pipeline.

use std::f64::consts::TAU;

use crate::unit;
use crate::unit::Part;
use crate::unit::Reach;
use crate::unit::ViewParams;

pub const DIRECTIONS: u32 = 16;
/// Cell side in texels; an impostor is used once the unit's bounding
/// sphere is smaller than this on screen, so cells are not magnified.
pub const CELL: u32 = 64;
const MAX_FRAMES: u32 = 20;
/// The camera pitch the cells are drawn from; the page's camera starts at
/// it, and other pitches show the unit slightly off.
pub const ELEVATION: f32 = 0.6;
/// Down to 4 texel cells; smaller ones would blur into their neighbours.
pub const LEVELS: u32 = 5;
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const INSTANCE_SIZE: u64 = unit::INSTANCE_SIZE;

pub struct Atlas {
    pub bind_group: wgpu::BindGroup,
    pub columns: u32,
    pub rows: u32,
    /// Frames drawn, evenly spread over the animation's `frames`.
    pub slots: u32,
    pub frames: u32,
}

impl Atlas {
    /// Columns and rows, which impostor instances carry.
    pub fn grid(&self) -> [f32; 2] {
        [self.columns as f32, self.rows as f32]
    }

    /// `frame` is the animation position in frames; `yaw` is the direction
    /// to the camera in the unit's own frame, as atan2(x, z).
    pub fn cell(&self, frame: f64, yaw: f64) -> u32 {
        let directions = f64::from(DIRECTIONS);
        let direction = (yaw / TAU * directions).round().rem_euclid(directions) as u32;
        let slots = f64::from(self.slots);
        let slot = (frame / f64::from(self.frames) * slots)
            .round()
            .rem_euclid(slots) as u32;
        slot * DIRECTIONS + direction
    }
}

/// What drawing the unit into the atlas needs from the page.
pub struct Scene<'a> {
    pub uvs: &'a wgpu::Buffer,
    pub indices: &'a wgpu::Buffer,
    pub parts: &'a [Part],
    pub reach: &'a Reach,
    pub vertex_count: u32,
    pub texture_width: u32,
}

pub struct Kit {
    capture_pipeline: wgpu::RenderPipeline,
    capture_layout: wgpu::BindGroupLayout,
    mips: Mips,
    pub pipeline: wgpu::RenderPipeline,
}

/// Builds an atlas's mip chain from its level 0 and binds it.
pub struct Mips {
    downsample_pipeline: wgpu::RenderPipeline,
    downsample_layout: wgpu::BindGroupLayout,
    pub atlas_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

/// The atlas grid for `cells` cells: columns and rows.
pub fn grid(device: &wgpu::Device, cells: u32) -> (u32, u32) {
    let columns = (device.limits().max_texture_dimension_2d / CELL).min(cells);
    (columns, cells.div_ceil(columns))
}

/// The view-projection, camera right and up, and eye of the cell looking
/// from heading `direction` of `DIRECTIONS` at `elevation`, orthographic
/// over a sphere of `radius` around `centre`, depth 0..1.
pub fn cell_view(
    direction: u32,
    elevation: f32,
    radius: f32,
    centre: [f32; 3],
) -> ([[f32; 4]; 4], [f32; 3], [f32; 3], [f32; 3]) {
    let yaw = std::f32::consts::TAU * direction as f32 / DIRECTIONS as f32;
    let back = [
        elevation.cos() * yaw.sin(),
        elevation.sin(),
        elevation.cos() * yaw.cos(),
    ];
    let eye = std::array::from_fn(|i| centre[i] + back[i] * 2.0 * radius);
    let (view, right, up) = unit::look_at(eye, centre);
    // The sphere lies 1 to 3 radii in front of the eye.
    let projection = [
        [1.0 / radius, 0.0, 0.0, 0.0],
        [0.0, 1.0 / radius, 0.0, 0.0],
        [0.0, 0.0, 0.5 / radius, -0.5],
        [0.0, 0.0, 0.0, 1.0],
    ];
    (unit::multiply(&projection, &view), right, up, eye)
}

impl Kit {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        unit_shader: &wgpu::ShaderModule,
        view_layout: &wgpu::BindGroupLayout,
        animation_layout: &wgpu::BindGroupLayout,
        part_layout: &wgpu::BindGroupLayout,
        depth_stencil: wgpu::DepthStencilState,
    ) -> Self {
        // The same view uniform as the page's, one per cell at a dynamic
        // offset, so all cells are drawn in one pass.
        let capture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("impostor capture view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(unit::VIEW_SIZE),
                },
                count: None,
            }],
        });
        let capture_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("impostor capture"),
                bind_group_layouts: &[
                    Some(&capture_layout),
                    Some(animation_layout),
                    Some(part_layout),
                ],
                immediate_size: 0,
            });
        let targets = [Some(FORMAT.into()), Some(FORMAT.into())];
        let capture_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("impostor capture"),
            layout: Some(&capture_pipeline_layout),
            vertex: wgpu::VertexState {
                module: unit_shader,
                entry_point: Some("vs_unit"),
                compilation_options: Default::default(),
                buffers: &unit::unit_vertex_buffers(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_stencil.clone()),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: unit_shader,
                entry_point: Some("fs_capture"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });

        let mips = Mips::new(device);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("impostor"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("impostor.wgsl"),
                    include_str!("shared/turn.wgsl"),
                )
                .into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("impostor"),
            bind_group_layouts: &[Some(view_layout), Some(&mips.atlas_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("impostor"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_impostor"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_SIZE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: Some(depth_stencil),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_impostor"),
                compilation_options: Default::default(),
                targets: &[Some(surface_format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        Kit {
            capture_pipeline,
            capture_layout,
            mips,
            pipeline,
        }
    }

    /// Draws one animation's atlas.
    pub fn bake(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        animation: &wgpu::BindGroup,
        frames: u32,
    ) -> Atlas {
        let slots = frames.clamp(1, MAX_FRAMES);
        let cells = slots * DIRECTIONS;
        let (columns, rows) = grid(device, cells);
        let (width, height) = (columns * CELL, rows * CELL);

        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = unit::VIEW_SIZE.next_multiple_of(alignment);
        let mut uniforms = vec![0u8; (stride * u64::from(cells)) as usize];
        let mut instances = Vec::with_capacity((INSTANCE_SIZE * u64::from(cells)) as usize);
        let radius = scene.reach.radius;
        let centre = [0.0, scene.reach.centre_y, 0.0];
        for cell in 0..cells {
            let (slot, direction) = (cell / DIRECTIONS, cell % DIRECTIONS);
            let (view_proj, right, up, eye) = cell_view(direction, ELEVATION, radius, centre);
            let params = ViewParams {
                view_proj,
                right,
                up,
                eye,
                vertex_count: scene.vertex_count,
                texture_width: scene.texture_width,
                reach: scene.reach,
            };
            let start = (stride * u64::from(cell)) as usize;
            let bytes = unit::view_bytes(&params);
            uniforms[start..start + bytes.len()].copy_from_slice(&bytes);

            let position = f64::from(slot) * f64::from(frames) / f64::from(slots);
            let a = position.floor() as u32 % frames.max(1);
            let b = (a + 1) % frames.max(1);
            // Unturned, at the origin.
            for value in [
                0.0,
                0.0,
                0.0,
                1.0,
                a as f32,
                b as f32,
                position.fract() as f32,
                0.0,
            ] {
                instances.extend_from_slice(&value.to_le_bytes());
            }
        }
        let uniform_buffer = unit::create_buffer(
            device,
            queue,
            "impostor capture views",
            &uniforms,
            wgpu::BufferUsages::UNIFORM,
        );
        let instance_buffer = unit::create_buffer(
            device,
            queue,
            "impostor capture instances",
            &instances,
            wgpu::BufferUsages::VERTEX,
        );
        let capture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("impostor capture view"),
            layout: &self.capture_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(unit::VIEW_SIZE),
                }),
            }],
        });

        let levels = Mips::scratch(device, width, height);
        let depth = unit::create_depth(device, width, height);

        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let albedo = levels[0][0].create_view(&Default::default());
            let normal = levels[0][1].create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("impostor capture"),
                color_attachments: &[Some(clear_target(&albedo)), Some(clear_target(&normal))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&self.capture_pipeline);
            pass.set_bind_group(1, animation, &[]);
            pass.set_vertex_buffer(0, scene.uvs.slice(..));
            pass.set_index_buffer(scene.indices.slice(..), wgpu::IndexFormat::Uint32);
            for cell in 0..cells {
                let (column, row) = (cell % columns, cell / columns);
                pass.set_viewport(
                    (column * CELL) as f32,
                    (row * CELL) as f32,
                    CELL as f32,
                    CELL as f32,
                    0.0,
                    1.0,
                );
                let offset = (stride * u64::from(cell)) as u32;
                pass.set_bind_group(0, &capture_bind_group, &[offset]);
                let start = INSTANCE_SIZE * u64::from(cell);
                pass.set_vertex_buffer(1, instance_buffer.slice(start..start + INSTANCE_SIZE));
                for part in scene.parts {
                    let (first, count) = part.levels[0];
                    pass.set_bind_group(2, &part.bind_group, &[]);
                    pass.draw_indexed(first..first + count, 0, 0..1);
                }
            }
        }
        let bind_group = self
            .mips
            .finish(device, &mut encoder, &levels, width, height, 0);
        queue.submit([encoder.finish()]);
        Atlas {
            bind_group,
            columns,
            rows,
            slots,
            frames: frames.max(1),
        }
    }
}

impl Mips {
    pub fn new(device: &wgpu::Device) -> Self {
        let targets = [Some(FORMAT.into()), Some(FORMAT.into())];
        let downsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("impostor downsample"),
            source: wgpu::ShaderSource::Wgsl(include_str!("downsample.wgsl").into()),
        });
        let source_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let downsample_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("impostor downsample"),
            entries: &[source_entry(0), source_entry(1)],
        });
        let downsample_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("impostor downsample"),
                bind_group_layouts: &[Some(&downsample_layout)],
                immediate_size: 0,
            });
        let downsample_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("impostor downsample"),
            layout: Some(&downsample_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &downsample_shader,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &downsample_shader,
                entry_point: Some("fs_downsample"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });

        let atlas_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("impostor atlas"),
            entries: &[
                atlas_entry(0),
                atlas_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // Clamped, so a cell on the atlas edge does not wrap round to the
        // opposite one.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("impostor atlas"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            lod_max_clamp: (LEVELS - 1) as f32,
            ..Default::default()
        });

        Mips {
            downsample_pipeline,
            downsample_layout,
            atlas_layout,
            sampler,
        }
    }

    /// Albedo and normal targets for every level, level 0 to draw into.
    pub fn scratch(device: &wgpu::Device, width: u32, height: u32) -> Vec<[wgpu::Texture; 2]> {
        let scratch = |level: u32| {
            let (width, height) = level_size(width, height, level);
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("impostor level"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        (0..LEVELS)
            .map(|level| [scratch(level), scratch(level)])
            .collect()
    }

    /// Halves level 0 of `levels` down the chain and copies every level
    /// into the atlas, the normal plane from `normal_level` on, which the
    /// same UVs sample at a lower resolution. Levels are drawn into
    /// textures of their own and then copied, because sampling one level
    /// of a texture while drawing into another trips WebGL2's feedback
    /// loop check.
    pub fn finish(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        levels: &[[wgpu::Texture; 2]],
        width: u32,
        height: u32,
        normal_level: u32,
    ) -> wgpu::BindGroup {
        for level in 1..LEVELS as usize {
            let source = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("impostor downsample"),
                layout: &self.downsample_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &levels[level - 1][0].create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &levels[level - 1][1].create_view(&Default::default()),
                        ),
                    },
                ],
            });
            let albedo = levels[level][0].create_view(&Default::default());
            let normal = levels[level][1].create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("impostor downsample"),
                color_attachments: &[Some(clear_target(&albedo)), Some(clear_target(&normal))],
                ..Default::default()
            });
            pass.set_pipeline(&self.downsample_pipeline);
            pass.set_bind_group(0, &source, &[]);
            pass.draw(0..3, 0..1);
        }

        let atlas = |label, first: u32| {
            let (width, height) = level_size(width, height, first);
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: LEVELS - first,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let firsts = [0, normal_level];
        let atlases = [
            atlas("impostor albedo", firsts[0]),
            atlas("impostor normal", firsts[1]),
        ];
        for (level, textures) in levels.iter().enumerate() {
            let level = level as u32;
            let (width, height) = level_size(width, height, level);
            for ((source, target), first) in textures.iter().zip(&atlases).zip(firsts) {
                if level < first {
                    continue;
                }
                encoder.copy_texture_to_texture(
                    source.as_image_copy(),
                    wgpu::TexelCopyTextureInfo {
                        texture: target,
                        mip_level: level - first,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        let albedo = atlases[0].create_view(&Default::default());
        let normal = atlases[1].create_view(&Default::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("impostor atlas"),
            layout: &self.atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&albedo),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&normal),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}

fn level_size(width: u32, height: u32, level: u32) -> (u32, u32) {
    ((width >> level).max(1), (height >> level).max(1))
}

/// Cleared to transparent: alpha 0 marks texels the unit does not cover.
pub fn clear_target(view: &wgpu::TextureView) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        },
    }
}
