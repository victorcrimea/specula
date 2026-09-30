struct View {
    map_size: f32,
    width: f32,
    height: f32,
    dot_size: f32,
    // How far the dots have moved from their start to their turn position, 0..1.
    progress: f32,
}

@group(0) @binding(0) var<uniform> view: View;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(
    @builtin(vertex_index) corner: u32,
    @location(0) start: vec2<f32>,
    @location(1) end: vec2<f32>,
    @location(2) color: vec4<f32>,
) -> Varyings {
    let world = mix(start, end, view.progress);
    // Letterbox: the square map fills the shorter side of the canvas.
    let side = min(view.width, view.height);
    let fit = vec2<f32>(side / view.width, side / view.height);
    let center = (world / view.map_size * 2.0 - 1.0) * fit;
    // A triangle strip over corners (0,0) (1,0) (0,1) (1,1).
    let offset = vec2<f32>(f32(corner & 1u), f32(corner >> 1u)) - 0.5;
    let pixel = 2.0 / vec2<f32>(view.width, view.height);

    var out: Varyings;
    out.position = vec4<f32>(center + offset * view.dot_size * pixel, 0.0, 1.0);
    // Alpha 0 marks an entity with no minimap colour.
    out.color = select(vec4<f32>(0.35, 0.35, 0.35, 1.0), color, color.a > 0.5);
    return out;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    return in.color;
}
