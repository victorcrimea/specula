// The observer page's impostors (far.rs): a far entity is one quad facing
// the camera, cut from its atlas cell nearest its heading and animation
// frame, relit with the atlas's model-space normals and tinted with its
// player colour as fs_model in model.wgsl.

// model.wgsl `View`, with the camera fields only impostors read.
struct View {
    view_proj: mat4x4<f32>,
    sun: vec4<f32>,
    // Unused, unused, map size in metres, 1 on a circular map.
    misc: vec4<f32>,
    water: vec4<f32>,
    // The camera's right, up and position.
    camera_right: vec4<f32>,
    camera_up: vec4<f32>,
    eye: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var atlas_albedo: texture_2d<f32>;
@group(1) @binding(1) var atlas_normal: texture_2d<f32>;
@group(1) @binding(2) var atlas_sampler: sampler;

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
}

@group(2) @binding(0) var<uniform> light: Light;

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

// shading.h calculateShading, without specular and ambient occlusion, and
// unshadowed: an impostor is too small on screen for a shadow on it to
// show.
fn lit(albedo: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let sun = max(dot(normal, view.sun.xyz), 0.0);
    return albedo * (light.sun_color.rgb * sun + light.ambient.rgb);
}

fn sign_not_zero(v: vec2<f32>) -> vec2<f32> {
    return select(vec2<f32>(-1.0), vec2<f32>(1.0), v >= vec2<f32>(0.0));
}

// model.wgsl `octahedral` undone.
fn from_octahedral(e: vec2<f32>) -> vec3<f32> {
    let f = e * 2.0 - 1.0;
    var n = vec3<f32>(f, 1.0 - abs(f.x) - abs(f.y));
    let t = max(-n.z, 0.0);
    n = vec3<f32>(n.xy - sign_not_zero(n.xy) * t, n.z);
    return normalize(n);
}

struct ImpostorVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Sine and cosine of the model's turn.
    @location(1) @interpolate(flat) turn: vec2<f32>,
    @location(2) @interpolate(flat) color: vec3<f32>,
    @location(3) world: vec3<f32>,
}

// The y rotation of the model transform, as impostor.wgsl `turn`.
fn turn(v: vec3<f32>, sine: f32, cosine: f32) -> vec3<f32> {
    return vec3<f32>(v.x * cosine + v.z * sine, v.y, -v.x * sine + v.z * cosine);
}

@vertex
fn vs_impostor(
    @builtin(vertex_index) corner: u32,
    // The bounding sphere's centre and radius.
    @location(0) sphere: vec4<f32>,
    // Turn sine and cosine, atlas columns and rows.
    @location(1) grid: vec4<f32>,
    // Atlas cell, player colour as RGBA8.
    @location(2) cell: vec2<u32>,
) -> ImpostorVaryings {
    // A triangle strip over corners (-1,-1) (1,-1) (-1,1) (1,1).
    let square = vec2<f32>(f32(corner & 1u), f32(corner >> 1u)) * 2.0 - 1.0;
    let radius = sphere.w;
    let to_eye = view.eye.xyz - sphere.xyz;
    let distance = max(length(to_eye), radius * 1.01);
    // Pulled towards the camera by the radius, so the ground the unit
    // stands on does not cut off its feet, and shrunk to keep its size.
    let centre = sphere.xyz + to_eye / distance * radius;
    let half_size = radius * (distance - radius) / distance;
    let world = centre
        + (view.camera_right.xyz * square.x + view.camera_up.xyz * square.y) * half_size;

    let columns = u32(grid.z);
    let origin = vec2<f32>(f32(cell.x % columns), f32(cell.x / columns));
    // Cells are drawn top row first, so the top of the quad is v = 0.
    let inside = vec2<f32>(square.x + 1.0, 1.0 - square.y) * 0.5;
    var out: ImpostorVaryings;
    out.position = view.view_proj * vec4<f32>(world, 1.0);
    out.world = world;
    out.uv = (origin + inside) / grid.zw;
    out.turn = grid.xy;
    let rgb = vec3<f32>(f32(cell.y & 255u), f32((cell.y >> 8u) & 255u), f32((cell.y >> 16u) & 255u));
    out.color = rgb / 255.0;
    return out;
}

@fragment
fn fs_impostor(in: ImpostorVaryings) -> @location(0) vec4<f32> {
    let albedo = textureSample(atlas_albedo, atlas_sampler, in.uv);
    let packed = textureSample(atlas_normal, atlas_sampler, in.uv);
    if albedo.a < 0.5 {
        discard;
    }
    let normal = turn(from_octahedral(packed.xy), in.turn.x, in.turn.y);
    let diffuse = albedo.rgb * mix(vec3<f32>(1.0), in.color, packed.z);
    return vec4<f32>(apply_fog(lit(diffuse, normal), in.world), 1.0);
}
