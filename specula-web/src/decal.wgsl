// Actor decals (CModelDecal): a texture laid over the ground's own
// vertices, as CDecalRData builds them and terrain_decal draws them.

struct View {
    view_proj: mat4x4<f32>,
    // xyz towards the sun.
    sun: vec4<f32>,
    // Unused, unused, map size in metres, 1 on a circular map.
    misc: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;

@group(1) @binding(0) var decal_texture: texture_2d<f32>;
@group(1) @binding(1) var decal_sampler: sampler;

@group(2) @binding(0) var<uniform> light: Light;
@group(2) @binding(1) var shadow_map: texture_depth_2d;
@group(2) @binding(2) var shadow_sampler: sampler_comparison;

// shading.h calculateShading, without specular and ambient occlusion.
fn lit(albedo: vec3<f32>, normal: vec3<f32>, shadow: f32) -> vec3<f32> {
    let sun = max(dot(normalize(normal), view.sun.xyz), 0.0);
    return albedo * (light.sun_color.rgb * sun * shadow + light.ambient.rgb);
}

struct Varyings {
    // Invariant: tested against the ground's depth, from the same vertices.
    @builtin(position) @invariant position: vec4<f32>,
    // In metres, x and z.
    @location(0) ground: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // In metres.
    @location(3) height: f32,
}

@vertex
fn vs_decal(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // The decal's transform: cos and sin of its Y rotation, then its x and
    // z.
    @location(3) transform: vec4<f32>,
    // Offset x and z, size x and z, in metres.
    @location(4) shape: vec4<f32>,
) -> Varyings {
    var out: Varyings;
    out.position = view.view_proj * vec4<f32>(position, 1.0);
    out.ground = position.xz;
    out.normal = normal;
    out.height = position.y;
    // CDecalRData::BuildVertexData: the vertex back in decal space.
    let c = transform.x;
    let s = transform.y;
    let d = position.xz - transform.zw;
    let inv = vec2<f32>(c * d.x - s * d.y, s * d.x + c * d.y);
    out.uv = vec2<f32>(
        0.5 + (inv.x - shape.x) / shape.z,
        0.5 - (inv.y - shape.y) / shape.w,
    );
    return out;
}

@fragment
fn fs_decal(in: Varyings) -> @location(0) vec4<f32> {
    let tex = textureSample(decal_texture, decal_sampler, in.uv);
    // ObjectEntry.cpp samples baseTex with CLAMP_TO_BORDER, transparent
    // outside the decal's square.
    let inside = all(in.uv >= vec2<f32>(0.0)) && all(in.uv <= vec2<f32>(1.0));
    let size = view.misc.z;
    if !inside || (view.misc.w > 0.5 && distance(in.ground, vec2<f32>(size * 0.5)) > size * 0.5) {
        discard;
    }
    let world = vec3<f32>(in.ground.x, in.height, in.ground.y);
    let color = lit(tex.rgb, in.normal, shadow_at(world, light.shadow.y));
    return vec4<f32>(apply_fog(color, world), tex.a);
}
