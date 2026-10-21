// common/fog.h applyFog. Its depth, gl_FragCoord.z / gl_FragCoord.w, is
// the GL window depth times clip w under the engine's default planes (2
// and 4096 m), rebuilt here from the view depth so a map's fog factor
// reaches as far as in the game.
fn apply_fog(color: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let w = dot(world - light.eye.xyz, light.forward.xyz);
    let z = 4096.0 * (w - 2.0) / (4096.0 - 2.0);
    let density = light.fog.x;
    let max_fog = light.fog.y;
    let factor = exp2(-density * density * z * z * 1.442695) * (1.0 - max_fog) + max_fog;
    return mix(light.fog_color.rgb, color, clamp(factor, 0.0, 1.0));
}
