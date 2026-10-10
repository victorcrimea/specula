//! The observer page's light: the map's sun and ambient colours, and a
//! shadow map from its sun as `ShadowMap.cpp` draws one with the engine's
//! defaults: a single cascade around the camera frustum up to
//! `shadowscutoffdistance`, filtered with `shadowpcf`.

use crate::terrain::Light;
use crate::unit::frustum_planes;
use crate::unit::multiply;

/// The `Light` uniform of field.wgsl, model.wgsl, decal.wgsl and far.wgsl:
/// sun colour, ambient colour, shadow matrix, eye, forward, shadow, fog
/// colour, fog.
const LIGHT_SIZE: u64 = 176;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// `DEFAULT_SHADOWS_CUTOFF_DISTANCE`, in metres along the view.
pub const DEFAULT_CUTOFF: f32 = 300.0;
/// The bounds of the game options' "Cutoff distance" slider.
pub const MIN_CUTOFF: f32 = 100.0;
pub const MAX_CUTOFF: f32 = 1500.0;
/// `CalculateBoundsForCascade`'s insets, in metres.
const INSETS: f32 = 0.2;
/// Above the highest ground, so the tallest building still falls inside
/// the light's depth range and casts.
const CASTER_HEIGHT: f32 = 60.0;
/// `CalculateShadowMatrices` keeps casters this far off the depth planes.
const DEPTH_MARGIN: f32 = 2.0;
/// The depth biases of `getShadow` and `getShadowOnLandscape`, in metres
/// rather than the engine's fractions of its tighter depth range.
const MODEL_BIAS: f32 = 0.3;
const TERRAIN_BIAS: f32 = 0.05;
/// Without it the ground's own slopes shade themselves in stripes where the
/// sun grazes them; polygon offset on WebGL2.
const SLOPE_BIAS: f32 = 2.0;

/// Shadow map sizes the page offers; 0 is off.
pub const SIZES: [u32; 4] = [0, 1024, 2048, 4096];
pub const DEFAULT_SIZE: u32 = 2048;

/// The camera as the fit needs it.
pub struct Eye {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub near: f32,
    pub field_of_view: f32,
    pub aspect: f32,
}

/// The map as the fit needs it: its side in metres and its lowest and
/// highest ground.
pub struct Bounds {
    pub size: f32,
    pub low: f32,
    pub high: f32,
}

pub struct Shadow {
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// The shadow pass's view uniform, laid out as the page's, with the
    /// light's matrix.
    view_buffer: wgpu::Buffer,
    view_bind_group: wgpu::BindGroup,
    /// Depth only, with vs_terrain and vs_model; the view at group 0
    /// is set by `begin`.
    pub terrain_pipeline: wgpu::RenderPipeline,
    pub model_pipeline: wgpu::RenderPipeline,
    /// The map, when shadows are on.
    target: Option<wgpu::TextureView>,
    size: u32,
    /// In metres along the view.
    cutoff: f32,
}

impl Shadow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        view_layout: &wgpu::BindGroupLayout,
        view_size: u64,
        pose_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        model_shader: &wgpu::ShaderModule,
        model_buffers: &[Option<wgpu::VertexBufferLayout>],
        field_shader: &wgpu::ShaderModule,
        terrain_buffers: &[Option<wgpu::VertexBufferLayout>],
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("light"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(LIGHT_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("light"),
            size: LIGHT_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Linear: each tap compares the 2x2 texels around it and blends
        // the results, which shadows_fragment.h's PCF builds on.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow view"),
            size: view_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow view"),
            layout: view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buffer.as_entire_binding(),
            }],
        });

        let depth_stencil = wgpu::DepthStencilState {
            format: FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState {
                constant: 0,
                slope_scale: SLOPE_BIAS,
                clamp: 0.0,
            },
        };
        let terrain_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain shadow"),
            bind_group_layouts: &[Some(view_layout)],
            immediate_size: 0,
        });
        let terrain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain shadow"),
            layout: Some(&terrain_layout),
            vertex: wgpu::VertexState {
                module: field_shader,
                entry_point: Some("vs_terrain"),
                compilation_options: Default::default(),
                buffers: terrain_buffers,
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_stencil.clone()),
            multisample: Default::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let model_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model shadow"),
            bind_group_layouts: &[Some(view_layout), Some(pose_layout), Some(material_layout)],
            immediate_size: 0,
        });
        let model_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("model shadow"),
            layout: Some(&model_layout),
            vertex: wgpu::VertexState {
                module: model_shader,
                entry_point: Some("vs_model"),
                compilation_options: Default::default(),
                buffers: model_buffers,
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_stencil),
            multisample: Default::default(),
            // The alpha test, so leaves cast leaf-shaped shadows.
            fragment: Some(wgpu::FragmentState {
                module: model_shader,
                entry_point: Some("fs_shadow"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            multiview_mask: None,
            cache: None,
        });

        let placeholder = create_map(device, 1);
        let bind_group = light_bind_group(device, &layout, &buffer, &placeholder, &sampler);
        Shadow {
            layout,
            bind_group,
            buffer,
            sampler,
            view_buffer,
            view_bind_group,
            terrain_pipeline,
            model_pipeline,
            target: None,
            size: 0,
            cutoff: DEFAULT_CUTOFF,
        }
    }

    /// 0 turns shadows off.
    pub fn set_size(&mut self, device: &wgpu::Device, size: u32) {
        let size = size.min(device.limits().max_texture_dimension_2d);
        if size == self.size {
            return;
        }
        self.size = size;
        let map = create_map(device, size.max(1));
        self.bind_group = light_bind_group(device, &self.layout, &self.buffer, &map, &self.sampler);
        self.target = (size > 0).then_some(map);
    }

    pub fn set_cutoff(&mut self, cutoff: f32) {
        self.cutoff = cutoff.clamp(MIN_CUTOFF, MAX_CUTOFF);
    }

    /// Fits the map to this frame's camera and writes the light uniform,
    /// with the map's fog when `fog`; the planes of the box casters must
    /// touch, `None` when there is no shadow to draw.
    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        light: &Light,
        fog: bool,
        eye: &Eye,
        bounds: Option<&Bounds>,
    ) -> Option<[[f32; 4]; 6]> {
        let fitted = match bounds {
            Some(bounds) if self.size > 0 => fit(light.sun, eye, bounds, self.size, self.cutoff),
            _ => None,
        };
        let mut uniform: Vec<f32> = Vec::with_capacity((LIGHT_SIZE / 4) as usize);
        uniform.extend_from_slice(&[
            light.sun_color[0],
            light.sun_color[1],
            light.sun_color[2],
            0.0,
        ]);
        uniform.extend_from_slice(&[
            light.ambient_color[0],
            light.ambient_color[1],
            light.ambient_color[2],
            0.0,
        ]);
        let (matrix, depth_range) = match &fitted {
            Some((view_proj, depth_range)) => (to_texels(view_proj, self.size), *depth_range),
            None => ([[0.0; 4]; 4], 1.0),
        };
        // WGSL matrices are column-major.
        for column in 0..4 {
            for row in &matrix {
                uniform.push(row[column]);
            }
        }
        let [x, y, z] = eye.position;
        uniform.extend_from_slice(&[x, y, z, self.cutoff]);
        let [x, y, z] = eye.forward;
        uniform.extend_from_slice(&[x, y, z, 1.0 / self.size.max(1) as f32]);
        uniform.extend_from_slice(&[
            MODEL_BIAS / depth_range,
            TERRAIN_BIAS / depth_range,
            if fitted.is_some() { 1.0 } else { 0.0 },
            0.0,
        ]);
        let [r, g, b] = light.fog_color;
        uniform.extend_from_slice(&[r, g, b, 0.0]);
        // CLightEnv::Bind: a max of 1 keeps every fragment clear.
        let fog_max = if fog { light.fog_max } else { 1.0 };
        uniform.extend_from_slice(&[light.fog_factor, fog_max, 0.0, 0.0]);
        let bytes: Vec<u8> = uniform
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        queue.write_buffer(&self.buffer, 0, &bytes);

        let (view_proj, _) = fitted?;
        // Only the matrix: the shadow pass's shaders read nothing else.
        let mut bytes = Vec::with_capacity(64);
        for column in 0..4 {
            for row in &view_proj {
                bytes.extend_from_slice(&row[column].to_le_bytes());
            }
        }
        queue.write_buffer(&self.view_buffer, 0, &bytes);
        Some(frustum_planes(&view_proj))
    }

    /// Starts the depth-only pass, cleared to "lit everywhere"; `None`
    /// when shadows are off.
    pub fn begin<'e>(&self, encoder: &'e mut wgpu::CommandEncoder) -> Option<wgpu::RenderPass<'e>> {
        let target = self.target.as_ref()?;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, &self.view_bind_group, &[]);
        Some(pass)
    }
}

fn create_map(device: &wgpu::Device, size: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn light_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    map: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("light"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(map),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// `ShadowMap::SetupFrame`, `CalculateBoundsForCascade` and
/// `CalculateShadowMatrices` for one cascade: the light's orthographic
/// view-projection, row-major, and its depth range in metres. The box is
/// the sphere around the camera frustum up to the cutoff, so it keeps its
/// size as the camera turns, and moves in whole texels, so shadows do not
/// shimmer as it pans. `None` when the camera is so high above the ground
/// that all of it lies past the cutoff, which the engine's camera never is.
fn fit(
    sun: [f32; 3],
    eye: &Eye,
    bounds: &Bounds,
    size: u32,
    cutoff: f32,
) -> Option<([[f32; 4]; 4], f32)> {
    let spread = (eye.field_of_view / 2.0).tan() * (1.0 + eye.aspect * eye.aspect).sqrt();
    // A point in view is at least its distance times the cosine of the
    // frustum's corner angle ahead of the eye.
    if eye.position[1] - bounds.high > cutoff * (1.0 + spread * spread).sqrt() {
        return None;
    }

    // SetupFrame: z along the light, x as near the world's up as it can.
    let z = sun.map(|value| -value);
    let mut x = sub([0.0, 1.0, 0.0], scale(z, z[1]));
    if length(x) < 0.001 {
        x = sub([1.0, 0.0, 0.0], scale(z, z[0]));
    }
    let x = normalize(x);
    let y = cross(z, x);

    // CalculateBoundsForCascade: the frustum is symmetric, so one corner
    // of the near and of the far quad gives the circumscribed sphere.
    let first_base = eye.near * spread;
    let second_base = cutoff * spread;
    let height = cutoff - eye.near;
    let to_centre =
        (height * height + second_base * second_base - first_base * first_base) * 0.5 / height;
    let centre: [f32; 3] =
        std::array::from_fn(|i| eye.position[i] + eye.forward[i] * (eye.near + to_centre));
    let half = to_centre.hypot(first_base) + INSETS;

    let texel = 2.0 * half / size as f32;
    let snap = |value: f32| (value / texel).round() * texel;
    let (cx, cy) = (snap(dot(x, centre)), snap(dot(y, centre)));

    // The whole map's depth: the box's own would clip casters between it
    // and the sun.
    let mut near = f32::MAX;
    let mut far = f32::MIN;
    for corner in 0..8 {
        let point = [
            if corner & 1 == 0 { 0.0 } else { bounds.size },
            if corner & 2 == 0 {
                bounds.low
            } else {
                bounds.high + CASTER_HEIGHT
            },
            if corner & 4 == 0 { 0.0 } else { bounds.size },
        ];
        let depth = dot(z, point);
        near = near.min(depth);
        far = far.max(depth);
    }
    near -= DEPTH_MARGIN;
    far += DEPTH_MARGIN;
    let range = far - near;

    let view_proj = [
        [x[0] / half, x[1] / half, x[2] / half, -cx / half],
        [y[0] / half, y[1] / half, y[2] / half, -cy / half],
        [z[0] / range, z[1] / range, z[2] / range, -near / range],
        [0.0, 0.0, 0.0, 1.0],
    ];
    Some((view_proj, range))
}

/// The light's clip space to the map's texels, x right and y down as
/// textures run, depth kept.
fn to_texels(view_proj: &[[f32; 4]; 4], size: u32) -> [[f32; 4]; 4] {
    let half = size as f32 / 2.0;
    let texels = [
        [half, 0.0, 0.0, half],
        [0.0, -half, 0.0, half],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    multiply(&texels, view_proj)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    a.map(|value| value * s)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    scale(a, 1.0 / length(a))
}
