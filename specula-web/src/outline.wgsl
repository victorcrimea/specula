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

// CMiniMap::DrawViewRect's line width and colour.
const WIDTH_CSS_PIXELS: f32 = 2.0;
const COLOR: vec4<f32> = vec4<f32>(1.0, 0.3, 0.3, 1.0);

@vertex
fn vs_main(
    @builtin(vertex_index) corner: u32,
    @location(0) start: vec2<f32>,
    @location(1) end: vec2<f32>,
) -> @builtin(position) vec4<f32> {
    // The same letterbox as the dots, so the outline lines up with them.
    let side = min(view.width, view.height);
    let fit = vec2<f32>(side / view.width, side / view.height);
    let size = vec2<f32>(view.width, view.height);
    // In pixels, where the line's width is measured.
    let a = ((start / view.map_size * 2.0 - 1.0) * fit + 1.0) * 0.5 * size;
    let b = ((end / view.map_size * 2.0 - 1.0) * fit + 1.0) * 0.5 * size;
    let along = b - a;
    let direction = select(vec2<f32>(1.0, 0.0), normalize(along), length(along) > 0.0);
    let half = WIDTH_CSS_PIXELS * view.pixel_ratio * 0.5;
    // A triangle strip over corners (0,0) (1,0) (0,1) (1,1): x along the
    // segment, y across it; the ends reach out so corners close.
    let s = f32(corner & 1u);
    let t = f32(corner >> 1u) * 2.0 - 1.0;
    let normal = vec2<f32>(-direction.y, direction.x);
    let pixel = mix(a - direction * half, b + direction * half, s) + normal * t * half;
    return vec4<f32>(pixel / size * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return COLOR;
}
