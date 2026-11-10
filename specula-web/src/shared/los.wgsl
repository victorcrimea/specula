
// los_fragment.h getLOS on the shown player's CLOSTexture (los.rs), with
// SmoothLOS's fade from the texture before to the one now: the texels sit
// on the LOS vertices, 4 m apart, and the view darkens below 0.03. No LOS
// (an observer) leaves everything lit. The shader declares `los_now`,
// `los_before` and `los_sampler` in its light group.
fn los_at(world: vec3<f32>) -> f32 {
    let side = light.fog.w;
    if side < 1.0 {
        return 1.0;
    }
    let uv = (world.xz / 4.0 + vec2<f32>(0.5)) / side;
    let now = textureSampleLevel(los_now, los_sampler, uv, 0.0).r;
    let before = textureSampleLevel(los_before, los_sampler, uv, 0.0).r;
    let los = mix(before, now, light.fog.z);
    return clamp(los - 0.03, 0.0, 1.0) / (1.0 - 0.03);
}
