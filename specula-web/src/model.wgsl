// The observer page's models, skinned on the GPU from the pose atlas
// (pose.rs): per influence, the bone's rotation and translation at the
// instance's two frames, already composed with the inverse bind pose.
// Also the capture of models into impostor atlases (far.rs, far.wgsl).

struct View {
    view_proj: mat4x4<f32>,
    // xyz towards the sun.
    sun: vec4<f32>,
    // Unused, unused, map size in metres, 1 on a circular map.
    misc: vec4<f32>,
    // Water level in metres.
    water: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var poses: texture_2d<f32>;
@group(2) @binding(0) var skin: texture_2d<f32>;
@group(2) @binding(1) var skin_sampler: sampler;

struct Light {
    sun_color: vec4<f32>,
    ambient: vec4<f32>,
    // World to the shadow map: texels in xy, depth in z (shadow.rs).
    shadow_matrix: mat4x4<f32>,
    // The camera, then the shadow cutoff distance in metres.
    eye: vec4<f32>,
    // The camera's forward, then one over the shadow map's size.
    forward: vec4<f32>,
    // Depth bias for models, for the ground, then 1 when shadows are on.
    shadow: vec4<f32>,
}

@group(3) @binding(0) var<uniform> light: Light;
@group(3) @binding(1) var shadow_map: texture_depth_2d;
@group(3) @binding(2) var shadow_sampler: sampler_comparison;

// shadows_fragment.h getShadowWithFade with its PCF: four bilinear
// comparisons weighted into a tent over 3x3 texels, faded out over the
// last 8 m before the cutoff. The Level form: it may follow a discard.
fn shadow_at(world: vec3<f32>, bias: f32) -> f32 {
    if light.shadow.z < 0.5 {
        return 1.0;
    }
    let depth = dot(world - light.eye.xyz, light.forward.xyz);
    let fade = clamp((light.eye.w - depth - 8.0) / 8.0, 0.0, 1.0);
    if fade <= 0.0 {
        return 1.0;
    }
    let projected = light.shadow_matrix * vec4<f32>(world, 1.0);
    let reference = projected.z - bias;
    let offset = fract(projected.xy - 0.5);
    let size = vec4<f32>(offset + 1.0, 2.0 - offset);
    let weight = (vec4<f32>(1.0, 1.0, -0.5, -0.5) + (projected.xy - 0.5 * offset).xyxy) * light.forward.w;
    let taps = vec4<f32>(
        textureSampleCompareLevel(shadow_map, shadow_sampler, weight.zw, reference),
        textureSampleCompareLevel(shadow_map, shadow_sampler, weight.xw, reference),
        textureSampleCompareLevel(shadow_map, shadow_sampler, weight.zy, reference),
        textureSampleCompareLevel(shadow_map, shadow_sampler, weight.xy, reference),
    );
    let shadow = dot(size.zxzx * size.wwyy, taps) / 9.0;
    return mix(1.0, shadow, fade);
}

// shading.h calculateShading, without specular and ambient occlusion.
fn lit(albedo: vec3<f32>, normal: vec3<f32>, shadow: f32) -> vec3<f32> {
    let sun = max(dot(normalize(normal), view.sun.xyz), 0.0);
    return albedo * (light.sun_color.rgb * sun * shadow + light.ambient.rgb);
}

// Texels per pose atlas row, pose.rs WIDTH.
const POSE_WIDTH: u32 = 2048u;

struct ModelVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    // Player colour, then 1 when tinted by it.
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) alpha_ref: f32,
    @location(4) world: vec3<f32>,
}

fn pose_texel(index: u32) -> vec4<f32> {
    return textureLoad(poses, vec2<i32>(i32(index % POSE_WIDTH), i32(index / POSE_WIDTH)), 0);
}

// A bone's rotation and translation between two frames. nlerp rather than
// the engine's slerp: the keys are a frame apart, where they agree.
fn bone_pose(a: u32, b: u32, blend: f32, bone: u32) -> mat4x3<f32> {
    let qa = pose_texel(a + bone * 2u);
    var qb = pose_texel(b + bone * 2u);
    if dot(qa, qb) < 0.0 {
        qb = -qb;
    }
    let q = normalize(mix(qa, qb, blend));
    let t = mix(pose_texel(a + bone * 2u + 1u).xyz, pose_texel(b + bone * 2u + 1u).xyz, blend);
    // CQuaternion::ToMatrix, as columns.
    let xx = q.x * q.x * 2.0;
    let xy = q.x * q.y * 2.0;
    let xz = q.x * q.z * 2.0;
    let yy = q.y * q.y * 2.0;
    let yz = q.y * q.z * 2.0;
    let zz = q.z * q.z * 2.0;
    let wx = q.w * q.x * 2.0;
    let wy = q.w * q.y * 2.0;
    let wz = q.w * q.z * 2.0;
    return mat4x3<f32>(
        vec3<f32>(1.0 - (yy + zz), xy + wz, xz - wy),
        vec3<f32>(xy - wz, 1.0 - (xx + zz), yz + wx),
        vec3<f32>(xz + wy, yz - wx, 1.0 - (xx + yy)),
        t,
    );
}

@vertex
fn vs_model(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bones: vec4<u32>,
    @location(4) weights: vec4<f32>,
    // The model's world transform, rows of a 3x4 matrix.
    @location(5) row0: vec4<f32>,
    @location(6) row1: vec4<f32>,
    @location(7) row2: vec4<f32>,
    // First texel of frame a, of frame b, bone count (0: not skinned),
    // player colour as RGBA8.
    @location(8) pose: vec4<u32>,
    // Blend from a to b, 1 when tinted by the player colour, alpha ref.
    @location(9) params: vec4<f32>,
) -> ModelVaryings {
    var local = position;
    var local_normal = normal;
    let bone_count = pose.z;
    if bone_count > 0u {
        local = vec3<f32>(0.0);
        local_normal = vec3<f32>(0.0);
        for (var i = 0u; i < 4u; i++) {
            let weight = weights[i];
            if weight <= 0.0 {
                continue;
            }
            // Index `bone_count` is the bind shape: the model itself.
            if bones[i] >= bone_count {
                local += position * weight;
                local_normal += normal * weight;
                continue;
            }
            let m = bone_pose(pose.x, pose.y, params.x, bones[i]);
            local += (m * vec4<f32>(position, 1.0)) * weight;
            local_normal += (m * vec4<f32>(normal, 0.0)) * weight;
        }
    }
    let world = vec3<f32>(
        dot(row0, vec4<f32>(local, 1.0)),
        dot(row1, vec4<f32>(local, 1.0)),
        dot(row2, vec4<f32>(local, 1.0)),
    );
    let world_normal = vec3<f32>(
        dot(row0.xyz, local_normal),
        dot(row1.xyz, local_normal),
        dot(row2.xyz, local_normal),
    );
    var out: ModelVaryings;
    out.position = view.view_proj * vec4<f32>(world, 1.0);
    out.uv = uv;
    out.normal = world_normal;
    out.world = world;
    // Unpacked by hand: WebGL2's GLSL ES 3.0 has no unpackUnorm4x8.
    let rgb = vec3<f32>(f32(pose.w & 255u), f32((pose.w >> 8u) & 255u), f32((pose.w >> 16u) & 255u));
    out.color = vec4<f32>(rgb / 255.0, params.y);
    out.alpha_ref = params.z;
    return out;
}

// The skin's mips average alpha, so a thin leaf or stalk falls under the
// alpha reference a level or two down and foliage thins out with distance;
// raising alpha with the mip level the sampler picks keeps its coverage
// (Ben Golus, "Anti-aliased Alpha Test").
fn tested_alpha(alpha: f32, uv: vec2<f32>) -> f32 {
    let texels = uv * vec2<f32>(textureDimensions(skin, 0));
    let dx = dpdx(texels);
    let dy = dpdy(texels);
    let level = max(0.5 * log2(max(dot(dx, dx), dot(dy, dy))), 0.0);
    return alpha * (1.0 + level * 0.25);
}

// model_common.fs: the alpha of a player-coloured skin masks the tint.
@fragment
fn fs_model(in: ModelVaryings) -> @location(0) vec4<f32> {
    let tex = textureSample(skin, skin_sampler, in.uv);
    if tested_alpha(tex.a, in.uv) < in.alpha_ref {
        discard;
    }
    let tint = mix(in.color.rgb, vec3<f32>(1.0), tex.a);
    let diffuse = tex.rgb * mix(vec3<f32>(1.0), tint, in.color.a);
    return vec4<f32>(lit(diffuse, in.normal, shadow_at(in.world, light.shadow.x)), 1.0);
}

// The shadow map's depth pass: only the alpha test, so leaves cast
// leaf-shaped shadows.
@fragment
fn fs_shadow(in: ModelVaryings) {
    let tex = textureSample(skin, skin_sampler, in.uv);
    if tested_alpha(tex.a, in.uv) < in.alpha_ref {
        discard;
    }
}

struct Captured {
    @location(0) albedo: vec4<f32>,
    // Octahedral model-space normal in rg, the player colour's weight in b.
    @location(1) normal: vec4<f32>,
}

fn sign_not_zero(v: vec2<f32>) -> vec2<f32> {
    return select(vec2<f32>(-1.0), vec2<f32>(1.0), v >= vec2<f32>(0.0));
}

// Two channels for a normal leave the third for the tint weight, so one
// atlas serves every player; downsample.wgsl averages all three.
fn octahedral(n: vec3<f32>) -> vec2<f32> {
    var p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if n.z < 0.0 {
        p = (1.0 - abs(p.yx)) * sign_not_zero(p);
    }
    return p * 0.5 + 0.5;
}

// The colour fs_model would show, apart from the tint and the light: the
// tint is `mix(1, player colour, weight)` times the albedo.
@fragment
fn fs_capture(in: ModelVaryings) -> Captured {
    let tex = textureSample(skin, skin_sampler, in.uv);
    if tested_alpha(tex.a, in.uv) < in.alpha_ref {
        discard;
    }
    var out: Captured;
    out.albedo = vec4<f32>(tex.rgb, 1.0);
    let weight = in.color.a * (1.0 - tex.a);
    out.normal = vec4<f32>(octahedral(normalize(in.normal)), weight, 1.0);
    return out;
}
