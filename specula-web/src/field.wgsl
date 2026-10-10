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
    // Very High's cascades: world to the map's texels in each one's quarter
    // (shadow.rs), then where each ends, in metres along the view.
    cascades: array<mat4x4<f32>, 4>,
    cascade_ends: vec4<f32>,
}

@group(3) @binding(0) var<uniform> light: Light;
@group(3) @binding(1) var shadow_map: texture_depth_2d;
@group(3) @binding(2) var shadow_sampler: sampler_comparison;

// Which side of the water this pass keeps, as the engine's water cameras
// clip (SceneRenderer's oblique near planes): 0 everything, for the view;
// 1 what is under the water, for the refraction; 2 what is over it, for
// the mirrored reflection.
override WATER_CLIP: u32 = 0u;

fn water_clipped(height: f32) -> bool {
    return (WATER_CLIP == 1u && height > view.water.x + 2.0)
        || (WATER_CLIP == 2u && height < view.water.x - 0.5);
}

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

// shadows_fragment.h getShadowWithFade: faded out over the last 8 m before
// the cutoff, or with Very High's cascades from the first that reaches the
// point, blended into the next over the last eighth of its reach. The
// Level form: it may follow a discard.
fn shadow_at(world: vec3<f32>, bias: f32) -> f32 {
    if light.shadow.z < 0.5 {
        return 1.0;
    }
    let depth = dot(world - light.eye.xyz, light.forward.xyz);
    if light.shadow.w < 1.5 {
        let fade = clamp((light.eye.w - depth - 8.0) / 8.0, 0.0, 1.0);
        if fade <= 0.0 {
            return 1.0;
        }
        return mix(1.0, shadow_in(light.shadow_matrix, world, bias), fade);
    }
    var cascade = 0;
    while cascade < 4 && depth > light.cascade_ends[cascade] {
        cascade += 1;
    }
    if cascade == 4 {
        return 1.0;
    }
    let end = light.cascade_ends[cascade];
    let shadow = shadow_in(light.cascades[cascade], world, bias);
    let width = clamp(end / 8.0, 2.0, 32.0);
    let blend = clamp((end - depth - width) / width, 0.0, 1.0);
    if cascade == 3 {
        return mix(1.0, shadow, blend);
    }
    if blend >= 1.0 {
        return shadow;
    }
    return mix(shadow_in(light.cascades[cascade + 1], world, bias), shadow, blend);
}

// shadows_fragment.h getShadowImpl with its PCF: four bilinear comparisons
// weighted into a tent over 3x3 texels.
fn shadow_in(matrix: mat4x4<f32>, world: vec3<f32>, bias: f32) -> f32 {
    let projected = matrix * vec4<f32>(world, 1.0);
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
    return dot(size.zxzx * size.wwyy, taps) / 9.0;
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
