//! The observer page's impostors. A look (an actor in one animation) that
//! shows small on screen is drawn into
//! an atlas, from `DIRECTIONS` headings for up to `MAX_FRAMES` frames,
//! albedo and normal apart, with the coarse meshes and the smallest mips
//! the asset files start with, so it exists before the full model has
//! arrived. Its entities are then one quad each (far.wgsl).
//!
//! Atlases are kept within a memory budget, least recently drawn dropped
//! first, and saved to the origin private file system, so a later visit
//! loads them instead of drawing them again.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::rc::Rc;

use wasm_bindgen_futures::spawn_local;
use web_sys::FileSystemDirectoryHandle;

use crate::impostor;
use crate::impostor::Atlas;
use crate::impostor::CELL;
use crate::impostor::DIRECTIONS;
use crate::impostor::Mips;
use crate::impostor::clear_target;
use crate::opfs;
use crate::stream::Dot;
use crate::unit::DEPTH_FORMAT;
use crate::unit::create_buffer;
use crate::unit::create_depth;
use crate::variation::Model;

/// Fewer than the unit page's: a match has many looks to hold at once.
pub const MAX_FRAMES: u32 = 4;
/// Sphere centre and radius, turn and grid, cell and colour; far.wgsl.
pub const INSTANCE_SIZE: usize = 40;
/// Atlas bytes kept on the GPU, mips included.
const BUDGET: u64 = 192 << 20;
/// Drawing an atlas is a few hundred draws; one per frame keeps frames
/// smooth while a crowd comes into view.
const BAKES_PER_FRAME: usize = 1;
/// Taking a saved atlas in is a full upload and its mips; a few a frame
/// keep panning smooth.
const UPLOADS_PER_FRAME: usize = 2;
/// Frames before a look that did not fit is asked for again, by when the
/// view may have left room.
const RETRY_FRAMES: u64 = 60;
/// model.wgsl `View`, which the capture fills only the matrix of.
const CAPTURE_VIEW_SIZE: u64 = 112;
const OPFS_DIR: &str = "impostors";
/// Changes whenever what is saved, or how it is drawn, does.
const FILE_VERSION: u32 = 6;
/// The mip level of the capture the normal plane is kept from: lighting
/// at half the albedo's resolution does not show at this size, and it
/// saves most of the plane.
const NORMAL_LEVEL: u32 = 1;
const FILE_MAGIC: &[u8; 4] = b"SPIM";
const FILE_HEADER: usize = 4 + 4 * 6;

/// (actor, animation) as the stream indexes them, and the root's animation
/// file hash (0 for none). The seed and the variant selections are left
/// out on purpose: at this size a head, shield or prop does not show, and
/// keying by them would give every citizen an atlas of its own. A look is
/// drawn with the variant of the entity that first asked for it.
pub type LookKey = (u32, u32, u64);

/// The frames an atlas holds of an animation `frames` long. Idling is
/// breathing and shifting weight, which does not show at this size.
pub fn slots(state: &str, frames: u32) -> u32 {
    if state == "idle" {
        1
    } else {
        frames.clamp(1, MAX_FRAMES)
    }
}

/// Reads from storage that have finished, the bytes if found.
type Loaded = Rc<RefCell<Vec<(LookKey, Option<Vec<u8>>)>>>;

/// What drawing a look's atlas needs, from the entity that first asked.
pub struct Request {
    pub model: Rc<Model>,
    pub dot: Dot,
    /// The animation name, lower case.
    pub state: String,
    pub pick: u64,
    /// The bounding sphere in the model's own space.
    pub radius: f32,
    pub centre_y: f32,
    /// Its file in the origin private file system.
    pub file: String,
}

pub struct Look {
    pub atlas: Atlas,
    pub radius: f32,
    pub centre_y: f32,
    bytes: u64,
    last_used: u64,
}

enum State {
    /// Being read from the origin private file system, or waiting for it
    /// to open.
    Loading(Request),
    Queued(Request),
    Ready(Look),
    /// Did not fit beside the atlases on screen; drawn as a mesh until
    /// `retry`.
    NoRoom {
        retry: u64,
    },
}

enum Storage {
    Closed,
    Opening,
    Open(FileSystemDirectoryHandle),
    Unavailable,
}

/// One instanced draw of the capture: a mesh level with its material, for
/// every direction of frame `slot`.
pub struct CaptureDraw<'a> {
    pub slot: u32,
    pub vertices: &'a wgpu::Buffer,
    pub indices: &'a wgpu::Buffer,
    pub index_range: (u32, u32),
    pub material: &'a wgpu::BindGroup,
    /// Byte range of its instances.
    pub instances: (u64, u64),
    pub count: u32,
}

/// What a bake draws: `slots` frames spread over the root animation's
/// `frames`, model instances in model space.
pub struct Capture<'a> {
    pub slots: u32,
    pub frames: u32,
    pub instances: &'a [u8],
    pub draws: &'a [CaptureDraw<'a>],
    pub poses: &'a wgpu::BindGroup,
}

pub struct Far {
    capture_pipeline: wgpu::RenderPipeline,
    capture_layout: wgpu::BindGroupLayout,
    mips: Mips,
    pub pipeline: wgpu::RenderPipeline,
    elevation: f32,
    looks: HashMap<LookKey, State>,
    queue: VecDeque<LookKey>,
    bytes: u64,
    frame: u64,
    storage: Rc<RefCell<Storage>>,
    /// Looks asked for before storage was open, to read once it is.
    unread: Vec<LookKey>,
    loaded: Loaded,
    /// Bytes kept free for the reads under way.
    reading: u64,
    /// Atlases taken from storage and drawn, in all and since the stats
    /// line last took them.
    pub from_storage: u32,
    pub drawn: u32,
    pub uploads: u32,
    pub bakes: u32,
}

impl Far {
    /// `elevation` is the camera pitch atlases are drawn from;
    /// `model_buffers` the model pipeline's vertex buffers.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        model_shader: &wgpu::ShaderModule,
        model_buffers: &[Option<wgpu::VertexBufferLayout>],
        view_layout: &wgpu::BindGroupLayout,
        pose_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        light_layout: &wgpu::BindGroupLayout,
        elevation: f32,
    ) -> Self {
        // The page's view uniform, one per cell at a dynamic offset, so
        // all cells are drawn in one pass.
        let capture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("far capture view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(CAPTURE_VIEW_SIZE),
                },
                count: None,
            }],
        });
        let depth_stencil = wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        };
        let capture_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("far capture"),
                bind_group_layouts: &[
                    Some(&capture_layout),
                    Some(pose_layout),
                    Some(material_layout),
                ],
                immediate_size: 0,
            });
        let targets = [Some(impostor::FORMAT.into()), Some(impostor::FORMAT.into())];
        let capture_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("far capture"),
            layout: Some(&capture_pipeline_layout),
            vertex: wgpu::VertexState {
                module: model_shader,
                entry_point: Some("vs_model"),
                compilation_options: Default::default(),
                buffers: model_buffers,
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_stencil.clone()),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: model_shader,
                entry_point: Some("fs_capture"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });

        let mips = Mips::new(device);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("far"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("far.wgsl"),
                    include_str!("shared/turn.wgsl"),
                    include_str!("shared/fog.wgsl"),
                )
                .into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("far"),
            bind_group_layouts: &[
                Some(view_layout),
                Some(&mips.atlas_layout),
                Some(light_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("far"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_impostor"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4,
                        1 => Float32x4,
                        2 => Uint32x2,
                    ],
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

        Far {
            capture_pipeline,
            capture_layout,
            mips,
            pipeline,
            elevation,
            looks: HashMap::new(),
            queue: VecDeque::new(),
            bytes: 0,
            frame: 0,
            storage: Rc::new(RefCell::new(Storage::Closed)),
            unread: Vec::new(),
            loaded: Rc::new(RefCell::new(Vec::new())),
            reading: 0,
            from_storage: 0,
            drawn: 0,
            uploads: 0,
            bakes: 0,
        }
    }

    /// Stream indices belong to their match.
    pub fn clear(&mut self) {
        self.looks.clear();
        self.queue.clear();
        self.unread.clear();
        self.bytes = 0;
    }

    /// The look's atlas if it is ready, marked as drawn this frame.
    pub fn get(&mut self, key: LookKey) -> Option<&Look> {
        match self.looks.get_mut(&key) {
            Some(State::Ready(look)) => {
                look.last_used = self.frame;
                Some(look)
            }
            _ => None,
        }
    }

    /// The look's atlas if it is ready, as drawn.
    pub fn ready(&self, key: LookKey) -> Option<&Look> {
        match self.looks.get(&key) {
            Some(State::Ready(look)) => Some(look),
            _ => None,
        }
    }

    /// Whether the look's atlas was asked for and has not been dropped,
    /// nor is due to be asked again.
    pub fn asked(&self, key: LookKey) -> bool {
        match self.looks.get(&key) {
            Some(State::NoRoom { retry }) => *retry > self.frame,
            Some(_) => true,
            None => false,
        }
    }

    /// Atlases held, their bytes, and looks waiting for room.
    pub fn held(&self) -> (usize, u64, usize) {
        let ready = self
            .looks
            .values()
            .filter(|state| matches!(state, State::Ready(_)))
            .count();
        let waiting = self
            .looks
            .values()
            .filter(|state| matches!(state, State::NoRoom { .. }))
            .count();
        (ready, self.bytes, waiting)
    }

    pub fn budget(&self) -> u64 {
        BUDGET
    }

    /// Asks for the look's atlas: from storage if it has it, else drawn.
    /// Read in `poll`, as storage opens only once the assets are known,
    /// and with cached assets the first looks are asked for before that.
    pub fn want(&mut self, key: LookKey, request: Request) {
        if self.asked(key) {
            return;
        }
        self.looks.insert(key, State::Loading(request));
        self.unread.push(key);
    }

    /// Starts the reads of the looks asked for, or queues them to be
    /// drawn when there is no storage.
    fn read_asked(&mut self, device: &wgpu::Device) {
        let dir = match &*self.storage.borrow() {
            Storage::Open(dir) => Some(dir.clone()),
            Storage::Unavailable => None,
            Storage::Closed | Storage::Opening => return,
        };
        for key in std::mem::take(&mut self.unread) {
            if !matches!(self.looks.get(&key), Some(State::Loading(_))) {
                continue;
            }
            if !self.could_fit(Self::atlas_bytes(device, MAX_FRAMES)) {
                self.no_room(key);
                continue;
            }
            let Some(State::Loading(request)) = self.looks.get(&key) else {
                continue;
            };
            match &dir {
                Some(dir) => {
                    self.reading += Self::atlas_bytes(device, MAX_FRAMES);
                    let dir = dir.clone();
                    let loaded = Rc::clone(&self.loaded);
                    let file = request.file.clone();
                    spawn_local(async move {
                        let bytes = opfs::read(&dir, &file).await;
                        loaded.borrow_mut().push((key, bytes));
                    });
                }
                None => {
                    if let Some(State::Loading(request)) = self.looks.remove(&key) {
                        self.looks.insert(key, State::Queued(request));
                        self.queue.push_back(key);
                    }
                }
            }
        }
    }

    /// Opens storage once the asset bake is known, which names it.
    pub fn open_storage(&mut self, bake: &str) {
        if !matches!(*self.storage.borrow(), Storage::Closed) {
            return;
        }
        *self.storage.borrow_mut() = Storage::Opening;
        let storage = Rc::clone(&self.storage);
        let version = format!("{bake} {FILE_VERSION}");
        spawn_local(async move {
            let opened = opfs::directory(OPFS_DIR, &version).await;
            *storage.borrow_mut() = match opened {
                Ok(dir) => Storage::Open(dir),
                Err(error) => {
                    crate::log(&format!("impostors are not saved: {error}"));
                    Storage::Unavailable
                }
            };
        });
    }

    /// Once a frame, before placing.
    pub fn begin_frame(&mut self, device: &wgpu::Device) {
        self.frame += 1;
        // WebGL2 completes buffer mappings only when polled.
        let _ = device.poll(wgpu::PollType::Poll);
    }

    /// Once a frame, after placing, so the atlases drawn this frame are
    /// not dropped for the ones taken in: reads what was asked for and
    /// takes in what storage has returned.
    pub fn take_loaded(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.read_asked(device);
        let mut uploads = 0;
        loop {
            if uploads == UPLOADS_PER_FRAME {
                break;
            }
            let next = {
                let mut loaded = self.loaded.borrow_mut();
                if loaded.is_empty() {
                    None
                } else {
                    Some(loaded.remove(0))
                }
            };
            let Some((key, bytes)) = next else {
                break;
            };
            self.reading -= Self::atlas_bytes(device, MAX_FRAMES);
            let Some(State::Loading(request)) = self.looks.remove(&key) else {
                continue;
            };
            let Some(bytes) = bytes else {
                self.looks.insert(key, State::Queued(request));
                self.queue.push_back(key);
                continue;
            };
            uploads += 1;
            let look = self.upload(device, queue, &bytes);
            match look {
                Some(look) if self.make_room(look.bytes) => {
                    self.bytes += look.bytes;
                    self.from_storage += 1;
                    self.uploads += 1;
                    self.looks.insert(key, State::Ready(look));
                }
                Some(_) => self.no_room(key),
                None => {
                    self.looks.insert(key, State::Queued(request));
                    self.queue.push_back(key);
                }
            }
        }
    }

    fn no_room(&mut self, key: LookKey) {
        let retry = self.frame + RETRY_FRAMES;
        self.looks.insert(key, State::NoRoom { retry });
    }

    /// Whether `bytes` more fit once the atlases not drawn this frame are
    /// dropped.
    fn could_fit(&self, bytes: u64) -> bool {
        let evictable: u64 = self
            .looks
            .values()
            .map(|state| match state {
                State::Ready(look) if look.last_used < self.frame => look.bytes,
                _ => 0,
            })
            .sum();
        self.bytes + self.reading - evictable + bytes <= BUDGET
    }

    /// The looks to draw this frame, with their requests; each is either
    /// drawn with `bake` or handed back with `defer`.
    pub fn next_bakes(&mut self, device: &wgpu::Device) -> Vec<(LookKey, Request)> {
        let mut out = Vec::new();
        while out.len() < BAKES_PER_FRAME {
            let Some(key) = self.queue.pop_front() else {
                break;
            };
            if !matches!(self.looks.get(&key), Some(State::Queued(_))) {
                continue;
            }
            if !self.could_fit(Self::atlas_bytes(device, MAX_FRAMES)) {
                self.no_room(key);
                continue;
            }
            if let Some(State::Queued(request)) = self.looks.remove(&key) {
                out.push((key, request));
            }
        }
        out
    }

    /// A look whose pieces have not all arrived, to try again later.
    pub fn defer(&mut self, key: LookKey, request: Request) {
        self.looks.insert(key, State::Queued(request));
        self.queue.push_back(key);
    }

    /// Bytes an atlas of `slots` frames takes.
    fn atlas_bytes(device: &wgpu::Device, slots: u32) -> u64 {
        let (columns, rows) = impostor::grid(device, slots * DIRECTIONS);
        let level0 = u64::from(columns * CELL) * u64::from(rows * CELL) * 4;
        let normal = level0 >> (2 * NORMAL_LEVEL);
        // A third more for the mips.
        (level0 + normal) * 4 / 3
    }

    /// Drops the least recently drawn atlases until `bytes` more fit;
    /// false when they do not, as every atlas left was drawn this frame.
    fn make_room(&mut self, bytes: u64) -> bool {
        while self.bytes + bytes > BUDGET {
            let oldest = self
                .looks
                .iter()
                .filter_map(|(key, state)| match state {
                    State::Ready(look) if look.last_used < self.frame => {
                        Some((look.last_used, *key))
                    }
                    _ => None,
                })
                .min();
            let Some((_, key)) = oldest else {
                return false;
            };
            if let Some(State::Ready(look)) = self.looks.remove(&key) {
                self.bytes -= look.bytes;
            }
        }
        true
    }

    /// Draws the look's atlas and saves it to storage. Without room the
    /// look waits.
    pub fn bake(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: LookKey,
        request: Request,
        capture: &Capture,
    ) {
        let slots = capture.slots;
        let bytes = Self::atlas_bytes(device, slots);
        if !self.make_room(bytes) {
            self.no_room(key);
            return;
        }
        let cells = slots * DIRECTIONS;
        let (columns, rows) = impostor::grid(device, cells);
        let (width, height) = (columns * CELL, rows * CELL);

        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = CAPTURE_VIEW_SIZE.next_multiple_of(alignment);
        let mut uniforms = vec![0u8; (stride * u64::from(cells)) as usize];
        let centre = [0.0, request.centre_y, 0.0];
        for cell in 0..cells {
            let direction = cell % DIRECTIONS;
            let (view_proj, _, _, _) =
                impostor::cell_view(direction, self.elevation, request.radius, centre);
            let start = (stride * u64::from(cell)) as usize;
            // WGSL matrices are column-major.
            for column in 0..4 {
                for (row, values) in view_proj.iter().enumerate() {
                    let at = start + (column * 4 + row) * 4;
                    uniforms[at..at + 4].copy_from_slice(&values[column].to_le_bytes());
                }
            }
        }
        let uniform_buffer = create_buffer(
            device,
            queue,
            "far capture views",
            &uniforms,
            wgpu::BufferUsages::UNIFORM,
        );
        let instance_buffer = create_buffer(
            device,
            queue,
            "far capture instances",
            capture.instances,
            wgpu::BufferUsages::VERTEX,
        );
        let capture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("far capture view"),
            layout: &self.capture_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(CAPTURE_VIEW_SIZE),
                }),
            }],
        });

        let levels = Mips::scratch(device, width, height);
        let depth = create_depth(device, width, height);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let albedo = levels[0][0].create_view(&Default::default());
            let normal = levels[0][1].create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("far capture"),
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
            pass.set_bind_group(1, capture.poses, &[]);
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
                let slot = cell / DIRECTIONS;
                for draw in capture.draws.iter().filter(|draw| draw.slot == slot) {
                    let (first, count) = draw.index_range;
                    pass.set_bind_group(2, draw.material, &[]);
                    pass.set_vertex_buffer(0, draw.vertices.slice(..));
                    pass.set_vertex_buffer(
                        1,
                        instance_buffer.slice(draw.instances.0..draw.instances.1),
                    );
                    pass.set_index_buffer(draw.indices.slice(..), wgpu::IndexFormat::Uint16);
                    pass.draw_indexed(first..first + count, 0, 0..draw.count);
                }
            }
        }
        let bind_group =
            self.mips
                .finish(device, &mut encoder, &levels, width, height, NORMAL_LEVEL);
        let readback = self.read_back(device, &mut encoder, &levels, width, height);
        queue.submit([encoder.finish()]);

        let look = Look {
            atlas: Atlas {
                bind_group,
                columns,
                rows,
                slots,
                frames: capture.frames.max(1),
            },
            radius: request.radius,
            centre_y: request.centre_y,
            bytes,
            last_used: self.frame,
        };
        self.save(readback, &look, request.file);
        self.bytes += bytes;
        self.drawn += 1;
        self.bakes += 1;
        self.looks.insert(key, State::Ready(look));
    }

    /// Copies the albedo's level 0 and the normal's `NORMAL_LEVEL` to a
    /// buffer to save, when there is storage to save it to.
    fn read_back(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        levels: &[[wgpu::Texture; 2]],
        width: u32,
        height: u32,
    ) -> Option<wgpu::Buffer> {
        if !matches!(*self.storage.borrow(), Storage::Open(_)) {
            return None;
        }
        let (normal_width, normal_height) = (width >> NORMAL_LEVEL, height >> NORMAL_LEVEL);
        // Copies need rows of whole 256 bytes: a cell is that at level 0,
        // and the grid's even column counts keep it so at level 1.
        if (normal_width * 4) % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT != 0 {
            return None;
        }
        let albedo = u64::from(width) * u64::from(height) * 4;
        let normal = u64::from(normal_width) * u64::from(normal_height) * 4;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("far readback"),
            size: albedo + normal,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let planes = [
            (&levels[0][0], 0, width, height),
            (
                &levels[NORMAL_LEVEL as usize][1],
                albedo,
                normal_width,
                normal_height,
            ),
        ];
        for (texture, offset, width, height) in planes {
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset,
                        bytes_per_row: Some(width * 4),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        Some(buffer)
    }

    fn save(&self, readback: Option<wgpu::Buffer>, look: &Look, file: String) {
        let Some(buffer) = readback else {
            return;
        };
        let Storage::Open(dir) = &*self.storage.borrow() else {
            return;
        };
        let dir = dir.clone();
        let atlas = &look.atlas;
        let mut header = Vec::with_capacity(FILE_HEADER);
        header.extend_from_slice(FILE_MAGIC);
        for value in [atlas.columns, atlas.rows, atlas.slots, atlas.frames] {
            header.extend_from_slice(&value.to_le_bytes());
        }
        header.extend_from_slice(&look.radius.to_le_bytes());
        header.extend_from_slice(&look.centre_y.to_le_bytes());
        let mapped = buffer.clone();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            if result.is_err() {
                return;
            }
            let mut bytes = header;
            if let Ok(view) = mapped.get_mapped_range(..) {
                bytes.extend_from_slice(&view);
            }
            mapped.unmap();
            spawn_local(async move {
                if let Err(error) = opfs::write(&dir, &file, &bytes).await {
                    crate::log(&format!("impostor not saved: {error}"));
                }
            });
        });
    }

    /// A saved atlas back on the GPU, its mips rebuilt.
    fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> Option<Look> {
        if bytes.len() < FILE_HEADER || &bytes[..4] != FILE_MAGIC {
            return None;
        }
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let (columns, rows, slots, frames) = (u32_at(4), u32_at(8), u32_at(12), u32_at(16));
        let radius = f32::from_bits(u32_at(20));
        let centre_y = f32::from_bits(u32_at(24));
        // Saved on a device that allowed a wider atlas, or damaged.
        let fits = (1..=MAX_FRAMES).contains(&slots)
            && impostor::grid(device, slots * DIRECTIONS) == (columns, rows);
        let (width, height) = (columns * CELL, rows * CELL);
        let (normal_width, normal_height) = (width >> NORMAL_LEVEL, height >> NORMAL_LEVEL);
        let plane = width as usize * height as usize * 4;
        let normal_plane = normal_width as usize * normal_height as usize * 4;
        if !fits || bytes.len() != FILE_HEADER + plane + normal_plane {
            return None;
        }
        let albedo = &bytes[FILE_HEADER..FILE_HEADER + plane];
        // Spread back to full size so the mips are built as for a bake;
        // each saved texel becomes a block of equal ones, which average
        // back to it at NORMAL_LEVEL.
        let saved = &bytes[FILE_HEADER + plane..];
        let scale = 1usize << NORMAL_LEVEL;
        let mut normal = vec![0u8; plane];
        for (y, row) in normal.chunks_exact_mut(width as usize * 4).enumerate() {
            let source = &saved[(y / scale) * normal_width as usize * 4..];
            for (x, texel) in row.chunks_exact_mut(4).enumerate() {
                let at = (x / scale) * 4;
                texel.copy_from_slice(&source[at..at + 4]);
            }
        }
        let levels = Mips::scratch(device, width, height);
        for (texture, data) in levels[0].iter().zip([albedo, normal.as_slice()]) {
            queue.write_texture(
                texture.as_image_copy(),
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        let bind_group =
            self.mips
                .finish(device, &mut encoder, &levels, width, height, NORMAL_LEVEL);
        queue.submit([encoder.finish()]);
        Some(Look {
            atlas: Atlas {
                bind_group,
                columns,
                rows,
                slots,
                frames: frames.max(1),
            },
            radius,
            centre_y,
            bytes: Self::atlas_bytes(device, slots),
            last_used: self.frame,
        })
    }
}

/// The file a look is saved as: named by what its key stands for, so a
/// later visit finds it whatever stream indices the match gives it.
/// `state` is the animation name, lower case.
pub fn file_name(actor: &str, state: &str, root_animation: u64, elevation: f32) -> String {
    let mut hasher = DefaultHasher::new();
    (actor, state, root_animation).hash(&mut hasher);
    (CELL, DIRECTIONS, MAX_FRAMES, elevation.to_bits()).hash(&mut hasher);
    format!("{:016x}.bin", hasher.finish())
}
