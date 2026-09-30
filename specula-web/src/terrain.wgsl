// The view uniform of dots.wgsl.
struct View {
    map_size: f32,
    width: f32,
    height: f32,
    dot_size: f32,
    progress: f32,
    circular: f32,
    pixel_ratio: f32,
    // WebGL2 needs uniform structs sized in multiples of 16 bytes.
    _pad: f32,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var tiles: texture_2d<f32>;
@group(1) @binding(1) var tiles_sampler: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    // 0..1 across the map, x east, y north.
    @location(0) map: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) corner: u32) -> Varyings {
    // A triangle strip over corners (0,0) (1,0) (0,1) (1,1).
    let map = vec2<f32>(f32(corner & 1u), f32(corner >> 1u));
    // The same letterbox as the dots, so both line up.
    let side = min(view.width, view.height);
    let fit = vec2<f32>(side / view.width, side / view.height);

    var out: Varyings;
    out.position = vec4<f32>((map * 2.0 - 1.0) * fit, 0.0, 1.0);
    out.map = map;
    return out;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    // Sampled before the branch: WGSL needs uniform control flow for it.
    let color = textureSample(tiles, tiles_sampler, in.map);
    // Outside the circle of a circular map is not part of the world.
    if view.circular > 0.5 && distance(in.map, vec2<f32>(0.5, 0.5)) > 0.5 {
        discard;
    }
    return color;
}
