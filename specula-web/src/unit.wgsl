// The unit page: many copies of one actor baked by bake-unit, their
// vertices read back from vertex animation textures, on a flat ground.

struct View {
    view_proj: mat4x4<f32>,
    // xyz towards the sun.
    sun: vec4<f32>,
    player_color: vec4<f32>,
    // Vertex count, texture width.
    vat: vec4<f32>,
    // Ground half size in metres, ambient, sun strength.
    misc: vec4<f32>,
    // The rest is read by impostor.wgsl; it shares this buffer.
    camera_right: vec4<f32>,
    camera_up: vec4<f32>,
    eye: vec4<f32>,
}

struct Part {
    // 1 when tinted by the player colour.
    player_color: f32,
    // Fragments below this alpha are discarded.
    alpha_ref: f32,
    // WebGL2 needs uniform structs sized in multiples of 16 bytes.
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var positions: texture_2d<f32>;
@group(1) @binding(1) var normals: texture_2d<f32>;
@group(2) @binding(0) var skin: texture_2d<f32>;
@group(2) @binding(1) var skin_sampler: sampler;
@group(2) @binding(2) var<uniform> part: Part;

struct UnitVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
}

// Vertex v of frame f is texel f * V + v, rows of the texture width.
fn texel(frame: f32, vertex: u32) -> vec2<i32> {
    let width = u32(view.vat.y);
    let index = u32(frame) * u32(view.vat.x) + vertex;
    return vec2<i32>(i32(index % width), i32(index / width));
}

@vertex
fn vs_unit(
    @builtin(vertex_index) vertex: u32,
    @location(0) uv: vec2<f32>,
    // x, z, heading sine, heading cosine.
    @location(1) place: vec4<f32>,
    // Frame a, frame b, blend from a to b.
    @location(2) frames: vec4<f32>,
) -> UnitVaryings {
    let a = texel(frames.x, vertex);
    let b = texel(frames.y, vertex);
    let local = mix(textureLoad(positions, a, 0).xyz, textureLoad(positions, b, 0).xyz, frames.z);
    let normal = mix(textureLoad(normals, a, 0).xyz, textureLoad(normals, b, 0).xyz, frames.z);
    let world = turn(local, place.z, place.w) + vec3<f32>(place.x, 0.0, place.y);
    var out: UnitVaryings;
    out.position = view.view_proj * vec4<f32>(world, 1.0);
    out.uv = uv;
    out.normal = turn(normal, place.z, place.w);
    return out;
}

// model_common.fs: the alpha of a player-coloured skin masks the tint.
fn diffuse(tex: vec4<f32>) -> vec3<f32> {
    let tint = mix(view.player_color.rgb, vec3<f32>(1.0), tex.a);
    return tex.rgb * mix(vec3<f32>(1.0), tint, part.player_color);
}

@fragment
fn fs_unit(in: UnitVaryings) -> @location(0) vec4<f32> {
    let tex = textureSample(skin, skin_sampler, in.uv);
    if tex.a < part.alpha_ref {
        discard;
    }
    let lit = max(dot(normalize(in.normal), view.sun.xyz), 0.0);
    return vec4<f32>(diffuse(tex) * (view.misc.y + view.misc.z * lit), 1.0);
}

struct Captured {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
}

// Draws an impostor cell: unlit colour and the normal, so the impostor can
// be lit like the meshes. The unit is drawn unturned, so the normal is in
// model space; alpha marks coverage.
@fragment
fn fs_capture(in: UnitVaryings) -> Captured {
    let tex = textureSample(skin, skin_sampler, in.uv);
    if tex.a < part.alpha_ref {
        discard;
    }
    var out: Captured;
    out.albedo = vec4<f32>(diffuse(tex), 1.0);
    out.normal = vec4<f32>(normalize(in.normal) * 0.5 + 0.5, 1.0);
    return out;
}

struct GroundVaryings {
    @builtin(position) position: vec4<f32>,
    // In metres, x and z.
    @location(0) ground: vec2<f32>,
}

@vertex
fn vs_ground(@builtin(vertex_index) corner: u32) -> GroundVaryings {
    // A triangle strip over corners (-1,-1) (1,-1) (-1,1) (1,1).
    let unit_square = vec2<f32>(f32(corner & 1u), f32(corner >> 1u)) * 2.0 - 1.0;
    let ground = unit_square * view.misc.x;
    var out: GroundVaryings;
    out.position = view.view_proj * vec4<f32>(ground.x, 0.0, ground.y, 1.0);
    out.ground = ground;
    return out;
}

@fragment
fn fs_ground(in: GroundVaryings) -> @location(0) vec4<f32> {
    // Lines every metre, stronger every terrain tile (4 m), and the +x and
    // +z axes in red and blue, so scale and orientation can be read off.
    let width = fwidth(in.ground);
    let metre = abs(fract(in.ground + 0.5) - 0.5) / width;
    let tile = abs(fract(in.ground / 4.0 + 0.5) - 0.5) * 4.0 / width;
    // Metre lines fade out where they would be closer than a few pixels.
    let metre_fade = 1.0 - smoothstep(0.1, 0.3, max(width.x, width.y));
    let metre_line = (1.0 - clamp(min(metre.x, metre.y), 0.0, 1.0)) * metre_fade;
    let tile_line = 1.0 - clamp(min(tile.x, tile.y), 0.0, 1.0);
    var color = vec3<f32>(0.32, 0.36, 0.30);
    color = mix(color, vec3<f32>(0.40, 0.44, 0.38), metre_line);
    color = mix(color, vec3<f32>(0.52, 0.56, 0.50), tile_line);
    let on_x_axis = in.ground.x > 0.0 && abs(in.ground.y) / width.y < 1.5;
    let on_z_axis = in.ground.y > 0.0 && abs(in.ground.x) / width.x < 1.5;
    if on_x_axis {
        color = vec3<f32>(0.85, 0.25, 0.20);
    }
    if on_z_axis {
        color = vec3<f32>(0.20, 0.35, 0.85);
    }
    return vec4<f32>(color, 1.0);
}
