// The observer page's ground: the heightmap coloured with the minimap
// picture, and a see-through water plane at the water level.

struct View {
    view_proj: mat4x4<f32>,
    // xyz towards the sun.
    sun: vec4<f32>,
    // Ambient, sun strength, map size in metres, 1 on a circular map.
    misc: vec4<f32>,
    // Water level in metres.
    water: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;

struct TerrainVaryings {
    @builtin(position) position: vec4<f32>,
    // In metres, x and z.
    @location(0) ground: vec2<f32>,
    @location(1) normal: vec3<f32>,
}

@group(1) @binding(0) var minimap: texture_2d<f32>;
@group(1) @binding(1) var minimap_sampler: sampler;

@vertex
fn vs_terrain(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> TerrainVaryings {
    var out: TerrainVaryings;
    out.position = view.view_proj * vec4<f32>(position, 1.0);
    out.ground = position.xz;
    out.normal = normal;
    return out;
}

// The minimap picture, one texel per tile, lit by the heightmap's normals.
@fragment
fn fs_terrain(in: TerrainVaryings) -> @location(0) vec4<f32> {
    let size = view.misc.z;
    // Sampled before the discard: WGSL wants textureSample in uniform
    // control flow.
    let color = textureSample(minimap, minimap_sampler, in.ground / size).rgb;
    if view.misc.w > 0.5 && distance(in.ground, vec2<f32>(size * 0.5)) > size * 0.5 {
        discard;
    }
    let lit = max(dot(normalize(in.normal), view.sun.xyz), 0.0);
    return vec4<f32>(color * (view.misc.x + view.misc.y * lit), 1.0);
}

// The water plane over the whole map at the water level.
@vertex
fn vs_water(@builtin(vertex_index) corner: u32) -> TerrainVaryings {
    let unit_square = vec2<f32>(f32(corner & 1u), f32(corner >> 1u));
    let ground = unit_square * view.misc.z;
    var out: TerrainVaryings;
    out.position = view.view_proj * vec4<f32>(ground.x, view.water.x, ground.y, 1.0);
    out.ground = ground;
    out.normal = vec3<f32>(0.0, 1.0, 0.0);
    return out;
}

@fragment
fn fs_water(in: TerrainVaryings) -> @location(0) vec4<f32> {
    let size = view.misc.z;
    if view.misc.w > 0.5 && distance(in.ground, vec2<f32>(size * 0.5)) > size * 0.5 {
        discard;
    }
    // CMiniMapTexture's deep water colour, half see-through.
    return vec4<f32>(0.31, 0.47, 0.63, 0.6);
}
