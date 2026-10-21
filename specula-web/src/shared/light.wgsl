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
