// Selection overlays on the ground (overlay.rs): the quads under units, as
// the engine's overlay quads, and the outlines round buildings, as its
// CTexturedLineRData lines. Both shade as its overlayline shader. The
// status bar sprites reuse the quads' vertex stage, their corners already
// facing the camera.

struct View {
    view_proj: mat4x4<f32>,
    sun: vec4<f32>,
    misc: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var mask_texture: texture_2d<f32>;
@group(1) @binding(2) var overlay_sampler: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vs_quad(
    @builtin(vertex_index) index: u32,
    // Top left, bottom left, bottom right, top right, as the engine's
    // SOverlayQuad corners, already lifted off the ground.
    @location(0) corner0: vec3<f32>,
    @location(1) corner1: vec3<f32>,
    @location(2) corner2: vec3<f32>,
    @location(3) corner3: vec3<f32>,
    // The texture's place in the atlas: u0, v0, u1, v1.
    @location(4) rect: vec4<f32>,
    @location(5) color: vec4<f32>,
) -> Varyings {
    // A strip runs top left, bottom left, top right, bottom right.
    var corners = array<vec3<f32>, 4>(corner0, corner1, corner3, corner2);
    var uvs = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    var out: Varyings;
    out.position = view.view_proj * vec4<f32>(corners[index], 1.0);
    out.uv = mix(rect.xy, rect.zw, uvs[index]);
    out.color = color;
    return out;
}

@vertex
fn vs_line(
    // On the line, lifted off the ground.
    @location(0) position: vec3<f32>,
    // Across the line, scaled so 1 is 1 m from it.
    @location(1) miter: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) thickness: f32,
) -> Varyings {
    var out: Varyings;
    // The engine's p1 + b at u 0 and p1 - b at u 1, b as long as the
    // thickness.
    let across = miter * thickness * (1.0 - 2.0 * uv.x);
    out.position = view.view_proj * vec4<f32>(position + across, 1.0);
    out.uv = uv;
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    let base = textureSample(base_texture, overlay_sampler, in.uv);
    let mask = textureSample(mask_texture, overlay_sampler, in.uv);
    return vec4<f32>(mix(base.rgb, in.color.rgb, mask.r), in.color.a * base.a);
}

// The status bars, as the engine's foreground_overlay shader: a tinted
// texture with no mask.
@fragment
fn fs_sprite(in: Varyings) -> @location(0) vec4<f32> {
    return textureSample(base_texture, overlay_sampler, in.uv) * in.color;
}
