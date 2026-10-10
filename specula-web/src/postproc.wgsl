// The observer page's map post-processing (postproc.rs), as
// CPostprocManager runs it: the bloom.fs blur levels, then the map's
// effect, dof.fs and hdr.fs, then the antialiasing, fxaa.fs, then the
// sharpening, cas.fs.

// BLOOM_PASS_H's and BLOOM_PASS_V's taps sum the scene's bloom term in
// hdr.fs; off, it is left out, as a bloom of 0.2 would.
override BLOOM: bool = true;

struct Post {
    brightness: f32,
    // CLightEnv::m_Contrast, hdr.fs' `hdr`.
    contrast: f32,
    saturation: f32,
    bloom: f32,
    // The view's planes, in metres.
    near: f32,
    far: f32,
    // cas.fs' `sharpness`, the viewer's sharpness factor.
    sharpness: f32,
    // WebGL2 needs uniform structs sized in multiples of 16 bytes.
    _pad: f32,
}

// The blur passes' input, in its own group: an entry point may not see
// two resources at one binding.
@group(1) @binding(0) var source: texture_2d<f32>;
@group(1) @binding(1) var source_sampler: sampler;

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var rendered: texture_2d<f32>;
@group(0) @binding(2) var blur2: texture_2d<f32>;
@group(0) @binding(3) var blur4: texture_2d<f32>;
@group(0) @binding(4) var blur8: texture_2d<f32>;
@group(0) @binding(5) var depth: texture_2d<f32>;
@group(0) @binding(6) var effect_sampler: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    // v_tex, with v = 0 at the top as the engine's fullscreen quad has it
    // on Vulkan.
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> Varyings {
    // One triangle over the whole target: (-1,-1) (3,-1) (-1,3).
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0;
    var out: Varyings;
    out.position = vec4<f32>(corner, 0.0, 1.0);
    out.uv = vec2<f32>(corner.x + 1.0, 1.0 - corner.y) * 0.5;
    return out;
}

// BLOOM_NOP: one bilinear tap of the full-size scene, for every level, as
// ApplyBlur never feeds a level the one above it.
@fragment
fn fs_downscale(in: Varyings) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(source, source_sampler, in.uv).rgb, 1.0);
}

// BLOOM_PASS_H and BLOOM_PASS_V: six equal taps 0.004 apart in texture
// coordinates, whatever the level's size.
fn box_blur(uv: vec2<f32>, step: vec2<f32>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    var at = uv - step * 2.5;
    for (var i = 0; i < 6; i++) {
        color += textureSample(source, source_sampler, at);
        at += step;
    }
    return vec4<f32>(color.rgb / 6.0, 1.0);
}

@fragment
fn fs_blur_h(in: Varyings) -> @location(0) vec4<f32> {
    return box_blur(in.uv, vec2<f32>(0.004, 0.0));
}

@fragment
fn fs_blur_v(in: Varyings) -> @location(0) vec4<f32> {
    return box_blur(in.uv, vec2<f32>(0.0, 0.004));
}

// The view depth, which dof.fs' linearizeDepth gives from GL's depth, from
// this page's [0, 1] one.
fn view_depth(uv: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(depth));
    let texel = min(vec2<i32>(uv * size), vec2<i32>(size) - 1);
    let d = textureLoad(depth, texel, 0).r;
    return post.near * post.far / (post.far - d * (post.far - post.near));
}

// dof.fs, with DOF.xml's BLUR_FOV of 3: the farther a pixel's depth from
// the average around the screen's centre, the blurrier the level it takes.
@fragment
fn fs_dof(in: Varyings) -> @location(0) vec4<f32> {
    var color = textureSample(rendered, effect_sampler, in.uv).rgb;
    let b2 = textureSample(blur2, effect_sampler, in.uv).rgb;
    let b4 = textureSample(blur4, effect_sampler, in.uv).rgb;
    let b8 = textureSample(blur8, effect_sampler, in.uv).rgb;
    // dof.fs averages the raw depths before linearizing them.
    let size = vec2<f32>(textureDimensions(depth));
    var mid = 0.0;
    for (var i = 0; i < 5; i++) {
        let offset = select(vec2<f32>(0.0), vec2<f32>(f32(i & 1), f32((i >> 1u) & 1)) * 0.2 - 0.1, i < 4);
        let texel = min(vec2<i32>((vec2<f32>(0.5) + offset) * size), vec2<i32>(size) - 1);
        mid += textureLoad(depth, texel, 0).r;
    }
    mid /= 5.0;
    let linear_mid = post.near * post.far / (post.far - mid * (post.far - post.near));
    let amount = clamp(abs(view_depth(in.uv) - linear_mid) / (linear_mid * 3.0), 0.0, 1.0);
    if amount < 0.25 {
        color = mix(color, b2, amount / 0.25);
    } else if amount < 0.5 {
        color = mix(b2, b4, (amount - 0.25) / 0.25);
    } else if amount < 0.75 {
        color = mix(b4, b8, (amount - 0.5) / 0.25);
    } else {
        color = b8;
    }
    return vec4<f32>(color, 1.0);
}

// hdr.fs. Unclamped, as there: the 8-bit target clamps.
@fragment
fn fs_hdr(in: Varyings) -> @location(0) vec4<f32> {
    var color = textureSample(rendered, effect_sampler, in.uv).rgb;
    if BLOOM {
        let b2 = textureSample(blur2, effect_sampler, in.uv).rgb;
        let b4 = textureSample(blur4, effect_sampler, in.uv).rgb;
        let b8 = textureSample(blur8, effect_sampler, in.uv).rgb;
        let bloom = post.bloom;
        var glow = (max(b2 - bloom, vec3<f32>(0.0)) + max(b4 - bloom, vec3<f32>(0.0))
            + max(b8 - bloom, vec3<f32>(0.0))) / 3.0;
        glow = mix(glow, color, bloom / 0.2);
        color = max(glow, color);
    }
    color += vec3<f32>(post.brightness);
    color = (color - 0.5) * post.contrast + 0.5;
    color = mix(vec3<f32>(dot(color, vec3<f32>(0.299, 0.587, 0.114))), color, post.saturation);
    return vec4<f32>(color, 1.0);
}

//
//                  NVIDIA FXAA 3.11 by TIMOTHY LOTTES
//
// Copyright (c) 2014-2015, NVIDIA CORPORATION. All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
//  * Redistributions of source code must retain the above copyright
//    notice, this list of conditions and the following disclaimer.
//  * Redistributions in binary form must reproduce the above copyright
//    notice, this list of conditions and the following disclaimer in the
//    documentation and/or other materials provided with the distribution.
//  * Neither the name of NVIDIA CORPORATION nor the names of its
//    contributors may be used to endorse or promote products derived
//    from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS ``AS IS'' AND ANY
// EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
// PURPOSE ARE DISCLAIMED.  IN NO EVENT SHALL THE COPYRIGHT OWNER OR
// CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
// EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
// PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
// PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY
// OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//

// fxaa.fs' FXAA_QUALITY__PRESET 10: three search steps.
const FXAA_P0: f32 = 1.5;
const FXAA_P1: f32 = 3.0;
const FXAA_P2: f32 = 12.0;
// fxaa.fs' main.
const FXAA_SUBPIX: f32 = 0.75;
const FXAA_EDGE_THRESHOLD: f32 = 0.125;
const FXAA_EDGE_THRESHOLD_MIN: f32 = 0.0312;

// FxaaTexTop, also cas.fs' SAMPLE_2D. At level 0 explicitly, as most of
// FXAA's taps are in non-uniform control flow, where WGSL allows no
// implicit derivatives.
fn tap(at: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(rendered, effect_sampler, at, 0.0);
}

// FxaaLuma as the engine changed it: from all three channels.
fn fxaa_luma(rgba: vec4<f32>) -> f32 {
    return dot(rgba.rgb, vec3<f32>(0.299, 0.587, 0.114));
}

// FxaaPixelShader.
@fragment
fn fs_fxaa(in: Varyings) -> @location(0) vec4<f32> {
    let rcp_frame = 1.0 / vec2<f32>(textureDimensions(rendered));
    var pos_m = in.uv;
    let rgby_m = tap(pos_m);
    let luma_m = fxaa_luma(rgby_m);
    var luma_s = fxaa_luma(tap(pos_m + vec2<f32>(0.0, 1.0) * rcp_frame));
    var luma_e = fxaa_luma(tap(pos_m + vec2<f32>(1.0, 0.0) * rcp_frame));
    var luma_n = fxaa_luma(tap(pos_m + vec2<f32>(0.0, -1.0) * rcp_frame));
    var luma_w = fxaa_luma(tap(pos_m + vec2<f32>(-1.0, 0.0) * rcp_frame));

    let max_sm = max(luma_s, luma_m);
    let min_sm = min(luma_s, luma_m);
    let max_esm = max(luma_e, max_sm);
    let min_esm = min(luma_e, min_sm);
    let max_wn = max(luma_n, luma_w);
    let min_wn = min(luma_n, luma_w);
    let range_max = max(max_wn, max_esm);
    let range_min = min(min_wn, min_esm);
    let range_max_scaled = range_max * FXAA_EDGE_THRESHOLD;
    let range = range_max - range_min;
    let range_max_clamped = max(FXAA_EDGE_THRESHOLD_MIN, range_max_scaled);
    // The engine returns the alpha as it is; 1 here, as every other pass
    // writes, since the canvas may composite with it.
    if range < range_max_clamped {
        return vec4<f32>(rgby_m.rgb, 1.0);
    }

    let luma_nw = fxaa_luma(tap(pos_m + vec2<f32>(-1.0, -1.0) * rcp_frame));
    let luma_se = fxaa_luma(tap(pos_m + vec2<f32>(1.0, 1.0) * rcp_frame));
    let luma_ne = fxaa_luma(tap(pos_m + vec2<f32>(1.0, -1.0) * rcp_frame));
    let luma_sw = fxaa_luma(tap(pos_m + vec2<f32>(-1.0, 1.0) * rcp_frame));

    let luma_ns = luma_n + luma_s;
    let luma_we = luma_w + luma_e;
    let subpix_rcp_range = 1.0 / range;
    let subpix_nswe = luma_ns + luma_we;
    let edge_horz1 = (-2.0 * luma_m) + luma_ns;
    let edge_vert1 = (-2.0 * luma_m) + luma_we;
    let luma_nese = luma_ne + luma_se;
    let luma_nwne = luma_nw + luma_ne;
    let edge_horz2 = (-2.0 * luma_e) + luma_nese;
    let edge_vert2 = (-2.0 * luma_n) + luma_nwne;
    let luma_nwsw = luma_nw + luma_sw;
    let luma_swse = luma_sw + luma_se;
    let edge_horz4 = (abs(edge_horz1) * 2.0) + abs(edge_horz2);
    let edge_vert4 = (abs(edge_vert1) * 2.0) + abs(edge_vert2);
    let edge_horz3 = (-2.0 * luma_w) + luma_nwsw;
    let edge_vert3 = (-2.0 * luma_s) + luma_swse;
    let edge_horz = abs(edge_horz3) + edge_horz4;
    let edge_vert = abs(edge_vert3) + edge_vert4;

    let subpix_nwswnese = luma_nwsw + luma_nese;
    var length_sign = rcp_frame.x;
    let horz_span = edge_horz >= edge_vert;
    let subpix_a = subpix_nswe * 2.0 + subpix_nwswnese;
    if !horz_span {
        luma_n = luma_w;
        luma_s = luma_e;
    } else {
        length_sign = rcp_frame.y;
    }
    let subpix_b = (subpix_a * (1.0 / 12.0)) - luma_m;

    let gradient_n = luma_n - luma_m;
    let gradient_s = luma_s - luma_m;
    var luma_nn = luma_n + luma_m;
    let luma_ss = luma_s + luma_m;
    let pair_n = abs(gradient_n) >= abs(gradient_s);
    let gradient = max(abs(gradient_n), abs(gradient_s));
    if pair_n {
        length_sign = -length_sign;
    }
    let subpix_c = clamp(abs(subpix_b) * subpix_rcp_range, 0.0, 1.0);

    var pos_b = pos_m;
    let off_np = vec2<f32>(select(0.0, rcp_frame.x, horz_span), select(rcp_frame.y, 0.0, horz_span));
    if !horz_span {
        pos_b.x += length_sign * 0.5;
    } else {
        pos_b.y += length_sign * 0.5;
    }

    var pos_n = pos_b - off_np * FXAA_P0;
    var pos_p = pos_b + off_np * FXAA_P0;
    let subpix_d = ((-2.0) * subpix_c) + 3.0;
    var luma_end_n = fxaa_luma(tap(pos_n));
    let subpix_e = subpix_c * subpix_c;
    var luma_end_p = fxaa_luma(tap(pos_p));

    if !pair_n {
        luma_nn = luma_ss;
    }
    let gradient_scaled = gradient * 1.0 / 4.0;
    let luma_mm = luma_m - luma_nn * 0.5;
    let subpix_f = subpix_d * subpix_e;
    let luma_m_lt_zero = luma_mm < 0.0;

    luma_end_n -= luma_nn * 0.5;
    luma_end_p -= luma_nn * 0.5;
    var done_n = abs(luma_end_n) >= gradient_scaled;
    var done_p = abs(luma_end_p) >= gradient_scaled;
    if !done_n {
        pos_n -= off_np * FXAA_P1;
    }
    if !done_p {
        pos_p += off_np * FXAA_P1;
    }
    if !done_n || !done_p {
        if !done_n {
            luma_end_n = fxaa_luma(tap(pos_n)) - luma_nn * 0.5;
        }
        if !done_p {
            luma_end_p = fxaa_luma(tap(pos_p)) - luma_nn * 0.5;
        }
        done_n = abs(luma_end_n) >= gradient_scaled;
        done_p = abs(luma_end_p) >= gradient_scaled;
        if !done_n {
            pos_n -= off_np * FXAA_P2;
        }
        if !done_p {
            pos_p += off_np * FXAA_P2;
        }
    }

    var dst_n = pos_m.x - pos_n.x;
    var dst_p = pos_p.x - pos_m.x;
    if !horz_span {
        dst_n = pos_m.y - pos_n.y;
        dst_p = pos_p.y - pos_m.y;
    }
    let good_span_n = (luma_end_n < 0.0) != luma_m_lt_zero;
    let span_length = dst_p + dst_n;
    let good_span_p = (luma_end_p < 0.0) != luma_m_lt_zero;
    let span_length_rcp = 1.0 / span_length;

    let direction_n = dst_n < dst_p;
    let dst = min(dst_n, dst_p);
    let good_span = select(good_span_p, good_span_n, direction_n);
    let subpix_g = subpix_f * subpix_f;
    let pixel_offset = (dst * (-span_length_rcp)) + 0.5;
    let subpix_h = subpix_g * FXAA_SUBPIX;

    let pixel_offset_good = select(0.0, pixel_offset, good_span);
    let pixel_offset_subpix = max(pixel_offset_good, subpix_h);
    if !horz_span {
        pos_m.x += pixel_offset_subpix * length_sign;
    } else {
        pos_m.y += pixel_offset_subpix * length_sign;
    }
    return vec4<f32>(tap(pos_m).rgb, 1.0);
}

// LICENSE
// =======
// Copyright (c) 2020 Advanced Micro Devices, Inc. All rights reserved.
// -------
// Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation
// files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy,
// modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
// -------
// The above copyright notice and this permission notice shall be included in all copies or substantial portions of the
// Software.
// -------
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
// WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL THE AUTHORS OR
// COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
// ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE

// cas.fs, the engine's port of CasFilter() without scaling.
@fragment
fn fs_cas(in: Varyings) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(rendered));
    // The 3x3 neighbourhood around e:
    //  a b c
    //  d(e)f
    //  g h i
    let a = tap(in.uv + vec2<f32>(-1.0, -1.0) * texel).rgb;
    let b = tap(in.uv + vec2<f32>(0.0, -1.0) * texel).rgb;
    let c = tap(in.uv + vec2<f32>(1.0, -1.0) * texel).rgb;
    let d = tap(in.uv + vec2<f32>(-1.0, 0.0) * texel).rgb;
    let e = tap(in.uv).rgb;
    let f = tap(in.uv + vec2<f32>(1.0, 0.0) * texel).rgb;
    let g = tap(in.uv + vec2<f32>(-1.0, 1.0) * texel).rgb;
    let h = tap(in.uv + vec2<f32>(0.0, 1.0) * texel).rgb;
    let i = tap(in.uv + vec2<f32>(1.0, 1.0) * texel).rgb;

    // Soft min and max: the cross plus the whole 3x3, so twice as big.
    var mn = min(min(min(d, e), min(f, b)), h);
    let mn2 = min(mn, min(min(a, c), min(g, i)));
    mn += mn2;
    var mx = max(max(max(d, e), max(f, b)), h);
    let mx2 = max(mx, max(max(a, c), max(g, i)));
    mx += mx2;

    let rcp_m = 1.0 / mx;
    let amp = sqrt(clamp(min(mn, 2.0 - mx) * rcp_m, vec3<f32>(0.0), vec3<f32>(1.0)));
    let peak = -1.0 / (8.0 - 3.0 * post.sharpness);
    let w = amp * peak;
    let rcp_weight = 1.0 / (1.0 + 4.0 * w);
    let color = clamp(((b + d + f + h) * w + e) * rcp_weight, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(color, 1.0);
}
