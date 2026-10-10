// The observer page's models, skinned on the GPU from the pose atlas
// (pose.rs): per influence, the bone's rotation and translation at the
// instance's two frames, already composed with the inverse bind pose.
// Also the capture of models into impostor atlases (far.rs, far.wgsl).

struct View {
    view_proj: mat4x4<f32>,
    // xyz towards the sun.
    sun: vec4<f32>,
    // model_common.vs' sim_time in seconds, 1 when the wind sways the
    // models, map size in metres, 1 on a circular map.
    misc: vec4<f32>,
    // Water level in metres.
    water: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var poses: texture_2d<f32>;
@group(2) @binding(0) var skin: texture_2d<f32>;
@group(2) @binding(1) var skin_sampler: sampler;
@group(2) @binding(2) var normal_map: texture_2d<f32>;
@group(2) @binding(3) var specular_map: texture_2d<f32>;
@group(2) @binding(4) var ao_map: texture_2d<f32>;

struct Material {
    // effectSettings: normal strength, specular power, parallax scale,
    // the vertex lighting's share of the sun.
    effect: vec4<f32>,
    // 1 when the material has a normal map, 1 when it sways in the wind
    // (USE_WIND), then its windData.
    flags: vec4<f32>,
}

@group(2) @binding(5) var<uniform> material: Material;

// The viewer's material maps setting: off leaves the normal, specular and
// ambient occlusion maps unsampled, for the cost of the plain shading.
override MAPS: bool = true;

struct Light {
    sun_color: vec4<f32>,
    ambient: vec4<f32>,
    // World to the shadow map: texels in xy, depth in z (shadow.rs).
    shadow_matrix: mat4x4<f32>,
    // The camera, then the shadow cutoff distance in metres.
    eye: vec4<f32>,
    // The camera's forward, then one over the shadow map's size.
    forward: vec4<f32>,
    // Depth bias for models, for the ground, 1 when shadows are on, then
    // the number of cascades.
    shadow: vec4<f32>,
    // CLightEnv::m_FogColor.
    fog_color: vec4<f32>,
    // The engine's fogParams: factor, then max, which is 1 with fog off.
    fog: vec4<f32>,
    // Very High's cascades: world to the map's texels in each one's quarter
    // (shadow.rs), then where each ends, in metres along the view.
    cascades: array<mat4x4<f32>, 4>,
    cascade_ends: vec4<f32>,
}

@group(3) @binding(0) var<uniform> light: Light;
@group(3) @binding(1) var shadow_map: texture_depth_2d;
@group(3) @binding(2) var shadow_sampler: sampler_comparison;

// Which side of the water this pass keeps, as the engine's water cameras
// clip (SceneRenderer's oblique near planes): 0 everything, for the view;
// 1 what is under the water, for the refraction; 2 what is over it, for
// the mirrored reflection.
override WATER_CLIP: u32 = 0u;

// The camera this pass sees from, for fog and highlights: the view's, or
// for the reflection its mirror image under the water, so that a
// reflected fragment is as far and lit from where the engine's
// reflection camera sees it.
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

// common/fog.h applyFog. Its depth, gl_FragCoord.z / gl_FragCoord.w, is
// the GL window depth times clip w under the engine's default planes (2
// and 4096 m), rebuilt here from the view depth so a map's fog factor
// reaches as far as in the game.
fn apply_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let w = dot(world - pass_eye(), pass_forward());
    let z = 4096.0 * (w - 2.0) / (4096.0 - 2.0);
    let density = light.fog.x;
    let max_fog = light.fog.y;
    let factor = exp2(-density * density * z * z * 1.442695) * (1.0 - max_fog) + max_fog;
    return mix(light.fog_color.rgb, color, clamp(factor, 0.0, 1.0));
}

// shadows_fragment.h getShadowWithFade: faded out over the last 8 m before
// the cutoff, or with Very High's cascades from the first that reaches the
// point, blended into the next over the last eighth of its reach. The
// Level form: it may follow a discard.
fn shadow_at(world: vec3<f32>, bias: f32) -> f32 {
    if light.shadow.z < 0.5 {
        return 1.0;
    }
    let depth = dot(world - light.eye.xyz, light.forward.xyz);
    if light.shadow.w < 1.5 {
        let fade = clamp((light.eye.w - depth - 8.0) / 8.0, 0.0, 1.0);
        if fade <= 0.0 {
            return 1.0;
        }
        return mix(1.0, shadow_in(light.shadow_matrix, world, bias), fade);
    }
    var cascade = 0;
    while cascade < 4 && depth > light.cascade_ends[cascade] {
        cascade += 1;
    }
    if cascade == 4 {
        return 1.0;
    }
    let end = light.cascade_ends[cascade];
    let shadow = shadow_in(light.cascades[cascade], world, bias);
    let width = clamp(end / 8.0, 2.0, 32.0);
    let blend = clamp((end - depth - width) / width, 0.0, 1.0);
    if cascade == 3 {
        return mix(1.0, shadow, blend);
    }
    if blend >= 1.0 {
        return shadow;
    }
    return mix(shadow_in(light.cascades[cascade + 1], world, bias), shadow, blend);
}

// shadows_fragment.h getShadowImpl with its PCF: four bilinear comparisons
// weighted into a tent over 3x3 texels.
fn shadow_in(matrix: mat4x4<f32>, world: vec3<f32>, bias: f32) -> f32 {
    let projected = matrix * vec4<f32>(world, 1.0);
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
    return dot(size.zxzx * size.wwyy, taps) / 9.0;
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
    @location(5) uv2: vec2<f32>,
    @location(6) tangent: vec3<f32>,
    @location(7) bitangent: vec3<f32>,
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

// model_common.vs fakeCos: a smoothed triangle wave.
fn fake_cos(x: vec3<f32>) -> vec3<f32> {
    let tri = abs(fract(x + 0.5) * 2.0 - 1.0);
    return tri * tri * (3.0 - 2.0 * tri);
}

// model_common.vs USE_WIND: `world` is the vertex after the instance
// transform, `local` the mesh's own vertex (a_vertex), `translation` the
// instance's. Each instance's fractional position offsets its phase, so
// a forest does not sway in step.
fn sway(world: vec3<f32>, local: vec3<f32>, translation: vec3<f32>) -> vec3<f32> {
    let wind = material.flags.zw;
    let time = view.misc.x;
    let model_pos = clamp(fract(translation), vec3<f32>(0.4), vec3<f32>(1.0));
    let abs_wind = abs(wind.x) + abs(wind.y);
    let cos_vec = fake_cos(vec3<f32>(
        time * model_pos.x + world.x,
        time * model_pos.z / 3.0 + translation.x,
        time * abs_wind / 4.0 + world.z,
    ));
    let limit = clamp(local.x * local.z * local.y / 3000.0, 0.0, 0.2);
    let diff = cos_vec.x * limit;
    let diff2 = cos_vec.y * clamp(local.y / 60.0, 0.0, 0.25);
    var swayed = world + cos_vec.z * limit * clamp(abs_wind, 1.2, 1.7);
    swayed.x += diff + diff2 * wind.x;
    swayed.z += diff + diff2 * wind.y;
    return swayed;
}

@vertex
fn vs_model(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bones: vec4<u32>,
    @location(4) weights: vec4<f32>,
    @location(10) uv2: vec2<f32>,
    // The bitangent's sign in w.
    @location(11) tangent: vec4<f32>,
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
    var local_tangent = tangent.xyz;
    let bone_count = pose.z;
    if bone_count > 0u {
        local = vec3<f32>(0.0);
        local_normal = vec3<f32>(0.0);
        local_tangent = vec3<f32>(0.0);
        for (var i = 0u; i < 4u; i++) {
            let weight = weights[i];
            if weight <= 0.0 {
                continue;
            }
            // Index `bone_count` is the bind shape: the model itself.
            if bones[i] >= bone_count {
                local += position * weight;
                local_normal += normal * weight;
                local_tangent += tangent.xyz * weight;
                continue;
            }
            let m = bone_pose(pose.x, pose.y, params.x, bones[i]);
            local += (m * vec4<f32>(position, 1.0)) * weight;
            local_normal += (m * vec4<f32>(normal, 0.0)) * weight;
            local_tangent += (m * vec4<f32>(tangent.xyz, 0.0)) * weight;
        }
    }
    var world = vec3<f32>(
        dot(row0, vec4<f32>(local, 1.0)),
        dot(row1, vec4<f32>(local, 1.0)),
        dot(row2, vec4<f32>(local, 1.0)),
    );
    if material.flags.y > 0.5 && view.misc.y > 0.5 {
        world = sway(world, position, vec3<f32>(row0.w, row1.w, row2.w));
    }
    let world_normal = vec3<f32>(
        dot(row0.xyz, local_normal),
        dot(row1.xyz, local_normal),
        dot(row2.xyz, local_normal),
    );
    let world_tangent = vec3<f32>(
        dot(row0.xyz, local_tangent),
        dot(row1.xyz, local_tangent),
        dot(row2.xyz, local_tangent),
    );
    var out: ModelVaryings;
    out.position = view.view_proj * vec4<f32>(world, 1.0);
    out.uv = uv;
    out.normal = world_normal;
    out.world = world;
    out.uv2 = uv2;
    // model_common.vs: the bitangent from the transformed normal and
    // tangent, flipped where the UVs are mirrored.
    out.tangent = world_tangent;
    out.bitangent = cross(world_normal, world_tangent) * tangent.w;
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

// model_common.fs with shading.h: the alpha of a player-coloured skin
// masks the tint; the normal map bends the normal, the specular map adds
// the sun's highlight and the ambient occlusion map darkens only the
// ambient light. A missing specular or occlusion map is black or white.
@fragment
fn fs_model(in: ModelVaryings) -> @location(0) vec4<f32> {
    // Sampled before the discard: WGSL wants textureSample in uniform
    // control flow.
    let tex = textureSample(skin, skin_sampler, in.uv);
    var packed_normal = vec3<f32>(0.5, 0.5, 1.0);
    var specular_color = vec3<f32>(0.0);
    var ao = 1.0;
    if MAPS {
        packed_normal = textureSample(normal_map, skin_sampler, in.uv).rgb;
        specular_color = textureSample(specular_map, skin_sampler, in.uv).rgb;
        ao = textureSample(ao_map, skin_sampler, in.uv2).r;
    }
    let clipped = (WATER_CLIP == 1u && in.world.y > view.water.x + 2.0)
        || (WATER_CLIP == 2u && in.world.y < view.water.x - 0.5);
    if tested_alpha(tex.a, in.uv) < in.alpha_ref || clipped {
        discard;
    }
    let tint = mix(in.color.rgb, vec3<f32>(1.0), tex.a);
    let diffuse = tex.rgb * mix(vec3<f32>(1.0), tint, in.color.a);

    var normal = normalize(in.normal);
    var sun_diffuse = max(dot(normal, view.sun.xyz), 0.0) * light.sun_color.rgb;
    if MAPS && material.flags.x > 0.5 {
        // calculateNormal.
        var local = packed_normal * 2.0 - 1.0;
        local.y = -local.y;
        var mapped = normalize(mat3x3<f32>(in.tangent, in.bitangent, in.normal) * local);
        let strength = material.effect.x;
        if strength <= 1.0 {
            mapped = normalize(mix(normal, mapped, strength));
        } else {
            mapped = normalize(mapped - normal * min(1.0, strength - 1.0));
        }
        let mapped_sun = max(dot(mapped, view.sun.xyz), 0.0) * light.sun_color.rgb;
        sun_diffuse = mix(mapped_sun, sun_diffuse, material.effect.w);
        normal = mapped;
    }
    var specular = vec3<f32>(0.0);
    if MAPS {
        // calculateSpecular, with model_common.vs' half vector.
        let half_vector = normalize(view.sun.xyz + normalize(pass_eye() - in.world));
        specular = light.sun_color.rgb * specular_color
            * pow(max(0.001, dot(normal, half_vector)), material.effect.y);
    }
    // calculateShading.
    let shadow = shadow_at(in.world, light.shadow.x);
    let color = (diffuse * sun_diffuse + specular) * shadow + diffuse * light.ambient.rgb * ao;
    return vec4<f32>(apply_fog(color, in.world), 1.0);
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
