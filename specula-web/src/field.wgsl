// The observer page's ground: the heightmap in its terrain textures,
// blended as CPatchRData draws them, and a see-through water plane at the
// water level.

struct View {
    view_proj: mat4x4<f32>,
    // xyz towards the sun.
    sun: vec4<f32>,
    // Unused, unused, map size in metres, 1 on a circular map.
    misc: vec4<f32>,
    // Water level in metres.
    water: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;

struct TerrainVaryings {
    // Invariant: the blend pass tests its depth against the base pass's.
    @builtin(position) @invariant position: vec4<f32>,
    // In metres, x and z.
    @location(0) ground: vec2<f32>,
    @location(1) normal: vec3<f32>,
    // Across the tile's blend mask, and the mask's index in the atlas.
    @location(2) mask: vec3<f32>,
    // In metres.
    @location(3) height: f32,
}

struct Terrain {
    // CTerrainTextureEntry's matrix as terrain_common.vs takes it,
    // (_11, _13), then 1 when the texture is in and 0 while the minimap
    // picture stands in for it.
    transform: vec4<f32>,
}

@group(1) @binding(0) var terrain_texture: texture_2d<f32>;
@group(1) @binding(1) var terrain_sampler: sampler;
@group(1) @binding(2) var<uniform> terrain: Terrain;

@group(2) @binding(0) var alpha_maps: texture_2d<f32>;
@group(2) @binding(1) var alpha_sampler: sampler;

struct Light {
    sun_color: vec4<f32>,
    ambient: vec4<f32>,
    // World to the shadow map: texels in xy, depth in z (shadow.rs).
    shadow_matrix: mat4x4<f32>,
    // The camera, then the shadow cutoff distance in metres.
    eye: vec4<f32>,
    // The camera's forward, then one over the shadow map's size.
    forward: vec4<f32>,
    // Depth bias for models, for the ground, then 1 when shadows are on.
    shadow: vec4<f32>,
}

@group(3) @binding(0) var<uniform> light: Light;
@group(3) @binding(1) var shadow_map: texture_depth_2d;
@group(3) @binding(2) var shadow_sampler: sampler_comparison;

// shadows_fragment.h getShadowWithFade with its PCF: four bilinear
// comparisons weighted into a tent over 3x3 texels, faded out over the
// last 8 m before the cutoff. The Level form: it may follow a discard.
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

// shading.h calculateShading, without specular and ambient occlusion.
fn lit(albedo: vec3<f32>, normal: vec3<f32>, shadow: f32) -> vec3<f32> {
    let sun = max(dot(normalize(normal), view.sun.xyz), 0.0);
    return albedo * (light.sun_color.rgb * sun * shadow + light.ambient.rgb);
}

@vertex
fn vs_terrain(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) mask: vec3<f32>,
) -> TerrainVaryings {
    var out: TerrainVaryings;
    out.position = view.view_proj * vec4<f32>(position, 1.0);
    out.ground = position.xz;
    out.normal = normal;
    out.mask = mask;
    out.height = position.y;
    return out;
}

// The terrain's texture, planar as terrain_common.vs maps it, or the
// minimap picture's one texel per tile until it arrives; lit by the
// heightmap's normals and shadowed.
fn shade(in: TerrainVaryings) -> vec3<f32> {
    let size = view.misc.z;
    let c = terrain.transform.x;
    let s = -terrain.transform.y;
    let planar = vec2<f32>(
        in.ground.x * c + in.ground.y * -s,
        in.ground.x * -s + in.ground.y * -c,
    );
    let uv = select(in.ground / size, planar, terrain.transform.z > 0.5);
    let color = textureSample(terrain_texture, terrain_sampler, uv).rgb;
    let world = vec3<f32>(in.ground.x, in.height, in.ground.y);
    return lit(color, in.normal, shadow_at(world, light.shadow.y));
}

fn outside(in: TerrainVaryings) -> bool {
    let size = view.misc.z;
    return view.misc.w > 0.5 && distance(in.ground, vec2<f32>(size * 0.5)) > size * 0.5;
}

@fragment
fn fs_terrain(in: TerrainVaryings) -> @location(0) vec4<f32> {
    // Sampled before the discard: WGSL wants textureSample in uniform
    // control flow.
    let color = shade(in);
    if outside(in) {
        discard;
    }
    return vec4<f32>(color, 1.0);
}

// A neighbour's texture over the tile, through its mask: the atlas holds
// the masks side by side, each with a 2 texel border
// (CTerrainTextureManager::LoadAlphaMap).
@fragment
fn fs_blend(in: TerrainVaryings) -> @location(0) vec4<f32> {
    let color = shade(in);
    let atlas = vec2<f32>(textureDimensions(alpha_maps));
    let base = atlas.y;
    let u = (in.mask.z * (base + 4.0) + 2.0 + in.mask.x * base) / atlas.x;
    let alpha = 1.0 - textureSample(alpha_maps, alpha_sampler, vec2<f32>(u, in.mask.y)).a;
    if outside(in) {
        discard;
    }
    return vec4<f32>(color, alpha);
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
    out.height = view.water.x;
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
