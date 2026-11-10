//! The observer page's particles: `CParticleEmitterType::UpdateEmitter`,
//! `CParticleEmitter`, `CParticleManager`'s unattached emitters and
//! `ParticleRenderer` (graphics/ParticleEmitterType.cpp,
//! ParticleEmitter.cpp, ParticleManager.cpp, renderer/ParticleRenderer.cpp).
//!
//! An emitter is the particle prop of a model the view placed this frame.
//! One that is not placed, because its entity is gone, its variant changed
//! or it is drawn far as an impostor, stops emitting and is kept until its
//! last particle dies, as the engine keeps an unattached emitter. One out
//! of view is neither run nor drawn, as the engine culls it, and catches
//! up when it comes back, clamped to its longest lifetime as
//! `UpdateEmitter` clamps it.

use std::collections::HashMap;
use std::rc::Rc;

use specula_assets::files;
use specula_assets::files::Emitter;
use specula_assets::files::FileRef;
use specula_assets::files::PARTICLE_VARIABLES;
use specula_assets::files::ParticleVariable;
use specula_assets::math::Mat4;
use specula_assets::math::Vec3;

use crate::unit::DEPTH_FORMAT;
use crate::variation::Mt19937;

/// Centre f32x3, corner axis f32x2, colour unorm8x4.
const INSTANCE_SIZE: usize = 24;
/// `UpdateEmitter`'s `maxStepLength`, in seconds.
const MAX_STEP: f32 = 0.2;
/// What `CParticleManager::m_RNG`, a default `std::mt19937`, starts from.
const RNG_SEED: u32 = 5489;
/// A variable copying in a cycle names no bound; this ends the search.
const MAX_COPY_DEPTH: usize = PARTICLE_VARIABLES;

/// What carries an emitter, so it is found again each frame.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    Entity(u32),
    /// A projectile in flight, and the actor it leaves where it lands.
    Shot(u32),
    Impact(u32),
}

/// The owner and the particle model in its tree, by address: a model is
/// resolved anew when the entity's actor, variant or animation changes,
/// which makes a new emitter as `CUnit::ReloadObject` does.
type Key = (Owner, usize);

#[derive(Clone, Copy)]
struct Particle {
    position: Vec3,
    velocity: Vec3,
    angle: f32,
    angle_speed: f32,
    size: f32,
    growth: f32,
    color: [u8; 4],
    age: f32,
    max_age: f32,
}

impl Particle {
    fn alive(&self) -> bool {
        self.age < self.max_age
    }
}

struct Instance {
    kind: Rc<Emitter>,
    max_lifetime: f32,
    max_particles: usize,
    /// How far from the emitter a particle can get, for culling.
    reach: f32,
    active: bool,
    position: Vec3,
    transform: Mat4,
    /// The entity's variables, for `Expr`.
    variables: Vec<(String, f32)>,
    /// A ring of at most `max_particles`, as `AddParticle` keeps it.
    particles: Vec<Particle>,
    next: usize,
    last_update: f64,
    rounding: f32,
    /// What each variable last gave, for `Copy`.
    last: [f32; PARTICLE_VARIABLES],
    placed: bool,
    /// Run this frame, so drawn.
    drawn: bool,
}

impl Instance {
    fn new(kind: &Rc<Emitter>, time: f64) -> Self {
        let max_lifetime = max(kind, files::VAR_LIFETIME, 0);
        let max_particles = (max(kind, files::VAR_EMISSION_RATE, 0) * max_lifetime)
            .ceil()
            .clamp(0.0, f32::from(u16::MAX)) as usize;
        // As if created that long ago, so the first update fills it.
        let last_update = if kind.start_full {
            time - f64::from(max_lifetime)
        } else {
            time
        };
        Instance {
            kind: Rc::clone(kind),
            max_lifetime,
            max_particles,
            reach: reach(kind, max_lifetime),
            active: true,
            position: Vec3::new(0.0, 0.0, 0.0),
            transform: Mat4::identity(),
            variables: Vec::new(),
            particles: Vec::with_capacity(max_particles),
            next: 0,
            last_update,
            rounding: 0.0,
            last: [0.0; PARTICLE_VARIABLES],
            placed: false,
            drawn: false,
        }
    }

    fn evaluate(&mut self, id: usize, rng: &mut Mt19937) -> f32 {
        let value = match &self.kind.variables[id] {
            ParticleVariable::Constant(value) => *value,
            ParticleVariable::Uniform(min, max) => rng.uniform_real(*min, *max),
            ParticleVariable::Copy(from) => self.last[usize::from(*from)],
            ParticleVariable::Expr { from, mul, max } => {
                let variable = self
                    .variables
                    .iter()
                    .find(|(name, _)| name == from)
                    .map_or(0.0, |(_, value)| *value);
                let value = variable * mul;
                if value < *max { value } else { *max }
            }
        };
        self.last[id] = value;
        value
    }

    /// `CParticleEmitterType::UpdateEmitter`.
    fn update(&mut self, dt: f32, rng: &mut Mt19937) {
        let mut dt = dt.min(self.max_lifetime);
        while dt > MAX_STEP {
            self.step(MAX_STEP, rng);
            dt -= MAX_STEP;
        }
        self.step(dt, rng);
    }

    /// `CParticleEmitterType::UpdateEmitterStep`.
    fn step(&mut self, dt: f32, rng: &mut Mt19937) {
        if self.active {
            let rate = self.evaluate(files::VAR_EMISSION_RATE, rng);
            let new = (self.rounding + dt * rate).floor();
            self.rounding += dt * rate - new;
            let new = (new.max(0.0) as usize).min(self.max_particles);
            for _ in 0..new {
                let offset = Vec3::new(
                    self.evaluate(files::VAR_POSITION_X, rng),
                    self.evaluate(files::VAR_POSITION_Y, rng),
                    self.evaluate(files::VAR_POSITION_Z, rng),
                );
                let mut velocity = Vec3::new(
                    self.evaluate(files::VAR_VELOCITY_X, rng),
                    self.evaluate(files::VAR_VELOCITY_Y, rng),
                    self.evaluate(files::VAR_VELOCITY_Z, rng),
                );
                if self.kind.relative_velocity {
                    velocity = self.transform.rotate(velocity);
                }
                let angle = self.evaluate(files::VAR_ANGLE, rng);
                let angle_speed = self.evaluate(files::VAR_VELOCITY_ANGLE, rng);
                let size = self.evaluate(files::VAR_SIZE, rng);
                let growth = self.evaluate(files::VAR_SIZE_GROWTH_RATE, rng);
                // ConvertRGBColorTo4ub.
                let channel = |value: f32| (value * 255.0).clamp(0.0, 255.0) as u8;
                let color = [
                    channel(self.evaluate(files::VAR_COLOR_R, rng)),
                    channel(self.evaluate(files::VAR_COLOR_G, rng)),
                    channel(self.evaluate(files::VAR_COLOR_B, rng)),
                    255,
                ];
                let max_age = self.evaluate(files::VAR_LIFETIME, rng);
                let particle = Particle {
                    position: offset + self.position,
                    velocity,
                    angle,
                    angle_speed,
                    size,
                    growth,
                    color,
                    age: 0.0,
                    max_age,
                };
                if self.next >= self.particles.len() {
                    self.particles.push(particle);
                } else {
                    self.particles[self.next] = particle;
                }
                self.next = (self.next + 1) % self.max_particles;
            }
        }
        for particle in &mut self.particles {
            if particle.age > particle.max_age {
                continue;
            }
            particle.position = particle.position + particle.velocity * dt;
            particle.angle += particle.angle_speed * dt;
            particle.age += dt;
            particle.size += particle.growth * dt;
            // The engine's hard-coded fade in and out.
            let fraction = particle.age / particle.max_age;
            let alpha = (1.0 - fraction).min(5.0 * fraction);
            particle.color[3] = (alpha * 255.0).clamp(0.0, 255.0) as u8;
        }
        if self.kind.force != [0.0; 3] {
            let [x, y, z] = self.kind.force;
            let change = Vec3::new(x, y, z) * dt;
            for particle in &mut self.particles {
                particle.velocity = particle.velocity + change;
            }
        }
    }
}

/// The most a variable can give, as `IParticleVar::Max`.
fn max(kind: &Emitter, id: usize, depth: usize) -> f32 {
    match &kind.variables[id] {
        ParticleVariable::Constant(value) => *value,
        ParticleVariable::Uniform(_, max) => *max,
        ParticleVariable::Copy(from) if depth < MAX_COPY_DEPTH => {
            self::max(kind, usize::from(*from), depth + 1)
        }
        ParticleVariable::Copy(_) => 0.0,
        ParticleVariable::Expr { max, .. } => *max,
    }
}

/// The furthest a variable can give from 0.
fn extent(kind: &Emitter, id: usize, depth: usize) -> f32 {
    match &kind.variables[id] {
        ParticleVariable::Constant(value) => value.abs(),
        ParticleVariable::Uniform(min, max) => min.abs().max(max.abs()),
        ParticleVariable::Copy(from) if depth < MAX_COPY_DEPTH => {
            extent(kind, usize::from(*from), depth + 1)
        }
        ParticleVariable::Copy(_) => 0.0,
        ParticleVariable::Expr { max, .. } => max.abs(),
    }
}

/// A sphere round the emitter that holds every particle, as
/// `CParticleEmitterType::m_MaxBounds` and `CalculateBounds` bound them,
/// only rounder.
fn reach(kind: &Emitter, lifetime: f32) -> f32 {
    let length = |ids: [usize; 3]| {
        let [x, y, z] = ids.map(|id| extent(kind, id, 0));
        Vec3::new(x, y, z).length()
    };
    let offset = length([
        files::VAR_POSITION_X,
        files::VAR_POSITION_Y,
        files::VAR_POSITION_Z,
    ]);
    let speed = length([
        files::VAR_VELOCITY_X,
        files::VAR_VELOCITY_Y,
        files::VAR_VELOCITY_Z,
    ]);
    let [x, y, z] = kind.force;
    let force = Vec3::new(x, y, z).length();
    let size =
        extent(kind, files::VAR_SIZE, 0) + extent(kind, files::VAR_SIZE_GROWTH_RATE, 0) * lifetime;
    offset
        + speed * lifetime
        + 0.5 * force * lifetime * lifetime
        + size * std::f32::consts::SQRT_2 / 2.0
}

/// One emitter's particles in the instance buffer.
pub struct Draw {
    pub texture: FileRef,
    pub blend: u8,
    pub start: u64,
    pub end: u64,
    pub count: u32,
}

pub struct Particles {
    emitters: HashMap<Key, Instance>,
    rng: Mt19937,
    /// The sim time of the frame, in seconds.
    time: f64,
}

impl Particles {
    pub fn new() -> Self {
        Particles {
            emitters: HashMap::new(),
            rng: Mt19937::new(RNG_SEED),
            time: 0.0,
        }
    }

    pub fn clear(&mut self) {
        self.emitters.clear();
    }

    /// Starts a frame at the sim time `time`, in seconds. Particles cannot
    /// run backwards, so a seek back starts them over.
    pub fn begin(&mut self, time: f64) {
        if time < self.time {
            self.clear();
        }
        self.time = time;
        for emitter in self.emitters.values_mut() {
            emitter.placed = false;
        }
    }

    /// `CModelParticleEmitter::SetTransform` and `SetEntityVariable` of the
    /// emitter `model` (a `Shape::Particles` model's address) of `owner`.
    pub fn place(
        &mut self,
        owner: Owner,
        model: usize,
        kind: &Rc<Emitter>,
        transform: &Mat4,
        variables: Vec<(String, f32)>,
    ) {
        let time = self.time;
        let emitter = self
            .emitters
            .entry((owner, model))
            .or_insert_with(|| Instance::new(kind, time));
        emitter.placed = true;
        emitter.active = true;
        emitter.position = transform.transform(Vec3::new(0.0, 0.0, 0.0));
        emitter.transform = *transform;
        emitter.variables = variables;
    }

    /// Runs the emitters this frame draws: the placed ones, and those not
    /// placed that `in_view` keeps, or whose owner `alive` has lost, which
    /// stop emitting. Drops those left without a living particle.
    pub fn finish(&mut self, in_view: impl Fn(Vec3, f32) -> bool, alive: impl Fn(Owner) -> bool) {
        let time = self.time;
        let rng = &mut self.rng;
        self.emitters.retain(|(owner, _), emitter| {
            if !emitter.placed {
                emitter.active = false;
                if alive(*owner) && !in_view(emitter.position, emitter.reach) {
                    emitter.drawn = false;
                    return true;
                }
            }
            let dt = (time - emitter.last_update) as f32;
            emitter.update(dt, rng);
            emitter.last_update = time;
            emitter.drawn = true;
            emitter.placed || emitter.particles.iter().any(Particle::alive)
        });
    }

    /// The instances of every emitter drawn, back to front by emitter as
    /// `ParticleRenderer::PrepareForRendering` sorts them, and the draws
    /// into them.
    pub fn instances(&self, eye: [f32; 3]) -> (Vec<u8>, Vec<Draw>) {
        let distance = |position: Vec3| {
            let [x, y, z] = eye;
            (position + -Vec3::new(x, y, z)).length()
        };
        let mut drawn: Vec<(f32, &Instance)> = self
            .emitters
            .values()
            .filter(|emitter| emitter.drawn && emitter.kind.texture.is_some())
            .map(|emitter| (distance(emitter.position), emitter))
            .collect();
        drawn.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut bytes = Vec::new();
        let mut draws = Vec::new();
        for (_, emitter) in drawn {
            let Some(texture) = emitter.kind.texture else {
                continue;
            };
            // RGB times alpha where the blend reads the source colour, as
            // `UpdateArrayData` does.
            let premultiply = matches!(
                emitter.kind.blend,
                files::BLEND_OVER | files::BLEND_MULTIPLY
            );
            let start = bytes.len() as u64;
            let mut count = 0;
            for particle in &emitter.particles {
                if !particle.alive() || particle.color[3] == 0 {
                    continue;
                }
                let half = particle.size / 2.0;
                let axis = [particle.angle.cos() * half, particle.angle.sin() * half];
                let mut color = particle.color;
                if premultiply {
                    let alpha = u32::from(color[3]);
                    for channel in &mut color[..3] {
                        *channel = (u32::from(*channel) * alpha / 255) as u8;
                    }
                }
                let Vec3 { x, y, z } = particle.position;
                for value in [x, y, z, axis[0], axis[1]] {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                bytes.extend_from_slice(&color);
                count += 1;
            }
            if count > 0 {
                draws.push(Draw {
                    texture,
                    blend: emitter.kind.blend,
                    start,
                    end: bytes.len() as u64,
                    count,
                });
            }
        }
        (bytes, draws)
    }
}

/// The four blend modes' pipelines, by `BLEND_*`, for one pass.
pub struct Pipelines([wgpu::RenderPipeline; 4]);

impl Pipelines {
    /// `reflection` mirrors the quads and clips them as model.wgsl's
    /// `WATER_CLIP` 2.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        view_layout: &wgpu::BindGroupLayout,
        texture_layout: &wgpu::BindGroupLayout,
        light_layout: &wgpu::BindGroupLayout,
        reflection: bool,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("particles.wgsl"),
                    include_str!("shared/los.wgsl"),
                    include_str!("shared/pass_fog.wgsl"),
                )
                .into(),
            ),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particles"),
            bind_group_layouts: &[Some(view_layout), Some(texture_layout), Some(light_layout)],
            immediate_size: 0,
        });
        let constants = [("WATER_CLIP", if reflection { 2.0 } else { 0.0 })];
        let pipeline = |blend: u8| {
            // shaders/effects/particle_*.xml.
            let component = |src, dst, operation| wgpu::BlendComponent {
                src_factor: src,
                dst_factor: dst,
                operation,
            };
            let component = match blend {
                files::BLEND_SUBTRACT => component(
                    wgpu::BlendFactor::SrcAlpha,
                    wgpu::BlendFactor::One,
                    wgpu::BlendOperation::ReverseSubtract,
                ),
                files::BLEND_OVER => component(
                    wgpu::BlendFactor::SrcAlpha,
                    wgpu::BlendFactor::OneMinusSrcAlpha,
                    wgpu::BlendOperation::Add,
                ),
                files::BLEND_MULTIPLY => component(
                    wgpu::BlendFactor::Zero,
                    wgpu::BlendFactor::OneMinusSrc,
                    wgpu::BlendOperation::Add,
                ),
                _ => component(
                    wgpu::BlendFactor::SrcAlpha,
                    wgpu::BlendFactor::One,
                    wgpu::BlendOperation::Add,
                ),
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("particles"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_particle"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &constants,
                        ..Default::default()
                    },
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: INSTANCE_SIZE as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3,
                            1 => Float32x2,
                            2 => Unorm8x4,
                        ],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                // Tested, never written, as the effects' `depth mask`.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_particle"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &constants,
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState {
                            color: component,
                            alpha: component,
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Pipelines(
            [
                files::BLEND_ADD,
                files::BLEND_SUBTRACT,
                files::BLEND_OVER,
                files::BLEND_MULTIPLY,
            ]
            .map(pipeline),
        )
    }

    /// Draws `draws` from `instances`; the view group is set, the light's
    /// is `light`. `group` gives an uploaded texture's bind group.
    pub fn draw<'a>(
        &self,
        pass: &mut wgpu::RenderPass,
        draws: &[Draw],
        instances: &wgpu::Buffer,
        light: &wgpu::BindGroup,
        group: impl Fn(FileRef) -> Option<&'a wgpu::BindGroup>,
    ) {
        if draws.is_empty() {
            return;
        }
        // As for the plain water: WebGL2 numbers uniform slots across
        // groups, and wgpu drops a repeat of a group it holds as
        // compatible, so the light is bound afresh.
        pass.set_bind_group(2, None, &[]);
        pass.set_bind_group(2, light, &[]);
        let mut current = None;
        for draw in draws {
            let Some(texture) = group(draw.texture) else {
                continue;
            };
            if current != Some(draw.blend) {
                pass.set_pipeline(&self.0[usize::from(draw.blend)]);
                current = Some(draw.blend);
            }
            pass.set_bind_group(1, texture, &[]);
            pass.set_vertex_buffer(0, instances.slice(draw.start..draw.end));
            pass.draw(0..4, 0..draw.count);
        }
    }
}
