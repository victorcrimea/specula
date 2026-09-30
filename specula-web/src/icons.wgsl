// The view uniform of dots.wgsl, plus the fields only icons read.
struct View {
    map_size: f32,
    width: f32,
    height: f32,
    dot_size: f32,
    progress: f32,
    circular: f32,
    pixel_ratio: f32,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vs_main(
    @builtin(vertex_index) corner: u32,
    @location(0) start: vec2<f32>,
    @location(1) end: vec2<f32>,
    // Left, top, right, bottom in the atlas.
    @location(2) rect: vec4<f32>,
    // In CSS pixels.
    @location(3) size: f32,
    @location(4) color: vec4<f32>,
) -> Varyings {
    let world = mix(start, end, view.progress);
    // The same letterbox as the dots, so icons sit on their dots.
    let side = min(view.width, view.height);
    let fit = vec2<f32>(side / view.width, side / view.height);
    let center = (world / view.map_size * 2.0 - 1.0) * fit;
    // A triangle strip over corners (0,0) (1,0) (0,1) (1,1).
    let unit = vec2<f32>(f32(corner & 1u), f32(corner >> 1u));
    let pixel = 2.0 / vec2<f32>(view.width, view.height);

    var out: Varyings;
    out.position = vec4<f32>(center + (unit - 0.5) * size * view.pixel_ratio * pixel, 0.0, 1.0);
    // Clip space y points north like the map, image rows go down, so the
    // top of the icon is its first row.
    out.uv = mix(rect.xw, rect.zy, unit);
    // Alpha 0 marks an entity with no minimap colour; grey like its dot.
    out.color = select(vec4<f32>(0.35, 0.35, 0.35, 1.0), color, color.a > 0.5);
    return out;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    // The engine multiplies the white icon by the minimap colour.
    return textureSample(atlas, atlas_sampler, in.uv) * in.color;
}
