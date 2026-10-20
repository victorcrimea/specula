//! The selection overlays the engine draws after the water
//! (`RenderOverlaysAfterWater`): the outlines round buildings, then the
//! quads under units, rings and stars (`CCmpSelectable`), with the
//! textures of `/selection.bin`. Nothing draws until it has loaded.
//!
//! A texture is found by the name the template gives; its mask is the one
//! selection-atlas paired with it, which is the template's own in every
//! template of the game.

use std::collections::HashMap;
use std::f32::consts::PI;

use crate::terrain::TILE_SIZE;
use crate::terrain::TerrainImage;
use crate::territory;
use crate::territory::OVERLAY_VOFFSET;
use crate::unit::DEPTH_FORMAT;
use crate::unit::create_buffer;

/// selection.bin's format, built by specula-tools' selection-atlas (layout
/// documented there).
const ATLAS_VERSION: u8 = 1;
/// Four corners f32x3, the atlas rect f32x4, colour u8x4; selection.wgsl.
const QUAD_SIZE: usize = 68;
const QUAD_ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x3,
    3 => Float32x3,
    4 => Float32x4,
    5 => Unorm8x4,
];
/// `SubdividePoints`' and `ConstructTexturedLineCircle`'s longest step,
/// `TERRAIN_TILE_SIZE / 3`.
const OUTLINE_STEP: f32 = TILE_SIZE / 3.0;

/// `CCmpSelectable`'s overlay shape, its own or the footprint's.
#[derive(Clone, Copy)]
pub enum OverlayShape {
    Square { width: f32, depth: f32 },
    Circle { radius: f32 },
}

/// One entity's overlay this frame.
pub struct Mark<'a> {
    /// Relative to art/textures/selection/.
    pub texture: &'a str,
    /// An outline's line thickness; `None` for a quad.
    pub outline: Option<f32>,
    pub shape: OverlayShape,
    /// The interpolated position, x and z.
    pub centre: [f32; 2],
    /// `Pickable::facing`.
    pub facing: [f32; 2],
    /// 0xAARRGGBB.
    pub color: u32,
}

struct Textures {
    group: wgpu::BindGroup,
    /// Quad name to u0, v0, u1, v1 in the atlas.
    quads: HashMap<String, [f32; 4]>,
    /// Line texture name to its own group.
    lines: HashMap<String, wgpu::BindGroup>,
}

pub struct Overlays {
    quad_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: Option<Textures>,
    quads: wgpu::Buffer,
    quad_count: u32,
    line_vertices: wgpu::Buffer,
    line_indices: wgpu::Buffer,
    /// Per line texture, its index range.
    line_draws: Vec<(String, std::ops::Range<u32>)>,
}

impl Overlays {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        view_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        // The engine clamps the textures to their border across and to the
        // edge along; every edge texel of them is clear.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("selection overlay"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: territory::ANISOTROPY,
            ..Default::default()
        });
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
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
            label: Some("selection overlay"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("selection overlay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("selection.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("selection overlay"),
            bind_group_layouts: &[Some(view_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |label: &str,
                        entry_point: &str,
                        buffer: wgpu::VertexBufferLayout,
                        topology: wgpu::PrimitiveTopology| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    buffers: &[Some(buffer)],
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                // On the ground, as the territory borders: hidden behind
                // what stands in front, hiding nothing itself.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    // Lifted only 0.2 m, which far away the depth buffer
                    // cannot tell from the ground.
                    bias: wgpu::DepthBiasState {
                        constant: -2,
                        slope_scale: -1.0,
                        clamp: 0.0,
                    },
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let quad_pipeline = pipeline(
            "selection quad",
            "vs_quad",
            wgpu::VertexBufferLayout {
                array_stride: QUAD_SIZE as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &QUAD_ATTRIBUTES,
            },
            wgpu::PrimitiveTopology::TriangleStrip,
        );
        let line_pipeline = pipeline(
            "selection outline",
            "vs_line",
            wgpu::VertexBufferLayout {
                array_stride: territory::VERTEX_SIZE as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &territory::VERTEX_ATTRIBUTES,
            },
            wgpu::PrimitiveTopology::TriangleList,
        );
        let empty = |label, usage| create_buffer(device, queue, label, &[], usage);
        Overlays {
            quad_pipeline,
            line_pipeline,
            layout,
            sampler,
            textures: None,
            quads: empty("selection quads", wgpu::BufferUsages::VERTEX),
            quad_count: 0,
            line_vertices: empty("selection outline vertices", wgpu::BufferUsages::VERTEX),
            line_indices: empty("selection outline indices", wgpu::BufferUsages::INDEX),
            line_draws: Vec::new(),
        }
    }

    /// Uploads `/selection.bin`.
    pub fn set_atlas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bytes: &[u8],
    ) -> Result<(), &'static str> {
        let atlas = decode(bytes)?;
        let group = |label: &str, width: u32, height: u32, rgba: Vec<u8>, mask: Vec<u8>| {
            let base = territory::upload_texture(
                device,
                queue,
                label,
                width,
                height,
                wgpu::TextureFormat::Rgba8Unorm,
                rgba,
            );
            let mask = territory::upload_texture(
                device,
                queue,
                label,
                width,
                height,
                wgpu::TextureFormat::R8Unorm,
                mask,
            );
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&base),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&mask),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            })
        };
        let quads_group = group(
            "selection atlas",
            atlas.width,
            atlas.height,
            atlas.rgba,
            atlas.mask,
        );
        let lines = atlas
            .lines
            .into_iter()
            .map(|line| {
                let bind = group(&line.name, line.width, line.height, line.rgba, line.mask);
                (line.name, bind)
            })
            .collect();
        self.textures = Some(Textures {
            group: quads_group,
            quads: atlas.quads,
            lines,
        });
        Ok(())
    }

    /// Lays this frame's `marks` on `image`.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        marks: &[Mark],
        image: Option<&TerrainImage>,
    ) {
        self.quad_count = 0;
        self.line_draws.clear();
        let (Some(textures), Some(image)) = (&self.textures, image) else {
            return;
        };
        // Clamped to the water, as the engine's corners.
        let height = |x: f32, z: f32| image.ground(x, z).max(image.water_level) + OVERLAY_VOFFSET;

        let mut quads = Vec::new();
        let mut outlines: Vec<&Mark> = Vec::new();
        for mark in marks {
            if mark.outline.is_some() {
                if textures.lines.contains_key(mark.texture) {
                    outlines.push(mark);
                }
                continue;
            }
            let Some(rect) = textures.quads.get(mark.texture) else {
                continue;
            };
            // `UpdateDynamicOverlay`.
            let (half_x, half_z) = match mark.shape {
                OverlayShape::Square { width, depth } => (width / 2.0, depth / 2.0),
                OverlayShape::Circle { radius } => (radius, radius),
            };
            let [ux, uz] = mark.facing;
            let unit_z = [-uz, ux];
            let [cx, cz] = mark.centre;
            for (sx, sz) in [(-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0), (1.0, 1.0)] {
                let x = cx + ux * sx * half_x + unit_z[0] * sz * half_z;
                let z = cz + uz * sx * half_x + unit_z[1] * sz * half_z;
                for value in [x, height(x, z), z] {
                    quads.extend_from_slice(&value.to_le_bytes());
                }
            }
            for value in rect {
                quads.extend_from_slice(&value.to_le_bytes());
            }
            let argb = mark.color.to_be_bytes();
            quads.extend_from_slice(&[argb[1], argb[2], argb[3], argb[0]]);
        }
        self.quad_count = (quads.len() / QUAD_SIZE) as u32;
        upload(device, queue, &mut self.quads, "selection quads", &quads);

        // One draw per line texture.
        outlines.sort_by(|a, b| a.texture.cmp(b.texture));
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for mark in outlines {
            let thickness = mark.outline.unwrap_or(0.0);
            let start = indices.len() as u32;
            territory::ribbon(
                &outline_points(mark, thickness),
                mark.color,
                thickness,
                image,
                &mut vertices,
                &mut indices,
            );
            let end = indices.len() as u32;
            match self.line_draws.last_mut() {
                Some((texture, range)) if texture == mark.texture => range.end = end,
                _ => self.line_draws.push((mark.texture.to_string(), start..end)),
            }
        }
        let index_bytes: Vec<u8> = indices
            .iter()
            .flat_map(|index| index.to_le_bytes())
            .collect();
        upload(
            device,
            queue,
            &mut self.line_vertices,
            "selection outline vertices",
            &vertices,
        );
        upload(
            device,
            queue,
            &mut self.line_indices,
            "selection outline indices",
            &index_bytes,
        );
    }

    /// Expects the view bind group at 0.
    pub fn draw(&self, pass: &mut wgpu::RenderPass) {
        let Some(textures) = &self.textures else {
            return;
        };
        if !self.line_draws.is_empty() {
            pass.set_pipeline(&self.line_pipeline);
            pass.set_vertex_buffer(0, self.line_vertices.slice(..));
            pass.set_index_buffer(self.line_indices.slice(..), wgpu::IndexFormat::Uint32);
            for (texture, range) in &self.line_draws {
                if let Some(group) = textures.lines.get(texture) {
                    pass.set_bind_group(1, group, &[]);
                    pass.draw_indexed(range.clone(), 0, 0..1);
                }
            }
        }
        if self.quad_count > 0 {
            pass.set_pipeline(&self.quad_pipeline);
            pass.set_bind_group(1, &textures.group, &[]);
            pass.set_vertex_buffer(0, self.quads.slice(..));
            pass.draw(0..4, 0..self.quad_count);
        }
    }
}

/// `SimRender::ConstructTexturedLineBox` or `...Circle`: just outside the
/// shape, so the line strokes it from the outside.
fn outline_points(mark: &Mark, thickness: f32) -> Vec<[f32; 2]> {
    let [cx, cz] = mark.centre;
    match mark.shape {
        OverlayShape::Square { width, depth } => {
            let [ux, uz] = mark.facing;
            let unit_z = [-uz, ux];
            let (half_x, half_z) = (width / 2.0 + thickness / 2.0, depth / 2.0 + thickness / 2.0);
            let corner = |sx: f32, sz: f32| {
                [
                    cx + ux * sx * half_x + unit_z[0] * sz * half_z,
                    cz + uz * sx * half_x + unit_z[1] * sz * half_z,
                ]
            };
            let corners = [
                corner(1.0, -1.0),
                corner(-1.0, -1.0),
                corner(-1.0, 1.0),
                corner(1.0, 1.0),
            ];
            // `SubdividePoints` of a closed line.
            let mut points = Vec::new();
            for (i, &from) in corners.iter().enumerate() {
                let to = corners[(i + 1) % corners.len()];
                let length = (to[0] - from[0]).hypot(to[1] - from[1]);
                let segments = (length / OUTLINE_STEP).ceil().max(1.0) as usize;
                for s in 0..segments {
                    let t = s as f32 / segments as f32;
                    points.push([
                        from[0] + (to[0] - from[0]) * t,
                        from[1] + (to[1] - from[1]) * t,
                    ]);
                }
            }
            points
        }
        OverlayShape::Circle { radius } => {
            let radius = radius + thickness / 3.0;
            let steps = (2.0 * PI * radius / OUTLINE_STEP).ceil() as usize;
            (0..steps)
                .map(|i| {
                    let angle = i as f32 * 2.0 * PI / steps as f32;
                    [cx + radius * angle.sin(), cz + radius * angle.cos()]
                })
                .collect()
        }
    }
}

/// Writes `bytes` to `buffer`, growing it in powers of two as particle
/// instances do.
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    label: &str,
    bytes: &[u8],
) {
    if bytes.len() as u64 > buffer.size() {
        *buffer = create_buffer(
            device,
            queue,
            label,
            &vec![0; bytes.len().next_power_of_two()],
            buffer.usage(),
        );
    }
    if !bytes.is_empty() {
        queue.write_buffer(buffer, 0, bytes);
    }
}

struct Atlas {
    width: u32,
    height: u32,
    quads: HashMap<String, [f32; 4]>,
    rgba: Vec<u8>,
    mask: Vec<u8>,
    lines: Vec<Line>,
}

struct Line {
    name: String,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    mask: Vec<u8>,
}

fn decode(bytes: &[u8]) -> Result<Atlas, &'static str> {
    let mut cursor = Cursor(bytes);
    if cursor.u8()? != ATLAS_VERSION {
        return Err("unknown selection.bin version");
    }
    let width = cursor.u32()?;
    let height = cursor.u32()?;
    let mut quads = HashMap::new();
    for _ in 0..cursor.u32()? {
        let name = cursor.name()?;
        cursor.name()?;
        let [x, y, w, h] =
            [cursor.u16()?, cursor.u16()?, cursor.u16()?, cursor.u16()?].map(f32::from);
        quads.insert(
            name,
            [
                x / width as f32,
                y / height as f32,
                (x + w) / width as f32,
                (y + h) / height as f32,
            ],
        );
    }
    let mut sizes = Vec::new();
    for _ in 0..cursor.u32()? {
        let name = cursor.name()?;
        cursor.name()?;
        sizes.push((name, cursor.u32()?, cursor.u32()?));
    }
    let texels = width as usize * height as usize;
    let rgba = cursor.take(texels * 4)?.to_vec();
    let mask = cursor.take(texels)?.to_vec();
    let mut lines = Vec::new();
    for (name, width, height) in sizes {
        let texels = width as usize * height as usize;
        lines.push(Line {
            name,
            width,
            height,
            rgba: cursor.take(texels * 4)?.to_vec(),
            mask: cursor.take(texels)?.to_vec(),
        });
    }
    // The status bar sprites, which nothing draws until C18.
    let sprite_width = cursor.u32()? as usize;
    let sprite_height = cursor.u32()? as usize;
    for _ in 0..cursor.u32()? {
        cursor.name()?;
        cursor.take(4 * 2)?;
    }
    cursor.take(sprite_width * sprite_height * 4)?;
    if !cursor.0.is_empty() {
        return Err("selection.bin has trailing bytes");
    }
    Ok(Atlas {
        width,
        height,
        quads,
        rgba,
        mask,
        lines,
    })
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], &'static str> {
        if self.0.len() < length {
            return Err("selection.bin is cut short");
        }
        let (bytes, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, &'static str> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, &'static str> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn name(&mut self) -> Result<String, &'static str> {
        let length = usize::from(self.u16()?);
        std::str::from_utf8(self.take(length)?)
            .map(str::to_string)
            .map_err(|_| "selection.bin name not UTF-8")
    }
}
