// Territory borders on the ground: CTexturedLineRData's closed lines, drawn
// as the engine's overlayline shader draws them, but widened so they keep a
// least width on screen (territory.rs).

struct View {
    view_proj: mat4x4<f32>,
    sun: vec4<f32>,
    misc: vec4<f32>,
}

struct Params {
    // Metres per pixel at depth 1, narrowest line in pixels, seconds into
    // the blink period, unused.
    misc: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;

@group(1) @binding(0) var<uniform> params: Params;
@group(1) @binding(1) var base_texture: texture_2d<f32>;
@group(1) @binding(2) var mask_texture: texture_2d<f32>;
@group(1) @binding(3) var border_sampler: sampler;

// territorymanager.xml's BorderThickness and BorderSeparation: the line's
// half width, and how far inside the border its middle runs, in metres.
const THICKNESS: f32 = 0.75;
const SEPARATION: f32 = 0.85;
const PI: f32 = 3.14159265;
// A widened line lies flat at each border point, so on a slope its inner
// part would sink under the ground; it is raised by this much per metre it
// was widened.
const LIFT: f32 = 0.5;
// How many times the engine's width the line is when it has fully turned
// from the engine's texture to plain colour: far away the texture's glow
// and dark band would leave only a sliver of the player's colour.
const FULL_WIDEN: f32 = 3.0;
// The texture's dark outer rim, kept on the plain line so it still reads
// as a border.
const RIM: vec3<f32> = vec3<f32>(60.0 / 255.0, 60.0 / 255.0, 60.0 / 255.0);
// Lets a little terrain show through the plain line.
const FAR_ALPHA: f32 = 0.85;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // 0 for the engine's look, 1 for the plain one.
    @location(2) far: f32,
}

@vertex
fn vs_main(
    // On the border itself, lifted off the ground.
    @location(0) position: vec3<f32>,
    // Inwards, along the ground, scaled so 1 is 1 m from the border.
    @location(1) miter: vec3<f32>,
    // u 0 on the inner edge, 1 on the outer one.
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) blinking: f32,
) -> Varyings {
    let depth = (view.view_proj * vec4<f32>(position, 1.0)).w;
    let half_width = max(THICKNESS, 0.5 * params.misc.y * params.misc.x * depth);
    // The outer edge stays where the engine draws it, so a wider line
    // grows into its own territory and not over the neighbour's.
    let outer = SEPARATION - THICKNESS;
    let inwards = outer + (1.0 - uv.x) * 2.0 * half_width;
    let lift = vec3<f32>(0.0, (half_width - THICKNESS) * LIFT, 0.0);
    var out: Varyings;
    out.position = view.view_proj * vec4<f32>(position + miter * inwards + lift, 1.0);
    out.far = clamp((half_width / THICKNESS - 1.0) / (FULL_WIDEN - 1.0), 0.0, 1.0);
    out.uv = uv;
    // CCmpTerritoryManager::Interpolate.
    let pulse = 0.2 + 0.8 * abs(cos(params.misc.z * PI));
    out.color = vec4<f32>(color.rgb, color.a * select(1.0, pulse, blinking > 0.5));
    return out;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    let base = textureSample(base_texture, border_sampler, in.uv);
    let mask = textureSample(mask_texture, border_sampler, in.uv);
    let near = vec4<f32>(mix(base.rgb, in.color.rgb, mask.r), in.color.a * base.a);
    let rim = smoothstep(0.85, 0.95, in.uv.x);
    let far = vec4<f32>(mix(in.color.rgb, RIM, rim), in.color.a * FAR_ALPHA);
    return mix(near, far, in.far);
}
