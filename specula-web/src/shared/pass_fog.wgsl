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
