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
    // picture stands in for it, then 1 when it has a normal map.
    transform: vec4<f32>,
    // The material's normal strength and specular power, then 1 when it
    // maps the texture triplanar, unused.
    effect: vec4<f32>,
}

@group(1) @binding(0) var terrain_texture: texture_2d<f32>;
@group(1) @binding(1) var terrain_sampler: sampler;
@group(1) @binding(2) var<uniform> terrain: Terrain;
// Flat and black where the terrain has none.
@group(1) @binding(3) var terrain_normal: texture_2d<f32>;
@group(1) @binding(4) var terrain_specular: texture_2d<f32>;

// The viewer's material maps setting: off leaves the normal and specular
// maps unsampled, for the cost of the plain shading.
override MAPS: bool = true;
// The viewer's triplanar setting: off maps the triplanar terrains planar
// too, for the cost of the extra samples.
override TRIPLANAR: bool = true;

@group(2) @binding(0) var alpha_maps: texture_2d<f32>;
@group(2) @binding(1) var alpha_sampler: sampler;

@group(3) @binding(0) var<uniform> light: Light;
@group(3) @binding(1) var shadow_map: texture_depth_2d;
@group(3) @binding(2) var shadow_sampler: sampler_comparison;

fn water_clipped(height: f32) -> bool {
    return (WATER_CLIP == 1u && height > view.water.x + 2.0)
        || (WATER_CLIP == 2u && height < view.water.x - 0.5);
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

// terrain_common.fs' triplanar weights: the normal's axes, tightened so
// that only steep faces take the side projections.
fn triplanar_weights(normal: vec3<f32>) -> vec3<f32> {
    let blending = max(abs(normalize(normal)) - 0.4679, vec3<f32>(0.0));
    return blending / (blending.x + blending.y + blending.z);
}

// terrain_common.fs' triplanar: world position over 32, projected along
// x onto (y, z), along y onto (z, x) and along z onto (y, x).
fn triplanar_sample(texture: texture_2d<f32>, coords: vec3<f32>, weights: vec3<f32>) -> vec4<f32> {
    return textureSample(texture, terrain_sampler, coords.yz) * weights.x
        + textureSample(texture, terrain_sampler, coords.zx) * weights.y
        + textureSample(texture, terrain_sampler, coords.yx) * weights.z;
}

// A normal map texel as the planar path reads it, in the frame of a
// projection whose u and v run along `u_axis` and `v_axis`. Unlike
// terrain_common.vs, which gives every projection the tangent (1, 0, 0)
// (its own TODO), each gets its axes, made orthogonal to the surface so
// that a face looking down an axis keeps its handedness.
fn projected_normal(packed: vec3<f32>, normal: vec3<f32>, u_axis: vec3<f32>, v_axis: vec3<f32>) -> vec3<f32> {
    let tangent = normalize(u_axis - normal * dot(normal, u_axis));
    let bitangent = normalize(v_axis - normal * dot(normal, v_axis));
    var local = packed * 2.0 - 1.0;
    local.y = -local.y;
    return mat3x3<f32>(tangent, bitangent, normal) * local;
}

// The terrain's texture, planar as terrain_common.vs maps it or triplanar
// where its material asks, or the minimap picture's one texel per tile
// until it arrives; lit by the heightmap's normals bent by the normal
// map, with the specular map's highlight, and shadowed
// (terrain_common.fs).
fn shade(in: TerrainVaryings) -> vec3<f32> {
    let size = view.misc.z;
    let c = terrain.transform.x;
    let s = -terrain.transform.y;
    let planar = vec2<f32>(
        in.ground.x * c + in.ground.y * -s,
        in.ground.x * -s + in.ground.y * -c,
    );
    let textured = terrain.transform.z > 0.5;
    let triplanar = TRIPLANAR && textured && terrain.effect.z > 0.5;
    let uv = select(in.ground / size, planar, textured);
    let world = vec3<f32>(in.ground.x, in.height, in.ground.y);
    let coords = world / 32.0;
    var normal = normalize(in.normal);
    let weights = triplanar_weights(normal);
    var color: vec3<f32>;
    if triplanar {
        color = triplanar_sample(terrain_texture, coords, weights).rgb;
    } else {
        color = textureSample(terrain_texture, terrain_sampler, uv).rgb;
    }
    let shadow = shadow_at(world, light.shadow.y);
    if !MAPS {
        let sun = max(dot(normal, view.sun.xyz), 0.0) * light.sun_color.rgb;
        return color * (sun * shadow + light.ambient.rgb);
    }
    var mapped: vec3<f32>;
    var specular_color: vec3<f32>;
    if triplanar {
        let x = vec3<f32>(1.0, 0.0, 0.0);
        let y = vec3<f32>(0.0, 1.0, 0.0);
        let z = vec3<f32>(0.0, 0.0, 1.0);
        mapped = normalize(
            projected_normal(textureSample(terrain_normal, terrain_sampler, coords.yz).rgb, normal, y, z) * weights.x
            + projected_normal(textureSample(terrain_normal, terrain_sampler, coords.zx).rgb, normal, z, x) * weights.y
            + projected_normal(textureSample(terrain_normal, terrain_sampler, coords.yx).rgb, normal, y, x) * weights.z
        );
        specular_color = triplanar_sample(terrain_specular, coords, weights).rgb;
    } else {
        // terrain_common.vs' tangent along the mapping.
        let tangent = normalize(vec3<f32>(c, 0.0, -s));
        let bitangent = cross(in.normal, tangent);
        var local = textureSample(terrain_normal, terrain_sampler, uv).rgb * 2.0 - 1.0;
        local.y = -local.y;
        mapped = normalize(mat3x3<f32>(tangent, bitangent, in.normal) * local);
        specular_color = textureSample(terrain_specular, terrain_sampler, uv).rgb;
    }
    if terrain.transform.w > 0.5 {
        // calculateNormal.
        let strength = terrain.effect.x;
        if strength <= 1.0 {
            mapped = normalize(mix(normal, mapped, strength));
        } else {
            mapped = normalize(mapped - normal * min(1.0, strength - 1.0));
        }
        normal = mapped;
    }
    let sun_diffuse = max(dot(normal, view.sun.xyz), 0.0) * light.sun_color.rgb;
    let half_vector = normalize(view.sun.xyz + normalize(pass_eye() - world));
    let specular = light.sun_color.rgb * specular_color
        * pow(max(0.001, dot(normal, half_vector)), terrain.effect.y);
    return (color * sun_diffuse + specular) * shadow + color * light.ambient.rgb;
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
    if outside(in) || water_clipped(in.height) {
        discard;
    }
    let world = vec3<f32>(in.ground.x, in.height, in.ground.y);
    return vec4<f32>(apply_fog(color, world), 1.0);
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
    if outside(in) || water_clipped(in.height) {
        discard;
    }
    let world = vec3<f32>(in.ground.x, in.height, in.ground.y);
    return vec4<f32>(apply_fog(color, world), alpha);
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
    let world = vec3<f32>(in.ground.x, in.height, in.ground.y);
    return vec4<f32>(apply_fog(vec3<f32>(0.31, 0.47, 0.63), world), 0.6);
}
