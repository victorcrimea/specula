//! The engine's fancy water: the surface `CPatchRData::BuildWater` lays
//! over every tile near water, with `WaterManager`'s wind strength and
//! depth at each vertex, drawn with water.wgsl as
//! `TerrainRenderer::RenderFancyWater` draws it, animated through the
//! water type's 60 normal map frames.
//!
//! `WaterManager` works the wind and the shore distance out against the
//! water height it held before the map set its own (CCmpWaterManager's
//! `RecomputeWaterData` runs before it stores the new height), so in the
//! game they depend on the map played before. This takes the map's own.

use std::rc::Rc;

use specula_assets::files::FileRef;
use specula_assets::files::Texture;

use crate::terrain::TILE_SIZE;
use crate::terrain::TerrainImage;

/// Engine heights are u16 in these units.
const HEIGHT_UNITS_PER_METRE: f32 = 92.0;
/// `WaterManager`'s m_NormalMap.
pub const FRAMES: usize = 60;
/// `GetCurrentTextureIndex(8.0)`: 60 frames over 8 s of sim time.
const FRAME_PERIOD: f64 = 8.0;
/// f32 x 3 position, f32 x 2 wind strength and depth.
pub const VERTEX_SIZE: usize = 20;
/// water.wgsl's `Water`.
const UNIFORM_SIZE: u64 = 96;

/// The water surface's vertices and indices.
pub struct Surface {
    pub vertices: Vec<u8>,
    pub indices: Vec<u32>,
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
    let wind = wind_strength(image);
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
        }
    }
    Surface { vertices, indices }
}

/// `WaterManager::RecomputeWindStrength`: wind marched across the map
/// from the edges it blows in from, slowed by rising ground and sped up
/// over falling ground, drifting back to 1 in between.
fn wind_strength(image: &TerrainImage) -> Vec<f32> {
    let side = image.size as usize + 1;
    let level = image.water_level;
    let mut strength = vec![0.0; side * side];
    let angle = image.water.wind_angle;
    let (dir_x, dir_y) = (angle.cos(), angle.sin());
    let step_size = 10.0;
    let mut wind_x = -(step_size * dir_x).round() as isize;
    let mut wind_y = -(step_size * dir_y).round() as isize;
    let mut starts: Vec<(isize, isize)> = Vec::new();
    let mut movement: Vec<(isize, isize)> = Vec::new();
    let last = side as isize - 1;
    if dir_x.abs() < 0.01 {
        movement.push((0, if wind_y > 0 { 1 } else { -1 }));
        let start = if wind_y > 0 { 0 } else { last };
        starts.extend((0..side as isize).map(|x| (x, start)));
    } else if dir_y.abs() < 0.01 {
        movement.push((if wind_x > 0 { 1 } else { -1 }, 0));
        let start = if wind_x > 0 { 0 } else { last };
        starts.extend((0..side as isize).map(|z| (start, z)));
    } else {
        let start = if wind_y > 0 { 0 } else { last };
        starts.extend((0..side as isize).map(|x| (x, start)));
        // Without repeating the corner.
        let start = if wind_x > 0 { 0 } else { last };
        if wind_y > 0 {
            starts.extend((1..side as isize).map(|z| (start, z)));
        } else {
            starts.extend((0..last).map(|z| (start, z)));
        }
        while wind_x != 0 || wind_y != 0 {
            let step = (wind_x.signum(), wind_y.signum());
            wind_x -= step.0;
            wind_y -= step.1;
            movement.push(step);
        }
    }
    for (mut x, mut y) in starts {
        let at = |x: isize, y: isize| y as usize * side + x as usize;
        // Starting at 1 unless in shallow water.
        let depth = level - ground(image, x, y);
        let mut wind = if depth > 0.0 && depth < 2.0 {
            depth / 2.0
        } else {
            1.0
        };
        strength[at(x, y)] = wind;
        'march: loop {
            for &(step_x, step_y) in &movement {
                wind = 0.15 + wind * 0.85;
                let height_diff = ground(image, x + step_x, y + step_y).max(level)
                    - ground(image, x, y).max(level);
                wind = if height_diff > 0.0 {
                    (wind + height_diff.min(4.0) / 40.0).min(2.0)
                } else {
                    (wind + height_diff.max(-4.0) / 5.0).max(0.0)
                };
                x += step_x;
                y += step_y;
                if x < 0 || x > last || y < 0 || y > last {
                    break 'march;
                }
                strength[at(x, y)] = wind;
            }
        }
    }
    strength
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

/// The uniform for water.wgsl's `Water`.
pub fn uniform(image: &TerrainImage, time: f64, sky_rotation: f32) -> Vec<u8> {
    let water = &image.water;
    let [wave1, wave2] = wave_params(&water.kind);
    let (current, next) = frames(time);
    let (wind_sin, wind_cos) = (-water.wind_angle).sin_cos();
    let (sky_sin, sky_cos) = (std::f32::consts::PI + sky_rotation).sin_cos();
    let [r, g, b] = water.color;
    let [tint_r, tint_g, tint_b] = water.tint;
    let values: [f32; (UNIFORM_SIZE / 4) as usize] = [
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
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
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
                    depth_or_array_layers: FRAMES as u32,
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

/// What the water pipeline binds that does not change with the map.
pub struct Water {
    pub pipeline: wgpu::RenderPipeline,
    pub layout: wgpu::BindGroupLayout,
    pub buffer: wgpu::Buffer,
    pub sampler: wgpu::Sampler,
}

impl Water {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        view_layout: &wgpu::BindGroupLayout,
        sky_layout: &wgpu::BindGroupLayout,
        light_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water"),
            source: wgpu::ShaderSource::Wgsl(include_str!("water.wgsl").into()),
        });
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water"),
            bind_group_layouts: &[
                Some(view_layout),
                Some(&layout),
                Some(sky_layout),
                Some(light_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
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
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_water"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Water {
            pipeline,
            layout,
            buffer,
            sampler,
        }
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
}
