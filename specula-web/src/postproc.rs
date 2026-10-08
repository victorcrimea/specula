//! The observer page's map post-processing, as `CPostprocManager` runs it
//! with `postproc` on: the scene is drawn to a texture, blurred into three
//! levels, then the map's effect (`hdr`, or `DOF` then `hdr`) draws it to
//! the screen. The targets are 8-bit like the engine's, so every pass
//! clamps as there.

use crate::terrain::Light;
use crate::terrain::PostEffect;
use crate::unit::DEPTH_FORMAT;

/// The `Post` uniform of postproc.wgsl.
const UNIFORM_SIZE: u64 = 32;
/// Each blur level's divisor of the scene size, `m_BlurScales`.
const LEVEL_DIVISORS: [u32; 3] = [2, 4, 8];

/// The passes one frame runs.
#[derive(Clone, Copy)]
pub struct Plan {
    blur: bool,
    bloom: bool,
    dof: bool,
}

impl Plan {
    /// `None` when the scene goes straight to the screen: post-processing
    /// off, or the map's effect is "default", for which the engine skips
    /// it all.
    pub fn new(light: &Light, enabled: bool, bloom: bool, dof: bool) -> Option<Self> {
        if !enabled || light.effect == PostEffect::Default {
            return None;
        }
        let dof = dof && light.effect == PostEffect::Dof;
        Some(Plan {
            blur: bloom || dof,
            bloom,
            dof,
        })
    }

    /// Whether the scene pass must keep its depth for this plan.
    pub fn reads_depth(&self) -> bool {
        self.dof
    }
}

/// Everything sized by the canvas.
struct Targets {
    depth: wgpu::TextureView,
    scene: wgpu::TextureView,
    /// The level textures: each level's result, then its scratch.
    levels: [[wgpu::TextureView; 2]; 3],
    /// The scene, as the downscale passes read it.
    downscale_group: wgpu::BindGroup,
    /// Each level's result and scratch, as the blur passes read them.
    level_groups: [[wgpu::BindGroup; 2]; 3],
    /// The effect inputs with the scene, and with the DOF pass's output.
    scene_group: wgpu::BindGroup,
    dof_group: wgpu::BindGroup,
    ping: wgpu::TextureView,
}

pub struct Post {
    format: wgpu::TextureFormat,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    source_layout: wgpu::BindGroupLayout,
    effect_layout: wgpu::BindGroupLayout,
    downscale: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    dof: wgpu::RenderPipeline,
    /// Without and with the bloom term.
    hdr: [wgpu::RenderPipeline; 2],
    targets: Targets,
}

impl Post {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("postproc"),
            source: wgpu::ShaderSource::Wgsl(include_str!("postproc.wgsl").into()),
        });
        let texture_entry = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let source_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("postproc source"),
            entries: &[texture_entry(0, true), sampler_entry(1)],
        });
        let effect_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("postproc effect"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                texture_entry(1, true),
                texture_entry(2, true),
                texture_entry(3, true),
                texture_entry(4, true),
                // Depth read as unfilterable float, which WebGL2 can fetch
                // from where it cannot from a depth binding.
                texture_entry(5, false),
                sampler_entry(6),
            ],
        });
        let source_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("postproc blur"),
                bind_group_layouts: &[None, Some(&source_layout)],
                immediate_size: 0,
            });
        let effect_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("postproc effect"),
                bind_group_layouts: &[Some(&effect_layout)],
                immediate_size: 0,
            });
        let pipeline = |label, layout, entry_point, constants: &[(&str, f64)]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants,
                        ..Default::default()
                    },
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let downscale = pipeline(
            "postproc downscale",
            &source_pipeline_layout,
            "fs_downscale",
            &[],
        );
        let blur_h = pipeline("postproc blur", &source_pipeline_layout, "fs_blur_h", &[]);
        let blur_v = pipeline("postproc blur", &source_pipeline_layout, "fs_blur_v", &[]);
        let dof = pipeline("postproc dof", &effect_pipeline_layout, "fs_dof", &[]);
        let hdr = [0.0, 1.0].map(|bloom| {
            pipeline(
                "postproc hdr",
                &effect_pipeline_layout,
                "fs_hdr",
                &[("BLOOM", bloom)],
            )
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("postproc"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("postproc"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let targets = create_targets(
            device,
            format,
            &sampler,
            &uniform,
            &source_layout,
            &effect_layout,
            width,
            height,
        );
        Post {
            format,
            sampler,
            uniform,
            source_layout,
            effect_layout,
            downscale,
            blur_h,
            blur_v,
            dof,
            hdr,
            targets,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.targets = create_targets(
            device,
            self.format,
            &self.sampler,
            &self.uniform,
            &self.source_layout,
            &self.effect_layout,
            width,
            height,
        );
    }

    /// The scene pass's depth, readable by the DOF pass.
    pub fn depth(&self) -> &wgpu::TextureView {
        &self.targets.depth
    }

    /// Where the scene pass draws when post-processing runs.
    pub fn scene(&self) -> &wgpu::TextureView {
        &self.targets.scene
    }

    /// Writes the map's values; `near` and `far` are the view's planes.
    pub fn prepare(&self, queue: &wgpu::Queue, light: &Light, near: f32, far: f32) {
        let values = [
            light.brightness,
            light.contrast,
            light.saturation,
            light.bloom,
            near,
            far,
            0.0,
            0.0,
        ];
        let bytes: Vec<u8> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        queue.write_buffer(&self.uniform, 0, &bytes);
    }

    /// `ApplyPostproc` from the scene texture to `target`.
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, plan: Plan, target: &wgpu::TextureView) {
        let targets = &self.targets;
        if plan.blur {
            for (level, groups) in targets.levels.iter().zip(&targets.level_groups) {
                fullscreen(
                    encoder,
                    &level[0],
                    &self.downscale,
                    1,
                    &targets.downscale_group,
                );
                fullscreen(encoder, &level[1], &self.blur_h, 1, &groups[0]);
                fullscreen(encoder, &level[0], &self.blur_v, 1, &groups[1]);
            }
        }
        let mut group = &targets.scene_group;
        if plan.dof {
            fullscreen(encoder, &targets.ping, &self.dof, 0, group);
            group = &targets.dof_group;
        }
        fullscreen(
            encoder,
            target,
            &self.hdr[usize::from(plan.bloom)],
            0,
            group,
        );
    }
}

fn fullscreen(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    index: u32,
    group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("postproc"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(index, group, &[]);
    pass.draw(0..3, 0..1);
}

fn create_texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

#[allow(clippy::too_many_arguments)]
fn create_targets(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sampler: &wgpu::Sampler,
    uniform: &wgpu::Buffer,
    source_layout: &wgpu::BindGroupLayout,
    effect_layout: &wgpu::BindGroupLayout,
    width: u32,
    height: u32,
) -> Targets {
    let depth = create_texture(device, "postproc depth", DEPTH_FORMAT, width, height);
    let scene = create_texture(device, "postproc scene", format, width, height);
    let ping = create_texture(device, "postproc ping", format, width, height);
    let levels = LEVEL_DIVISORS.map(|divisor| {
        [0, 1].map(|_| {
            create_texture(
                device,
                "postproc blur",
                format,
                width / divisor,
                height / divisor,
            )
        })
    });
    let source_group = |view: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("postproc source"),
            layout: source_layout,
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
    };
    let effect_group = |rendered: &wgpu::TextureView| {
        let texture = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("postproc effect"),
            layout: effect_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                texture(1, rendered),
                texture(2, &levels[0][0]),
                texture(3, &levels[1][0]),
                texture(4, &levels[2][0]),
                texture(5, &depth),
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    };
    let downscale_group = source_group(&scene);
    let level_groups = [0, 1, 2].map(|level| [0, 1].map(|step| source_group(&levels[level][step])));
    let scene_group = effect_group(&scene);
    let dof_group = effect_group(&ping);
    Targets {
        depth,
        scene,
        levels,
        downscale_group,
        level_groups,
        scene_group,
        dof_group,
        ping,
    }
}
