// The water as the engine's water_high effect draws it (water_high.vs and
// water_high.fs): two animated normal maps bent by the wind, the sky's
// reflection weighted by fresnel, the sun's glint, and the map's colour
// and tint by depth and murkiness. Without the LOS, as an observer sees
// everything.

struct View {
    view_proj: mat4x4<f32>,
    // xyz towards the sun.
    sun: vec4<f32>,
    // Unused, unused, map size in metres, 1 on a circular map.
    misc: vec4<f32>,
}

// WaterManager's settings and the uniforms TerrainRenderer::RenderFancyWater
// sets from them.
struct Water {
    // m_WaterColor, then m_Waviness.
    color: vec4<f32>,
    // m_WaterTint, then m_Murkiness.
    tint: vec4<f32>,
    // waveParams1: wavyEffect, baseScale, flattenism, baseBump.
    wave1: vec4<f32>,
    // waveParams2: smallIntensity, smallBase, bigMovement, smallMovement.
    wave2: vec4<f32>,
    // time (m_WaterTexTimer), cos and sin of -m_WindAngle, unused.
    motion: vec4<f32>,
    // The current and next normal map frame, cos and sin of the sky's
    // rotation (skyBoxRot).
    frames: vec4<f32>,
}

struct Light {
    sun_color: vec4<f32>,
    ambient: vec4<f32>,
    shadow_matrix: mat4x4<f32>,
    // The camera, then the shadow cutoff distance in metres.
    eye: vec4<f32>,
    // The camera's forward, then one over the shadow map's size.
    forward: vec4<f32>,
    // Depth bias for models, for the ground, then 1 when shadows are on.
    shadow: vec4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;

@group(1) @binding(0) var<uniform> water: Water;
// m_NormalMap's 60 frames.
@group(1) @binding(1) var normal_maps: texture_2d_array<f32>;
@group(1) @binding(2) var normal_sampler: sampler;

// SkyManager's cube, black while it has none.
@group(2) @binding(0) var sky_cube: texture_cube<f32>;
@group(2) @binding(1) var sky_sampler: sampler;

@group(3) @binding(0) var<uniform> light: Light;
@group(3) @binding(1) var shadow_map: texture_depth_2d;
@group(3) @binding(2) var shadow_sampler: sampler_comparison;

// WaterManager's m_RepeatPeriod.
const REPEAT_SCALE: f32 = 1.0 / 16.0;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) world: vec3<f32>,
    // Wind strength, depth (a_waterInfo).
    @location(1) water_info: vec2<f32>,
    @location(2) normal_coords: vec2<f32>,
    @location(3) waviness: f32,
}

@vertex
fn vs_water(
    @location(0) position: vec3<f32>,
    @location(1) water_info: vec2<f32>,
) -> Varyings {
    let wind = water.motion.yz;
    var out: Varyings;
    out.position = view.view_proj * vec4<f32>(position, 1.0);
    out.world = position;
    out.water_info = water_info;
    out.normal_coords = vec2<f32>(
        position.x * wind.x - position.z * wind.y,
        position.x * wind.y + position.z * wind.x,
    ) * REPEAT_SCALE;
    out.waviness = water.color.w * (0.15 + water_info.x / 1.15);
    return out;
}

// common/fog.h applyFog, as field.wgsl rebuilds it.
fn apply_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let w = dot(world - light.eye.xyz, light.forward.xyz);
    let z = 4096.0 * (w - 2.0) / (4096.0 - 2.0);
    let density = light.fog.x;
    let max_fog = light.fog.y;
    let factor = exp2(-density * density * z * z * 1.442695) * (1.0 - max_fog) + max_fog;
    return mix(light.fog_color.rgb, color, clamp(factor, 0.0, 1.0));
}

// field.wgsl's shadow_at, the engine's getShadowOnLandscape.
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

// The engine's moddedTime: how far between the two frames.
fn frame_blend() -> f32 {
    return (water.motion.x * 60.0 % 8.0) / 8.0;
}

// getNormal without the fancy effects.
fn water_normal(in: Varyings) -> vec3<f32> {
    let waviness = water.color.w;
    let wavy_effect = water.wave1.x;
    let base_scale = water.wave1.y;
    let flattenism = water.wave1.z;
    let base_bump = water.wave1.w;
    let big_movement = water.wave2.z;
    // normalCoords.zw is (time, 0).
    let coords = (in.normal_coords + vec2<f32>(water.motion.x, 0.0) * big_movement * waviness / 10.0)
        * (base_scale - waviness / wavy_effect);
    let frame = i32(water.frames.x);
    let next = i32(water.frames.y);
    let first = textureSample(normal_maps, normal_sampler, coords, frame).xzy;
    let second = textureSample(normal_maps, normal_sampler, coords, next).xzy;
    let interpolated = mix(first, second, frame_blend()) - vec3<f32>(0.5, 0.0, 0.5);
    let wind = water.motion.yz;
    let bent = vec3<f32>(
        interpolated.x * wind.x - interpolated.z * wind.y,
        interpolated.y,
        interpolated.x * wind.y + interpolated.z * wind.x,
    );
    var normal = normalize(mix(vec3<f32>(0.0, 1.0, 0.0), bent,
        clamp(base_bump + in.waviness / flattenism, 0.0, 1.0)));
    normal = mix(vec3<f32>(0.0, 1.0, 0.0), normal, 0.5 + in.water_info.x / 2.0);
    return vec3<f32>(-normal.x, normal.y, -normal.z);
}

// The sky cube in direction `direction`, as `vec4(direction, 0.0) *
// skyBoxRot` turns it: by the inverse of the sky's rotation.
fn sky(direction: vec3<f32>) -> vec3<f32> {
    let c = water.frames.z;
    let s = water.frames.w;
    let turned = vec3<f32>(
        c * direction.x - s * direction.z,
        direction.y,
        s * direction.x + c * direction.z,
    );
    return textureSampleLevel(sky_cube, sky_sampler, turned, 0.0).rgb;
}

@fragment
fn fs_water(in: Varyings) -> @location(0) vec4<f32> {
    // Sampled before the discard: WGSL wants textureSample in uniform
    // control flow.
    let normal = water_normal(in);
    let size = view.misc.z;
    if view.misc.w > 0.5 && distance(in.world.xz, vec2<f32>(size * 0.5)) > size * 0.5 {
        discard;
    }
    let eye = normalize(light.eye.xyz - in.world);

    // getRefraction without refraction: the depth along the view from the
    // vertex depth, and the extinction by murkiness.
    let color = water.color.rgb;
    let murkiness = water.tint.w;
    let depth = in.water_info.y / (min(0.5, eye.y) * 1.5 * min(0.5, eye.y) * 2.0);
    let fixed_vy = max(eye.y, 0.01);
    let murky = mix(200.0, 0.1, pow(murkiness, 0.25));
    let extinction = max(0.0, 1.0 - depth * fixed_vy / murky);
    let refracted = mix(color, mix(color * water.tint.rgb, color, extinction), extinction);
    let alpha = (1.4 - extinction) * clamp(depth, 0.0, 1.0);

    // getReflection without the reflection texture.
    let reflected = sky(reflect(eye, normal));
    let reflection_weight = 0.75;

    let ndotv = clamp(dot(normal, eye), 0.0, 1.0);
    var fresnel = clamp(pow(1.1 - ndotv, 2.0) * 1.5, 0.1, 0.75);

    // getSpecular; the engine's sunDir points from the sun.
    let specular_vector = reflect(-view.sun.xyz, normal);
    let specular_intensity = smoothstep(0.6, 1.0, pow(max(dot(specular_vector, eye), 0.0), 100.0)) * 1.2;
    let specular = clamp(specular_intensity * light.sun_color.rgb, vec3<f32>(0.0), vec3<f32>(1.0));

    let shadow = shadow_at(in.world, light.shadow.y);
    fresnel = mix(fresnel, fresnel * shadow, 0.05 + murkiness * 0.2);

    var out = mix(refracted, reflected, fresnel * reflection_weight);
    out += shadow * specular;
    out = clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(apply_fog(out, in.world), alpha);
}
