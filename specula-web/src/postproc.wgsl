// The observer page's map post-processing (postproc.rs), as
// CPostprocManager runs it: the bloom.fs blur levels, then the map's
// effect, dof.fs and hdr.fs.

// BLOOM_PASS_H's and BLOOM_PASS_V's taps sum the scene's bloom term in
// hdr.fs; off, it is left out, as a bloom of 0.2 would.
override BLOOM: bool = true;

struct Post {
    brightness: f32,
    // CLightEnv::m_Contrast, hdr.fs' `hdr`.
    contrast: f32,
    saturation: f32,
    bloom: f32,
    // The view's planes, in metres.
    near: f32,
    far: f32,
    // WebGL2 needs uniform structs sized in multiples of 16 bytes.
    _pad: vec2<f32>,
}

// The blur passes' input, in its own group: an entry point may not see
// two resources at one binding.
@group(1) @binding(0) var source: texture_2d<f32>;
@group(1) @binding(1) var source_sampler: sampler;

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var rendered: texture_2d<f32>;
@group(0) @binding(2) var blur2: texture_2d<f32>;
@group(0) @binding(3) var blur4: texture_2d<f32>;
@group(0) @binding(4) var blur8: texture_2d<f32>;
@group(0) @binding(5) var depth: texture_2d<f32>;
@group(0) @binding(6) var effect_sampler: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    // v_tex, with v = 0 at the top as the engine's fullscreen quad has it
    // on Vulkan.
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> Varyings {
    // One triangle over the whole target: (-1,-1) (3,-1) (-1,3).
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0;
    var out: Varyings;
    out.position = vec4<f32>(corner, 0.0, 1.0);
    out.uv = vec2<f32>(corner.x + 1.0, 1.0 - corner.y) * 0.5;
    return out;
}

// BLOOM_NOP: one bilinear tap of the full-size scene, for every level, as
// ApplyBlur never feeds a level the one above it.
@fragment
fn fs_downscale(in: Varyings) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(source, source_sampler, in.uv).rgb, 1.0);
}

// BLOOM_PASS_H and BLOOM_PASS_V: six equal taps 0.004 apart in texture
// coordinates, whatever the level's size.
fn box_blur(uv: vec2<f32>, step: vec2<f32>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    var at = uv - step * 2.5;
    for (var i = 0; i < 6; i++) {
        color += textureSample(source, source_sampler, at);
        at += step;
    }
    return vec4<f32>(color.rgb / 6.0, 1.0);
}

@fragment
fn fs_blur_h(in: Varyings) -> @location(0) vec4<f32> {
    return box_blur(in.uv, vec2<f32>(0.004, 0.0));
}

@fragment
fn fs_blur_v(in: Varyings) -> @location(0) vec4<f32> {
    return box_blur(in.uv, vec2<f32>(0.0, 0.004));
}

// The view depth, which dof.fs' linearizeDepth gives from GL's depth, from
// this page's [0, 1] one.
fn view_depth(uv: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(depth));
    let texel = min(vec2<i32>(uv * size), vec2<i32>(size) - 1);
    let d = textureLoad(depth, texel, 0).r;
    return post.near * post.far / (post.far - d * (post.far - post.near));
}

// dof.fs, with DOF.xml's BLUR_FOV of 3: the farther a pixel's depth from
// the average around the screen's centre, the blurrier the level it takes.
@fragment
fn fs_dof(in: Varyings) -> @location(0) vec4<f32> {
    var color = textureSample(rendered, effect_sampler, in.uv).rgb;
    let b2 = textureSample(blur2, effect_sampler, in.uv).rgb;
    let b4 = textureSample(blur4, effect_sampler, in.uv).rgb;
    let b8 = textureSample(blur8, effect_sampler, in.uv).rgb;
    // dof.fs averages the raw depths before linearizing them.
    let size = vec2<f32>(textureDimensions(depth));
    var mid = 0.0;
    for (var i = 0; i < 5; i++) {
        let offset = select(vec2<f32>(0.0), vec2<f32>(f32(i & 1), f32((i >> 1u) & 1)) * 0.2 - 0.1, i < 4);
        let texel = min(vec2<i32>((vec2<f32>(0.5) + offset) * size), vec2<i32>(size) - 1);
        mid += textureLoad(depth, texel, 0).r;
    }
    mid /= 5.0;
    let linear_mid = post.near * post.far / (post.far - mid * (post.far - post.near));
    let amount = clamp(abs(view_depth(in.uv) - linear_mid) / (linear_mid * 3.0), 0.0, 1.0);
    if amount < 0.25 {
        color = mix(color, b2, amount / 0.25);
    } else if amount < 0.5 {
        color = mix(b2, b4, (amount - 0.25) / 0.25);
    } else if amount < 0.75 {
        color = mix(b4, b8, (amount - 0.5) / 0.25);
    } else {
        color = b8;
    }
    return vec4<f32>(color, 1.0);
}

// hdr.fs. Unclamped, as there: the 8-bit target clamps.
@fragment
fn fs_hdr(in: Varyings) -> @location(0) vec4<f32> {
    var color = textureSample(rendered, effect_sampler, in.uv).rgb;
    if BLOOM {
        let b2 = textureSample(blur2, effect_sampler, in.uv).rgb;
        let b4 = textureSample(blur4, effect_sampler, in.uv).rgb;
        let b8 = textureSample(blur8, effect_sampler, in.uv).rgb;
        let bloom = post.bloom;
        var glow = (max(b2 - bloom, vec3<f32>(0.0)) + max(b4 - bloom, vec3<f32>(0.0))
            + max(b8 - bloom, vec3<f32>(0.0))) / 3.0;
        glow = mix(glow, color, bloom / 0.2);
        color = max(glow, color);
    }
    color += vec3<f32>(post.brightness);
    color = (color - 0.5) * post.contrast + 0.5;
    color = mix(vec3<f32>(dot(color, vec3<f32>(0.299, 0.587, 0.114))), color, post.saturation);
    return vec4<f32>(color, 1.0);
}
