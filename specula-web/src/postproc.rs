//! The observer page's map post-processing, as `CPostprocManager` runs it
//! with `postproc` on: the scene is drawn to a texture, blurred into three
//! levels, then the map's effect (`hdr`, or `DOF` then `hdr`), FXAA and
//! CAS run in turn, the last of them drawing to the screen. The targets
//! are 8-bit like the engine's, so every pass clamps as there.

use crate::terrain::Light;
use crate::terrain::PostEffect;
use crate::timing;
use crate::timing::Timer;
use crate::unit::DEPTH_FORMAT;

/// The `Post` uniform of postproc.wgsl.
const UNIFORM_SIZE: u64 = 32;
/// The `sharpness` config's default, CAS's strength.
pub const DEFAULT_SHARPNESS: f32 = 0.3;
/// Each blur level's divisor of the scene size, `m_BlurScales`.
const LEVEL_DIVISORS: [u32; 3] = [2, 4, 8];

/// `renderer.upscale.technique`: how a scene drawn below the screen's
/// size reaches it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Upscale {
    Fsr,
    Bilinear,
    Pixelated,
}

/// The passes one frame runs.
#[derive(Clone, Copy)]
pub struct Plan {
    /// The map's effect runs: it is not "default".
    effect: bool,
    blur: bool,
    bloom: bool,
    dof: bool,
    fxaa: bool,
    cas: bool,
}

impl Plan {
    /// `None` when the scene goes straight to the screen: post-processing
    /// off, or the map's effect is "default" with neither FXAA nor CAS
    /// on and the resolution scale 1, for which the engine skips it all.
    pub fn new(
        light: &Light,
        enabled: bool,
        bloom: bool,
        dof: bool,
        fxaa: bool,
        cas: bool,
        scale: f32,
    ) -> Option<Self> {
        let effect = light.effect != PostEffect::Default;
        if !enabled || !(effect || fxaa || cas || scale != 1.0) {
            return None;
        }
        // The engine leaves CAS out when upscaling; FSR's RCAS sharpens.
        let cas = cas && scale >= 1.0;
        let bloom = bloom && effect;
        let dof = dof && light.effect == PostEffect::Dof;
        Some(Plan {
            effect,
            blur: bloom || dof,
            bloom,
            dof,
            fxaa,
            cas,
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
    /// The scene, as the downscale and resampling passes read it.
    downscale_group: wgpu::BindGroup,
    /// Ping, as the resampling passes read it.
    ping_source_group: wgpu::BindGroup,
    /// The size of every full-size target, the scaled one.
    size: (u32, u32),
    /// Each level's result and scratch, as the blur passes read them.
    level_groups: [[wgpu::BindGroup; 2]; 3],
    /// The effect inputs with the scene, and with the ping texture, which
    /// the passes after the scene's take turns writing.
    scene_group: wgpu::BindGroup,
    ping_group: wgpu::BindGroup,
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
    fxaa: wgpu::RenderPipeline,
    cas: wgpu::RenderPipeline,
    nearest: wgpu::RenderPipeline,
    easu: wgpu::RenderPipeline,
    rcas: wgpu::RenderPipeline,
    targets: Targets,
    screen: (u32, u32),
    upscale: Upscale,
    /// EASU's output at the screen's size, for RCAS, when FSR upscales.
    easu_target: Option<(wgpu::TextureView, wgpu::BindGroup)>,
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
        let fxaa = pipeline("postproc fxaa", &effect_pipeline_layout, "fs_fxaa", &[]);
        let cas = pipeline("postproc cas", &effect_pipeline_layout, "fs_cas", &[]);
        let nearest = pipeline(
            "postproc nearest",
            &source_pipeline_layout,
            "fs_nearest",
            &[],
        );
        let easu = pipeline("postproc easu", &source_pipeline_layout, "fs_easu", &[]);
        let rcas = pipeline("postproc rcas", &source_pipeline_layout, "fs_rcas", &[]);
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
            fxaa,
            cas,
            nearest,
            easu,
            rcas,
            targets,
            screen: (width, height),
            upscale: Upscale::Fsr,
            easu_target: None,
        }
    }

    /// `screen` is the canvas's size, `scaled` the scene's at the
    /// resolution scale.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        screen: (u32, u32),
        scaled: (u32, u32),
        upscale: Upscale,
    ) {
        if self.targets.size != scaled {
            self.targets = create_targets(
                device,
                self.format,
                &self.sampler,
                &self.uniform,
                &self.source_layout,
                &self.effect_layout,
                scaled.0,
                scaled.1,
            );
        }
        let fsr = upscale == Upscale::Fsr && self.upscaling_from(scaled, screen);
        if self.screen != screen || !fsr {
            self.easu_target = None;
        }
        if fsr && self.easu_target.is_none() {
            let view = create_texture(device, "postproc easu", self.format, screen.0, screen.1);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("postproc easu"),
                layout: &self.source_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.easu_target = Some((view, group));
        }
        self.screen = screen;
        self.upscale = upscale;
    }

    /// `ShouldUpscale`.
    fn upscaling_from(&self, scaled: (u32, u32), screen: (u32, u32)) -> bool {
        scaled.0 < screen.0 || scaled.1 < screen.1
    }

    /// The scene pass's depth, readable by the DOF pass.
    pub fn depth(&self) -> &wgpu::TextureView {
        &self.targets.depth
    }

    /// Where the scene pass draws when post-processing runs.
    pub fn scene(&self) -> &wgpu::TextureView {
        &self.targets.scene
    }

    /// Writes the map's values and the viewer's sharpness; `near` and
    /// `far` are the view's planes.
    pub fn prepare(&self, queue: &wgpu::Queue, light: &Light, sharpness: f32, near: f32, far: f32) {
        let values = [
            light.brightness,
            light.contrast,
            light.saturation,
            light.bloom,
            near,
            far,
            sharpness,
            0.0,
        ];
        let bytes: Vec<u8> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        queue.write_buffer(&self.uniform, 0, &bytes);
    }

    /// `ApplyPostproc` from the scene texture to `target`.
    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        plan: Plan,
        target: &wgpu::TextureView,
        timer: &Timer,
    ) {
        let targets = &self.targets;
        // Gathered first so the timer knows the first and the last pass.
        let mut runs = Vec::new();
        if plan.blur {
            for (level, groups) in targets.levels.iter().zip(&targets.level_groups) {
                runs.push((&level[0], &self.downscale, 1, &targets.downscale_group));
                runs.push((&level[1], &self.blur_h, 1, &groups[0]));
                runs.push((&level[0], &self.blur_v, 1, &groups[1]));
            }
        }
        // The image so far is in the scene texture or in ping; each pass
        // writes the other, as the engine's m_WhichBuffer swaps, and the
        // last one the screen.
        let mut in_scene = true;
        if plan.dof {
            runs.push((&targets.ping, &self.dof, 0, &targets.scene_group));
            in_scene = false;
        }
        let passes = [
            (plan.effect, &self.hdr[usize::from(plan.bloom)]),
            (plan.fxaa, &self.fxaa),
            (plan.cas, &self.cas),
        ];
        let mut left = passes.iter().filter(|(on, _)| *on).count();
        // Off the screen's size, or with no pass left to draw to it, the
        // image is resampled into it, as BlitOutputFramebuffer does.
        let resample = targets.size != self.screen || left == 0;
        for (_, pipeline) in passes.iter().filter(|(on, _)| *on) {
            left -= 1;
            let (group, next) = if in_scene {
                (&targets.scene_group, &targets.ping)
            } else {
                (&targets.ping_group, &targets.scene)
            };
            let output = if left == 0 && !resample { target } else { next };
            runs.push((output, *pipeline, 0, group));
            in_scene = !in_scene;
        }
        if resample {
            let group = if in_scene {
                &targets.downscale_group
            } else {
                &targets.ping_source_group
            };
            let upscaling = self.upscaling_from(targets.size, self.screen);
            match (upscaling, self.upscale, &self.easu_target) {
                (true, Upscale::Fsr, Some((easu, easu_group))) => {
                    runs.push((easu, &self.easu, 1, group));
                    runs.push((target, &self.rcas, 1, easu_group));
                }
                (true, Upscale::Pixelated, _) => runs.push((target, &self.nearest, 1, group)),
                // Bilinear, and compute_downscale's one tap per pixel.
                _ => runs.push((target, &self.downscale, 1, group)),
            }
        }
        let last = runs.len().saturating_sub(1);
        for (run, (output, pipeline, index, group)) in runs.into_iter().enumerate() {
            let writes = timer.writes(timing::POST, run == 0, run == last);
            fullscreen(encoder, output, pipeline, index, group, writes);
        }
    }
}

fn fullscreen(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    index: u32,
    group: &wgpu::BindGroup,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites>,
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
        timestamp_writes,
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
    let ping_group = effect_group(&ping);
    let ping_source_group = source_group(&ping);
    Targets {
        depth,
        scene,
        levels,
        downscale_group,
        ping_source_group,
        size: (width, height),
        level_groups,
        scene_group,
        ping_group,
        ping,
    }
}
