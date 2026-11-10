//! The engine's fancy water: the surface `CPatchRData::BuildWater` lays
//! over every tile near water, with `WaterManager`'s wind strength and
//! depth at each vertex, drawn with water.wgsl as
//! `TerrainRenderer::RenderFancyWater` draws it, animated through the
//! water type's 60 normal map frames, and with the fancy effects the
//! shore waves `WaterManager::CreateWaveMeshes` lays along the coasts.
//!
//! `WaterManager` works the wind and the shore distance out against the
//! water height it held before the map set its own (CCmpWaterManager's
//! `RecomputeWaterData` runs before it stores the new height), so in the
//! game they depend on the map played before. This takes the map's own.

use std::collections::HashMap;
use std::rc::Rc;

use specula_assets::files::FileRef;
use specula_assets::files::Texture;
use specula_assets::shore;
use specula_assets::wire::HEIGHT_UNITS_PER_METRE;

use crate::terrain::TILE_SIZE;
use crate::terrain::TerrainImage;

/// `WaterManager`'s m_NormalMap.
pub const FRAMES: usize = 60;
/// `GetCurrentTextureIndex(8.0)`: 60 frames over 8 s of sim time.
const FRAME_PERIOD: f64 = 8.0;
/// f32 x 3 position, f32 x 2 wind strength and depth.
pub const VERTEX_SIZE: usize = 20;
/// water.wgsl's `Water`.
const UNIFORM_SIZE: u64 = 288;
pub use specula_assets::shore::WAVE_VERTEX_SIZE;

/// The water surface's vertices and indices, and with the fancy effects
/// the shore tiles: the tiles partly over land, laid on the ground, which
/// the waves may run up.
pub struct Surface {
    pub vertices: Vec<u8>,
    pub indices: Vec<u32>,
    pub shore_vertices: Vec<u8>,
    pub shore_indices: Vec<u32>,
}

/// The engine's ground height in metres, clamped to the map as
/// `CTerrain::GetVertexGroundLevel` clamps.
fn ground(image: &TerrainImage, i: isize, j: isize) -> f32 {
    let side = image.size as isize + 1;
    let i = i.clamp(0, side - 1) as usize;
    let j = j.clamp(0, side - 1) as usize;
    f32::from(image.heights[j * side as usize + i]) / HEIGHT_UNITS_PER_METRE
}

/// `CPatchRData::BuildWater` over the whole map: a quad for every tile
/// with water at one of ten vertices around it, each vertex at the water
/// level, carrying its wind strength and its depth.
pub fn surface(image: &TerrainImage) -> Surface {
    let size = image.size as isize;
    let side = image.size as usize + 1;
    let level = image.water_level;
    let wind = image.wind_strength();
    // Where to look for water around a tile.
    const CHECK: [(isize, isize); 10] = [
        (0, 0),
        (1, 0),
        (2, 0),
        (0, 1),
        (0, 2),
        (1, 1),
        (2, 2),
        (-1, 0),
        (0, -1),
        (-1, -1),
    ];
    let mut index_of = vec![u32::MAX; side * side];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut shore_index_of = vec![u32::MAX; side * side];
    let mut shore_vertices = Vec::new();
    let mut shore_indices = Vec::new();
    for z in 0..size {
        for x in 0..size {
            if !CHECK
                .iter()
                .any(|(dx, dz)| ground(image, x + dx, z + dz) < level)
            {
                continue;
            }
            let mut corner = |dx: isize, dz: isize| {
                let (i, j) = ((x + dx) as usize, (z + dz) as usize);
                let at = j * side + i;
                if index_of[at] == u32::MAX {
                    index_of[at] = (vertices.len() / VERTEX_SIZE) as u32;
                    let depth = level - ground(image, i as isize, j as isize);
                    for value in [
                        i as f32 * TILE_SIZE,
                        level,
                        j as f32 * TILE_SIZE,
                        wind[at],
                        depth,
                    ] {
                        vertices.extend_from_slice(&value.to_le_bytes());
                    }
                }
                index_of[at]
            };
            let quad = [corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1)];
            indices.extend_from_slice(&[quad[2], quad[0], quad[1], quad[1], quad[3], quad[2]]);

            if [(0, 0), (1, 0), (0, 1), (1, 1)]
                .iter()
                .all(|(dx, dz)| ground(image, x + dx, z + dz) < level)
            {
                continue;
            }
            // Just over the ground, carrying a depth of -5 that
            // water_high reads as shore.
            let mut corner = |dx: isize, dz: isize| {
                let (i, j) = ((x + dx) as usize, (z + dz) as usize);
                let at = j * side + i;
                if shore_index_of[at] == u32::MAX {
                    shore_index_of[at] = (shore_vertices.len() / VERTEX_SIZE) as u32;
                    for value in [
                        i as f32 * TILE_SIZE,
                        ground(image, i as isize, j as isize) + 0.02,
                        j as f32 * TILE_SIZE,
                        0.0,
                        -5.0,
                    ] {
                        shore_vertices.extend_from_slice(&value.to_le_bytes());
                    }
                }
                shore_index_of[at]
            };
            let quad = [corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1)];
            if image.triangulation_dir(x as usize, z as usize) {
                shore_indices
                    .extend_from_slice(&[quad[2], quad[0], quad[1], quad[1], quad[3], quad[2]]);
            } else {
                shore_indices
                    .extend_from_slice(&[quad[3], quad[2], quad[0], quad[3], quad[0], quad[1]]);
            }
        }
    }
    Surface {
        vertices,
        indices,
        shore_vertices,
        shore_indices,
    }
}

/// `WaterManager::CreateWaveMeshes`' waves, as waves.wgsl draws them: a
/// seat's as the server sent them, else made from the whole heightmap.
pub fn waves(image: &TerrainImage) -> Surface {
    let (vertices, indices) = shore::wave_mesh(&image.waves());
    Surface {
        vertices,
        indices,
        shore_vertices: Vec::new(),
        shore_indices: Vec::new(),
    }
}

/// The waveParams uniforms `RenderFancyWater` sets by water type.
fn wave_params(kind: &str) -> [[f32; 4]; 2] {
    match kind {
        "clap" => [[30.0, 1.5, 20.0, 0.03], [0.5, 0.0, 0.0, 0.0]],
        "lake" => [[8.5, 1.5, 15.0, 0.03], [0.2, 0.0, 0.0, 0.07]],
        _ => [[15.0, 0.8, 10.0, 0.1], [0.3, 0.0, 0.1, 0.3]],
    }
}

/// `WaterManager::GetCurrentTextureIndex(8.0)` and the frame after it.
fn frames(time: f64) -> (usize, usize) {
    let current = (time * FRAMES as f64 / FRAME_PERIOD) as usize % FRAMES;
    (current, (current + 1) % FRAMES)
}

/// The camera the refraction is drawn from: the view's, as
/// `ComputeRefractionCamera` makes it, and the reflection's view-projection.
pub struct WaterCamera {
    /// Its view-projection, to project the water onto its texture.
    pub view_proj: [[f32; 4]; 4],
    /// The view's axes and where it is, and its projection's x and y
    /// scale, to rebuild a world position from the texture's depth.
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    pub scale: [f32; 2],
    pub near: f32,
    pub far: f32,
    /// `ComputeReflectionCamera`'s: the refraction's, of the scene
    /// mirrored in the water.
    pub reflection: [[f32; 4]; 4],
}

/// The uniform for water.wgsl's `Water`.
pub fn uniform(
    image: &TerrainImage,
    time: f64,
    sky_rotation: f32,
    refraction: &WaterCamera,
) -> Vec<u8> {
    let water = &image.water;
    let [wave1, wave2] = wave_params(&water.kind);
    let (current, next) = frames(time);
    let (wind_sin, wind_cos) = (-water.wind_angle).sin_cos();
    let (sky_sin, sky_cos) = (std::f32::consts::PI + sky_rotation).sin_cos();
    let [r, g, b] = water.color;
    let [tint_r, tint_g, tint_b] = water.tint;
    let values = [
        r,
        g,
        b,
        water.waviness,
        tint_r,
        tint_g,
        tint_b,
        water.murkiness,
        wave1[0],
        wave1[1],
        wave1[2],
        wave1[3],
        wave2[0],
        wave2[1],
        wave2[2],
        wave2[3],
        // The engine passes its double timer as a float too.
        time as f32,
        wind_cos,
        wind_sin,
        0.0,
        current as f32,
        next as f32,
        sky_cos,
        sky_sin,
    ];
    let mut bytes: Vec<u8> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    // WGSL matrices are column-major.
    for column in 0..4 {
        for row in &refraction.view_proj {
            bytes.extend_from_slice(&row[column].to_le_bytes());
        }
    }
    let [right_x, right_y, right_z] = refraction.right;
    let [up_x, up_y, up_z] = refraction.up;
    let [forward_x, forward_y, forward_z] = refraction.forward;
    for value in [
        right_x,
        right_y,
        right_z,
        refraction.scale[0],
        up_x,
        up_y,
        up_z,
        refraction.scale[1],
        forward_x,
        forward_y,
        forward_z,
        0.0,
        refraction.near,
        refraction.far,
        0.0,
        0.0,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for column in 0..4 {
        for row in &refraction.reflection {
            bytes.extend_from_slice(&row[column].to_le_bytes());
        }
    }
    bytes
}

/// A water type's frames as one array texture, uploaded as they arrive;
/// drawn only once all are in, as a missing frame would flash.
pub struct Normals {
    pub kind: String,
    pub files: Vec<FileRef>,
    texture: Option<wgpu::Texture>,
    /// The frames uploaded, by layer.
    uploaded: Vec<bool>,
    pub view: Option<wgpu::TextureView>,
}

impl Normals {
    pub fn new(kind: String, files: Vec<FileRef>) -> Self {
        let uploaded = vec![false; files.len()];
        Normals {
            kind,
            files,
            texture: None,
            uploaded,
            view: None,
        }
    }

    /// Uploads `frame`, whole as the texture quality has it; the first one
    /// in sets the array's size, format and mips, which the others must
    /// share.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: usize,
        texture: &Rc<Texture>,
        bc_supported: bool,
    ) -> Result<(), String> {
        if self.uploaded[frame] {
            return Ok(());
        }
        let format = crate::field::gpu_format(texture, bc_supported);
        let gpu = self.texture.get_or_insert_with(|| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("water normals"),
                size: wgpu::Extent3d {
                    width: texture.width,
                    height: texture.height,
                    // A spare layer: wgpu's GL backend makes a square
                    // array of a multiple of 6 layers a cube map array,
                    // which WebGL2 does not have.
                    depth_or_array_layers: FRAMES as u32 + 1,
                },
                mip_level_count: texture.levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let size = gpu.size();
        if (texture.width, texture.height) != (size.width, size.height)
            || texture.levels.len() as u32 != gpu.mip_level_count()
            || format != gpu.format()
        {
            return Err(format!(
                "water {} frame {} differs from the first",
                self.kind,
                frame + 1
            ));
        }
        crate::field::write_levels(queue, gpu, texture, 0, frame as u32, bc_supported);
        self.uploaded[frame] = true;
        if self.uploaded.iter().all(|done| *done) {
            self.view = self.texture.as_ref().map(|gpu| {
                gpu.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("water normals"),
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                })
            });
        }
        Ok(())
    }
}

/// Which of water_high's options a pipeline is compiled with, as the
/// game's water settings choose them.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Options {
    pub refraction: bool,
    pub real_depth: bool,
    pub reflection: bool,
    pub fancy: bool,
}

/// The textures the refraction and reflection passes draw into, as
/// `WaterManager` makes them: square, the screen's height rounded up to a
/// power of two.
pub struct Targets {
    pub side: u32,
    pub refraction: wgpu::TextureView,
    pub refraction_depth: wgpu::TextureView,
    pub reflection: wgpu::TextureView,
    pub reflection_depth: wgpu::TextureView,
}

impl Targets {
    /// Their side for a screen `screen_height` high.
    pub fn side(device: &wgpu::Device, screen_height: u32) -> u32 {
        screen_height
            .max(1)
            .next_power_of_two()
            .min(device.limits().max_texture_dimension_2d)
    }

    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, screen_height: u32) -> Self {
        let side = Self::side(device, screen_height);
        let target = |label: &str, format: wgpu::TextureFormat| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: side,
                        height: side,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Targets {
            side,
            refraction: target("water refraction", format),
            refraction_depth: target("water refraction depth", crate::unit::DEPTH_FORMAT),
            reflection: target("water reflection", format),
            reflection_depth: target("water reflection depth", crate::unit::DEPTH_FORMAT),
        }
    }
}

/// The water's pipelines, made as the options are first asked for, and
/// what they bind that does not change with the map.
pub struct Water {
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    pipelines: HashMap<Options, wgpu::RenderPipeline>,
    pub layout: wgpu::BindGroupLayout,
    pub environment_layout: wgpu::BindGroupLayout,
    pub buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    refraction_sampler: wgpu::Sampler,
    /// Bound for the refraction while it is not drawn, as the shader then
    /// reads none; never a target of the view's pass.
    stand_in: wgpu::TextureView,
}

impl Water {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        view_layout: &wgpu::BindGroupLayout,
        light_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("water.wgsl"),
                    include_str!("shared/los.wgsl"),
                    include_str!("shared/light.wgsl"),
                    include_str!("shared/fog.wgsl"),
                )
                .into(),
            ),
        });
        let texture_entry = |binding, view_dimension, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension,
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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                texture_entry(1, wgpu::TextureViewDimension::D2Array, true),
                sampler_entry(2),
            ],
        });
        // The refraction's depth is fetched as unfilterable float, which
        // WebGL2 can do with a depth texture, as the DOF pass does.
        let environment_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("water environment"),
                entries: &[
                    texture_entry(0, wgpu::TextureViewDimension::Cube, true),
                    sampler_entry(1),
                    texture_entry(2, wgpu::TextureViewDimension::D2, true),
                    texture_entry(3, wgpu::TextureViewDimension::D2, false),
                    sampler_entry(4),
                    texture_entry(5, wgpu::TextureViewDimension::D2, true),
                    texture_entry(6, wgpu::TextureViewDimension::D2, true),
                ],
            });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Repeating, mipmapped and anisotropic, as ReloadWaterNormalTextures
        // loads them.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water normals"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        // Linear and mirrored, as WaterManager makes the refraction and
        // reflection textures.
        let refraction_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water refraction"),
            address_mode_u: wgpu::AddressMode::MirrorRepeat,
            address_mode_v: wgpu::AddressMode::MirrorRepeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water"),
            bind_group_layouts: &[
                Some(view_layout),
                Some(&layout),
                Some(&environment_layout),
                Some(light_layout),
            ],
            immediate_size: 0,
        });
        let stand_in = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("water stand-in"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Water {
            shader,
            pipeline_layout,
            format,
            pipelines: HashMap::new(),
            layout,
            environment_layout,
            buffer,
            sampler,
            refraction_sampler,
            stand_in,
        }
    }

    /// Makes the pipeline for `options` if it is not yet; WebGL2 compiles
    /// each, so only those drawn are made.
    pub fn prepare(&mut self, device: &wgpu::Device, options: Options) {
        if self.pipelines.contains_key(&options) {
            return;
        }
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        let constants = [
            ("REFRACTION", flag(options.refraction)),
            ("REAL_DEPTH", flag(options.refraction && options.real_depth)),
            ("REFLECTION", flag(options.reflection)),
            ("FANCY", flag(options.fancy)),
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_water"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            // water_high.xml tests and writes depth.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::unit::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_water"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.insert(options, pipeline);
    }

    /// The pipeline `prepare` made for `options`.
    pub fn pipeline(&self, options: Options) -> Option<&wgpu::RenderPipeline> {
        self.pipelines.get(&options)
    }

    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        normals: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(normals),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// The sky cube, the refraction and reflection passes' textures and
    /// the fancy effects', while they are drawn.
    pub fn environment_group(
        &self,
        device: &wgpu::Device,
        sky: (&wgpu::TextureView, &wgpu::Sampler),
        targets: Option<&Targets>,
        effects: Option<&Effects>,
    ) -> wgpu::BindGroup {
        let effects = effects.map_or(&self.stand_in, |effects| &effects.color);
        let (refraction, refraction_depth, reflection) = match targets {
            Some(targets) => (
                &targets.refraction,
                &targets.refraction_depth,
                &targets.reflection,
            ),
            None => (&self.stand_in, &self.stand_in, &self.stand_in),
        };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water environment"),
            layout: &self.environment_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(sky.0),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sky.1),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(refraction),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(refraction_depth),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.refraction_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(reflection),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(effects),
                },
            ],
        })
    }
}

/// The fancy effects' textures, `WaterManager`'s m_FancyTexture and its
/// depth: the screen's size.
pub struct Effects {
    pub size: (u32, u32),
    pub color: wgpu::TextureView,
    pub depth: wgpu::TextureView,
}

impl Effects {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let target = |label: &str, format: wgpu::TextureFormat| {
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
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Effects {
            size: (width, height),
            color: target("water effects", wgpu::TextureFormat::Rgba8Unorm),
            depth: target("water effects depth", crate::unit::DEPTH_FORMAT),
        }
    }
}

/// The pipelines that draw the fancy effects: the shore waves
/// (water_waves.xml) and the shore tiles over them (the solid shader with
/// a depth test).
pub struct Waves {
    pub wave_pipeline: wgpu::RenderPipeline,
    pub occluder_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Waves {
    pub fn new(device: &wgpu::Device, view_layout: &wgpu::BindGroupLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water waves"),
            source: wgpu::ShaderSource::Wgsl(include_str!("waves.wgsl").into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water waves"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // Repeating, as LoadWaterTextures loads both.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water waves"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let wave_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water waves"),
            bind_group_layouts: &[Some(view_layout), Some(&layout)],
            immediate_size: 0,
        });
        let occluder_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water foam occluders"),
            bind_group_layouts: &[Some(view_layout)],
            immediate_size: 0,
        });
        // `<blend src="src_alpha" dst="one_minus_src_alpha"/>` blends the
        // alpha the same way, which the water reads.
        let blend = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let wave_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water waves"),
            layout: Some(&wave_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_wave"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: WAVE_VERTEX_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x3,
                        2 => Float32x3,
                        3 => Float32x3,
                        4 => Float32x2,
                        5 => Float32x2,
                        6 => Float32x2,
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            // water_waves.xml: a depth test that always passes, and writes,
            // so the shore tiles test against the waves.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::unit::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_wave"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState {
                        color: blend,
                        alpha: blend,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let occluder_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water foam occluders"),
            layout: Some(&occluder_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_occluder"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::unit::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_occluder"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        Waves {
            wave_pipeline,
            occluder_pipeline,
            layout,
            sampler,
        }
    }

    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        water: &wgpu::Buffer,
        wave: &wgpu::TextureView,
        foam: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water waves"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: water.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(wave),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(foam),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}
