// The sky behind everything, as SkyManager::RenderSky draws its cube
// around the camera with sky.vs and sky.fs: each cube vertex samples the
// cube at its own position negated, the cube turned by the light's
// rotation, and below the horizon the sky fades to black. Drawn here as
// one triangle over the screen, each pixel's view direction rebuilt from
// the camera.

struct SkyView {
    // The camera's right, up and forward, scaled so that NDC x and y
    // reach the screen's edges.
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    // cos and sin of the cube's rotation, pi plus the sun's.
    rotation: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: SkyView;
@group(1) @binding(0) var sky_cube: texture_cube<f32>;
@group(1) @binding(1) var sky_sampler: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_sky(@builtin(vertex_index) index: u32) -> Varyings {
    let ndc = vec2<f32>(f32(index & 1u) * 4.0 - 1.0, f32(index >> 1u) * 4.0 - 1.0);
    var out: Varyings;
    out.position = vec4<f32>(ndc, 1.0, 1.0);
    out.ndc = ndc;
    return out;
}

@fragment
fn fs_sky(in: Varyings) -> @location(0) vec4<f32> {
    let direction = view.forward.xyz + in.ndc.x * view.right.xyz + in.ndc.y * view.up.xyz;
    // The cube's own frame: the rotation undone.
    let c = view.rotation.x;
    let s = view.rotation.y;
    let local = vec3<f32>(
        c * direction.x - s * direction.z,
        direction.y,
        s * direction.x + c * direction.z,
    );
    // v_tex, the point on the cube's surface the engine interpolates.
    let tex = -local / max(max(abs(local.x), abs(local.y)), abs(local.z));
    let color = textureSampleLevel(sky_cube, sky_sampler, tex, 0.0).rgb;
    let fade = ((1.0 - tex.y) - 0.75) * 4.0;
    return vec4<f32>(select(color, color * fade, tex.y > 0.0), 1.0);
}
