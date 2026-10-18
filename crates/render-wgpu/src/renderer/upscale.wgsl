// The render scale's upscale (issue #55, ADR 0023 §3): the 3D drawn at a
// fraction (or a multiple) of the picture's resolution, stretched onto the
// picture with bilinear filtering and, below a scale of 1, an optional
// contrast-adaptive sharpening (after AMD's FidelityFX CAS: the four
// neighbours' local contrast decides how strongly the centre is pushed away
// from them, so edges sharpen and flat areas stay put).

struct Params {
    // 1 / the source's size, in its UVs.
    texel: vec2<f32>,
    // 0 = plain bilinear, 1 = strongest.
    sharpen: f32,
    _pad: f32,
    // The drawn part of the source (its top-left corner under dynamic
    // resolution): the UV of its far corner, and of its last texel's
    // centre (taps stay inside it).
    uv_scale: vec2<f32>,
    uv_max: vec2<f32>,
};

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var s_src: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_upscale(@builtin(vertex_index) i: u32) -> VsOut {
    // One triangle over the whole target.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.clip = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

fn inside(uv: vec2<f32>) -> vec2<f32> {
    return clamp(uv, params.texel * 0.5, params.uv_max);
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(t_src, s_src, inside(uv), 0.0).rgb;
}

@fragment
fn fs_upscale(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv * params.uv_scale;
    let c = textureSampleLevel(t_src, s_src, inside(uv), 0.0);
    if params.sharpen <= 0.0 {
        return c;
    }
    let t = params.texel;
    let n = tap(uv + vec2<f32>(0.0, -t.y));
    let w = tap(uv + vec2<f32>(-t.x, 0.0));
    let e = tap(uv + vec2<f32>(t.x, 0.0));
    let s = tap(uv + vec2<f32>(0.0, t.y));
    let lo = min(min(min(n, w), min(e, s)), c.rgb);
    let hi = max(max(max(n, w), max(e, s)), c.rgb);
    // How much room the neighbourhood leaves before clipping, per channel.
    let amp = sqrt(clamp(min(lo, vec3<f32>(1.0) - hi) / max(hi, vec3<f32>(1.0 / 255.0)), vec3<f32>(0.0), vec3<f32>(1.0)));
    // The negative lobe: -1/8 (soft) to -1/5 (strong).
    let peak = -1.0 / mix(8.0, 5.0, clamp(params.sharpen, 0.0, 1.0));
    let k = amp * peak;
    let rgb = (c.rgb + (n + w + e + s) * k) / (vec3<f32>(1.0) + 4.0 * k);
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), c.a);
}
