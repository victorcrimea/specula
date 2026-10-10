// The observer page's particles (particles.rs): particle.vs and particle.fs,
// a quad per particle facing the camera, its texture tinted halfway
// towards the sun's colour and fogged. The observer sees everything, so
// the LOS darkening is left out.

// model.wgsl `View`, with the camera fields only particles read.
struct View {
    view_proj: mat4x4<f32>,
    sun: vec4<f32>,
    misc: vec4<f32>,
    // The water level, then unused.
    water: vec4<f32>,
    // The camera's right, up and position.
    camera_right: vec4<f32>,
    camera_up: vec4<f32>,
    eye: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

// model.wgsl `Light` up to the fog, all particles read.
struct Light {
    sun_color: vec4<f32>,
    ambient: vec4<f32>,
    shadow_matrix: mat4x4<f32>,
    // The camera, then the shadow cutoff distance in metres.
    eye: vec4<f32>,
    // The camera's forward, then one over the shadow map's size.
    forward: vec4<f32>,
    shadow: vec4<f32>,
    // CLightEnv::m_FogColor.
    fog_color: vec4<f32>,
    // The engine's fogParams: factor, then max, which is 1 with fog off.
    fog: vec4<f32>,
}

@group(2) @binding(0) var<uniform> light: Light;

// model.wgsl `WATER_CLIP`: 0 the view, 2 the mirrored reflection.
override WATER_CLIP: u32 = 0u;

// model.wgsl `pass_eye` and `pass_forward`.
fn pass_eye() -> vec3<f32> {
    if WATER_CLIP == 2u {
        return vec3<f32>(light.eye.x, 2.0 * view.water.x - light.eye.y, light.eye.z);
    }
    return light.eye.xyz;
}

fn pass_forward() -> vec3<f32> {
    if WATER_CLIP == 2u {
        return vec3<f32>(light.forward.x, -light.forward.y, light.forward.z);
    }
    return light.forward.xyz;
}

// common/fog.h applyFog, as model.wgsl's.
fn apply_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let w = dot(world - pass_eye(), pass_forward());
    let z = 4096.0 * (w - 2.0) / (4096.0 - 2.0);
    let density = light.fog.x;
    let max_fog = light.fog.y;
    let factor = exp2(-density * density * z * z * 1.442695) * (1.0 - max_fog) + max_fog;
    return mix(light.fog_color.rgb, color, clamp(factor, 0.0, 1.0));
}

struct ParticleVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
}

@vertex
fn vs_particle(
    @builtin(vertex_index) index: u32,
    @location(0) centre: vec3<f32>,
    // cos and sin of the particle's angle, times half its size.
    @location(1) axis: vec2<f32>,
    @location(2) color: vec4<f32>,
) -> ParticleVaryings {
    // A triangle strip over the engine's corners 0, 1, 3, 2, which go
    // round the quad.
    let corner = array<u32, 4>(0u, 1u, 3u, 2u)[index];
    let c = axis.x;
    let s = axis.y;
    let offsets = array<vec2<f32>, 4>(
        vec2<f32>(c, s),
        vec2<f32>(s, -c),
        vec2<f32>(-c, -s),
        vec2<f32>(-s, c),
    );
    let uvs = array<vec2<f32>, 4>(
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );
    let offset = offsets[corner];
    var right = view.camera_right.xyz;
    var up = view.camera_up.xyz;
    // The reflection camera's axes, which the mirror in view_proj turns
    // back into the view's, so the quads face it as they face the engine's
    // reflection camera.
    if WATER_CLIP == 2u {
        right = vec3<f32>(right.x, -right.y, right.z);
        up = vec3<f32>(up.x, -up.y, up.z);
    }
    let world = right * (offset.x + offset.y) + up * (offset.x - offset.y) + centre;
    var out: ParticleVaryings;
    out.position = view.view_proj * vec4<f32>(world, 1.0);
    out.uv = uvs[corner];
    out.color = color;
    out.world = world;
    return out;
}

@fragment
fn fs_particle(in: ParticleVaryings) -> @location(0) vec4<f32> {
    // Sampled before the discard, which would leave uniform control flow.
    let texel = textureSample(base_texture, base_sampler, in.uv);
    if WATER_CLIP == 2u && in.world.y < view.water.x - 0.5 {
        discard;
    }
    let color = texel * vec4<f32>((in.color.rgb + light.sun_color.rgb) / 2.0, in.color.a);
    return vec4<f32>(apply_fog(color.rgb, in.world), color.a);
}
