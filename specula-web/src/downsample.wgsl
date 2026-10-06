// Halves one mip level of the impostor atlas into the next. Colours and
// normals are averaged weighted by coverage, so the empty background does
// not darken the edges of the unit.

@group(0) @binding(0) var source_albedo: texture_2d<f32>;
@group(0) @binding(1) var source_normal: texture_2d<f32>;

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle over the whole target: (-1,-1) (3,-1) (-1,3).
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0;
    return vec4<f32>(corner, 0.0, 1.0);
}

struct Halved {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
}

@fragment
fn fs_downsample(@builtin(position) position: vec4<f32>) -> Halved {
    let base = vec2<i32>(position.xy) * 2;
    var colour = vec3<f32>(0.0);
    var coverage = 0.0;
    var normal = vec3<f32>(0.0);
    var normal_coverage = 0.0;
    for (var i = 0; i < 4; i++) {
        let at = base + vec2<i32>(i & 1, i >> 1u);
        let albedo = textureLoad(source_albedo, at, 0);
        let packed = textureLoad(source_normal, at, 0);
        colour += albedo.rgb * albedo.a;
        coverage += albedo.a;
        normal += packed.rgb * packed.a;
        normal_coverage += packed.a;
    }
    var out: Halved;
    out.albedo = vec4<f32>(colour / max(coverage, 1e-4), coverage / 4.0);
    out.normal = vec4<f32>(normal / max(normal_coverage, 1e-4), normal_coverage / 4.0);
    return out;
}
