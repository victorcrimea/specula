// Impostors: a far unit is one quad facing the camera, cut from the atlas
// cell nearest its heading and animation frame, and lit by the atlas's
// model-space normals like the meshes are.

// The view uniform of unit.wgsl.
struct View {
    view_proj: mat4x4<f32>,
    // xyz towards the sun.
    sun: vec4<f32>,
    player_color: vec4<f32>,
    vat: vec4<f32>,
    // Ground half size in metres, ambient, sun strength.
    misc: vec4<f32>,
    // xyz the camera's right, w the radius of the unit's bounding sphere.
    camera_right: vec4<f32>,
    // xyz the camera's up, w the height of the sphere's centre.
    camera_up: vec4<f32>,
    eye: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var atlas_albedo: texture_2d<f32>;
@group(1) @binding(1) var atlas_normal: texture_2d<f32>;
@group(1) @binding(2) var atlas_sampler: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Heading sine and cosine.
    @location(1) heading: vec2<f32>,
}

@vertex
fn vs_impostor(
    @builtin(vertex_index) corner: u32,
    // x, z, heading sine, heading cosine.
    @location(1) place: vec4<f32>,
    // The atlas cell, then the atlas's columns and rows of cells.
    @location(2) cell: vec4<f32>,
) -> Varyings {
    // A triangle strip over corners (-1,-1) (1,-1) (-1,1) (1,1).
    let square = vec2<f32>(f32(corner & 1u), f32(corner >> 1u)) * 2.0 - 1.0;
    let radius = view.camera_right.w;
    let middle = vec3<f32>(place.x, view.camera_up.w, place.y);
    let to_eye = view.eye.xyz - middle;
    let distance = length(to_eye);
    // Pulled towards the camera by the radius, so the ground the unit stands
    // on does not cut off its feet, and shrunk to keep its size on screen.
    let centre = middle + to_eye / distance * radius;
    let half_size = radius * (distance - radius) / distance;
    let world = centre + (view.camera_right.xyz * square.x + view.camera_up.xyz * square.y) * half_size;

    let columns = u32(cell.y);
    let index = u32(cell.x);
    let origin = vec2<f32>(f32(index % columns), f32(index / columns));
    // Cells are drawn top row first, so the top of the quad is v = 0.
    let inside = vec2<f32>(square.x + 1.0, 1.0 - square.y) * 0.5;
    var out: Varyings;
    out.position = view.view_proj * vec4<f32>(world, 1.0);
    out.uv = (origin + inside) / cell.yz;
    out.heading = place.zw;
    return out;
}

@fragment
fn fs_impostor(in: Varyings) -> @location(0) vec4<f32> {
    let albedo = textureSample(atlas_albedo, atlas_sampler, in.uv);
    let packed = textureSample(atlas_normal, atlas_sampler, in.uv);
    if albedo.a < 0.5 {
        discard;
    }
    let normal = turn(packed.xyz * 2.0 - 1.0, in.heading.x, in.heading.y);
    let lit = max(dot(normalize(normal), view.sun.xyz), 0.0);
    return vec4<f32>(albedo.rgb * (view.misc.y + view.misc.z * lit), 1.0);
}
