// The fancy water effects texture water_high reads: the shore waves as
// waves.vs and waves.fs draw them, their normal bend in r and b, their
// foam in g and their coverage in alpha, and then the shore tiles as
// RenderWaterFoamOccluders draws them, clearing the waves the ground hides.

struct View {
    view_proj: mat4x4<f32>,
}

// water.wgsl's `Water`, of which only the time is read here.
struct Water {
    color: vec4<f32>,
    tint: vec4<f32>,
    wave1: vec4<f32>,
    wave2: vec4<f32>,
    // time (m_WaterTexTimer), then the wind.
    motion: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var<uniform> water: Water;
// coastalWave.png and foam.png.
@group(1) @binding(1) var wave_texture: texture_2d<f32>;
@group(1) @binding(2) var foam_texture: texture_2d<f32>;
@group(1) @binding(3) var wave_sampler: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) ttime: f32,
    @location(1) normal: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) width: f32,
}

@vertex
fn vs_wave(
    @location(0) base: vec3<f32>,
    @location(1) apex: vec3<f32>,
    @location(2) splash: vec3<f32>,
    @location(3) retreat: vec3<f32>,
    @location(4) normal: vec2<f32>,
    @location(5) uv: vec2<f32>,
    // The wave's `translation` and `width`.
    @location(6) wave: vec2<f32>,
) -> Varyings {
    let ttime = (water.motion.x + wave.x) % 10.0;
    var position = mix(base, apex, clamp(ttime / 3.0, 0.0, 1.0));
    position = mix(position, splash, clamp(sin((min(ttime, 6.1415926536) - 3.0) / 2.0), 0.0, 1.0));
    position = mix(position, retreat,
        clamp(1.0 - cos(max(0.0, ttime - 6.1415926536) / 2.0), 0.0, 1.0));
    var out: Varyings;
    out.position = view.view_proj * vec4<f32>(position, 1.0);
    out.ttime = ttime;
    out.normal = normal;
    out.uv = uv;
    out.width = wave.y;
    return out;
}

@fragment
fn fs_wave(in: Varyings) -> @location(0) vec4<f32> {
    var tex = textureSample(wave_texture, wave_sampler, -in.uv / 8.0).rbga;
    let foam_first = textureSample(foam_texture, wave_sampler,
        -in.uv / vec2<f32>(2.5, 7.0) + vec2<f32>(0.05, -0.3) * -cos(in.ttime / 2.0)).rbg;
    let foam_second = textureSample(foam_texture, wave_sampler,
        -in.uv / 5.0 + vec2<f32>(0.8, -0.8) + vec2<f32>(-0.05, -0.25) * -cos(in.ttime / 2.0) * 1.2).rbg;

    tex = vec4<f32>(tex.rgb - vec3<f32>(0.5, 0.0, 0.5), tex.a);
    tex = vec4<f32>(-tex.r, tex.g, -tex.b, tex.a);

    let half_width = (in.width - 1.0) / 2.0;
    var force_alpha = min(1.0, 2.0 - abs(in.uv.x - half_width) / (half_width / 2.0));
    force_alpha *= min(1.0, in.uv.y);

    var time_alpha = 1.0 - clamp((in.ttime - 6.0) / 4.0, 0.0, 1.0);
    time_alpha *= clamp(in.ttime / 2.0, 0.0, 1.0);

    let foam_alpha = tex.a * force_alpha * time_alpha;
    let norm = tex.rb;
    // The engine's own formula, normal.x twice.
    let r = norm.x * in.normal.x - norm.y * in.normal.x;
    let b = norm.x * in.normal.y + norm.y * in.normal.y;
    let foam = foam_first * foam_second;
    let g = foam_alpha * clamp(foam.r * 3.0, 0.0, 1.0) * 0.4;
    return vec4<f32>(r, g, b, force_alpha * time_alpha);
}

@vertex
fn vs_occluder(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return view.view_proj * vec4<f32>(position, 1.0);
}

// The solid shader with a colour of 0.
@fragment
fn fs_occluder() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
