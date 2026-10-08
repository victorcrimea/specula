// The water as the engine's water_high effect draws it (water_high.vs and
// water_high.fs): two animated normal maps bent by the wind, the sky's
// reflection weighted by fresnel, the sun's glint, and the map's colour
// and tint by depth and murkiness, over the refraction of what is under
// it where that is drawn. Without the LOS, as an observer sees
// everything.
//
// The engine samples its refraction and reflection textures with GL's
// texture coordinates, v up; `sample_refraction`, `refraction_position`
// and `sample_reflection` turn them into WebGPU's.

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
    // refractionMatrix: the refraction camera's view-projection.
    refraction: mat4x4<f32>,
    // Its right, up and forward, then the projection's x and y scale, to
    // rebuild a position from its depth as projInvTransform and
    // viewInvTransform do.
    camera_right: vec4<f32>,
    camera_up: vec4<f32>,
    camera_forward: vec4<f32>,
    // Its near and far planes.
    planes: vec4<f32>,
    // reflectionMatrix: the mirrored reflection camera's view-projection.
    reflection: mat4x4<f32>,
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
// The refraction pass's colour and depth.
@group(2) @binding(2) var refraction_map: texture_2d<f32>;
@group(2) @binding(3) var refraction_depth: texture_2d<f32>;
@group(2) @binding(4) var refraction_sampler: sampler;
// The reflection pass's colour, sampled as the refraction's.
@group(2) @binding(5) var reflection_map: texture_2d<f32>;

@group(3) @binding(0) var<uniform> light: Light;
@group(3) @binding(1) var shadow_map: texture_depth_2d;
@group(3) @binding(2) var shadow_sampler: sampler_comparison;

// waterrefraction: the refraction texture rather than the water's colour
// under the surface.
override REFRACTION: bool = false;
// waterrealdepth: the depth under the surface from the refraction's depth
// rather than from the vertex depth.
override REAL_DEPTH: bool = false;

// waterreflection: the reflection texture of what is over the water, the
// sky where it holds nothing, rather than the sky alone.
override REFLECTION: bool = false;

// WaterManager's m_RepeatPeriod.
const REPEAT_SCALE: f32 = 1.0 / 16.0;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) world: vec3<f32>,
    // Wind strength, depth (a_waterInfo).
    @location(1) water_info: vec2<f32>,
    @location(2) normal_coords: vec2<f32>,
    @location(3) waviness: f32,
    // refractionCoords: clip x, y and w in the refraction camera.
    @location(4) refraction_coords: vec3<f32>,
    // reflectionCoords, likewise in the reflection camera.
    @location(5) reflection_coords: vec3<f32>,
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
    let refraction = water.refraction * vec4<f32>(position, 1.0);
    out.refraction_coords = refraction.xyw;
    let reflection = water.reflection * vec4<f32>(position, 1.0);
    out.reflection_coords = reflection.xyw;
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

// The refraction's colour at GL texture coordinates.
fn sample_refraction(gl_uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(refraction_map, refraction_sampler, vec2<f32>(gl_uv.x, 1.0 - gl_uv.y), 0.0);
}

// getWorldPositionFromRefractionDepth: the position the refraction's depth
// holds at GL texture coordinates, nearest texel, no higher than the
// water.
fn refraction_position(gl_uv: vec2<f32>, water_position: vec3<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(refraction_depth));
    let texel = clamp(vec2<i32>(vec2<f32>(gl_uv.x, 1.0 - gl_uv.y) * size), vec2<i32>(0), vec2<i32>(size) - 1);
    let depth = textureLoad(refraction_depth, texel, 0).x;
    let near = water.planes.x;
    let far = water.planes.y;
    let view_z = near * far / (far - depth * (far - near));
    let ndc = gl_uv * 2.0 - 1.0;
    let view_x = ndc.x * view_z / water.camera_right.w;
    let view_y = ndc.y * view_z / water.camera_up.w;
    var position = light.eye.xyz + view_x * water.camera_right.xyz + view_y * water.camera_up.xyz
        + view_z * water.camera_forward.xyz;
    position.y = min(position.y, water_position.y);
    return position;
}

// getRefraction without the fancy effects: the colour under the surface
// and the water's alpha.
fn refracted(in: Varyings, normal: vec3<f32>, eye: vec3<f32>) -> vec4<f32> {
    let color = water.color.rgb;
    let coords = in.refraction_coords;
    var depth: f32;
    if REFRACTION && REAL_DEPTH {
        let position = refraction_position(0.5 * coords.xy / coords.z + 0.5, in.world);
        depth = distance(position, in.world);
    } else {
        depth = in.water_info.y / (min(0.5, eye.y) * 1.5 * min(0.5, eye.y) * 2.0);
    }
    var under = color;
    if REFRACTION {
        var distortion = 0.5 + clamp(depth / 2.0, 0.0, 7.0);
        if REAL_DEPTH {
            let shifted = (0.5 * coords.xy - normal.xz * distortion) / coords.z + 0.5;
            let new_depth = distance(refraction_position(shifted, in.world), in.world);
            if depth > new_depth + 3.0 {
                distortion /= 2.0;
            } else {
                depth = new_depth;
            }
        }
        let uv = (0.5 * coords.xy - normal.xz * distortion) / coords.z + 0.5;
        under = sample_refraction(uv).rgb;
        let blur = (0.1 + clamp(normal.x, -0.1, 0.1)) / coords.z;
        // Taps on what the pass left clear weigh nothing.
        var blurred = vec4<f32>(under, 1.0);
        var tap = sample_refraction(uv + vec2<f32>(blur + normal.x, blur + normal.z));
        blurred += vec4<f32>(tap.rgb * tap.a, tap.a);
        tap = sample_refraction(uv + vec2<f32>(-blur, blur + normal.z));
        blurred += vec4<f32>(tap.rgb * tap.a, tap.a);
        tap = sample_refraction(uv + vec2<f32>(-blur, -blur + normal.x));
        blurred += vec4<f32>(tap.rgb * tap.a, tap.a);
        tap = sample_refraction(uv + vec2<f32>(blur + normal.z, -blur));
        blurred += vec4<f32>(tap.rgb * tap.a, tap.a);
        blurred /= blurred.a;
        let blur_factor = distortion / 7.0;
        under = (under + blurred.rgb * blur_factor) / (1.0 + blur_factor);
    }
    let murkiness = water.tint.w;
    let fixed_vy = max(eye.y, 0.01);
    let murky = mix(200.0, 0.1, pow(murkiness, 0.25));
    let extinction = max(0.0, 1.0 - depth * fixed_vy / murky);
    let tinted = mix(under * water.tint.rgb, under, extinction);
    var alpha = clamp(depth, 0.0, 1.0);
    if !REFRACTION {
        alpha = (1.4 - extinction) * alpha;
    }
    return vec4<f32>(mix(color, tinted, extinction), alpha);
}

// getReflection: what the water mirrors, and how much of it the fresnel
// term may take.
fn reflected(in: Varyings, normal: vec3<f32>, eye: vec3<f32>) -> vec4<f32> {
    let sky_color = sky(reflect(eye, normal));
    if !REFLECTION {
        return vec4<f32>(sky_color, 0.75);
    }
    let vy = clamp(eye.y * 2.0, 0.05, 1.0);
    let coords = in.reflection_coords;
    let uv = (0.5 * coords.xy - 15.0 * normal.zx / vy) / coords.z + 0.5;
    let texel = textureSampleLevel(reflection_map, refraction_sampler, vec2<f32>(uv.x, 1.0 - uv.y), 0.0);
    var color = texel.rgb;
    if texel.a < 0.4 {
        color = mix(sky_color, texel.rgb, texel.a);
    }
    return vec4<f32>(color, max(texel.a, 0.75));
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

    let murkiness = water.tint.w;
    let refraction = refracted(in, normal, eye);

    let reflection = reflected(in, normal, eye);

    let ndotv = clamp(dot(normal, eye), 0.0, 1.0);
    var fresnel = clamp(pow(1.1 - ndotv, 2.0) * 1.5, 0.1, 0.75);

    // getSpecular; the engine's sunDir points from the sun.
    let specular_vector = reflect(-view.sun.xyz, normal);
    let specular_intensity = smoothstep(0.6, 1.0, pow(max(dot(specular_vector, eye), 0.0), 100.0)) * 1.2;
    let specular = clamp(specular_intensity * light.sun_color.rgb, vec3<f32>(0.0), vec3<f32>(1.0));

    let shadow = shadow_at(in.world, light.shadow.y);
    fresnel = mix(fresnel, fresnel * shadow, 0.05 + murkiness * 0.2);

    var out = mix(refraction.rgb, reflection.rgb, fresnel * reflection.a);
    out += shadow * specular;
    out = clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(apply_fog(out, in.world), refraction.a);
}
