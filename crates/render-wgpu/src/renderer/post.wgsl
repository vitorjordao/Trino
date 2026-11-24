// The post stack (issue #64, ADR 0033): the frame's 3D in an HDR target,
// its bloom (a dual-filter chain: Bjørge 2015's Kawase variant, at half
// resolution and below) and one composite pass — exposure, tonemap, the
// grading LUT, vignette, grain — into the picture the HUD draws on.
// `trino_core::post` has the same math on the CPU (`tone`, `Lut::apply`);
// the renderer's tests hold this shader to it.
//
// The scene holds display values (the shaders write sRGB-encoded color),
// extended past 1 by per-pixel lights and highlights: they are decoded to
// linear light here, exposed, tonemapped and encoded again.
//
// The speed blur (`trino_core::post::SpeedBlur`) is a radial blur toward a
// center in two steps: `fs_speed_gather` at half size, then the composite's
// taps of it (see `SpeedBlur::along` for the geometry both follow).

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_post(@builtin(vertex_index) i: u32) -> VsOut {
    // One triangle over the whole target.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.clip = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

fn decode3(v: vec3<f32>) -> vec3<f32> {
    let c = max(v, vec3<f32>(0.0));
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn encode3(l: vec3<f32>) -> vec3<f32> {
    let c = max(l, vec3<f32>(0.0));
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
}

// --- bloom ------------------------------------------------------------------

struct Blur {
    // 1 / the source's size, in its UVs.
    texel: vec2<f32>,
    // The prefilter's source corner (dynamic resolution): its far UV and
    // its last texel's centre.
    uv_scale: vec2<f32>,
    uv_max: vec2<f32>,
    threshold: f32,
    knee: f32,
    exposure: f32,
    // The up passes' tap spread, in source texels.
    spread: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var s_src: sampler;
@group(0) @binding(2) var<uniform> blur: Blur;

fn src(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(t_src, s_src, uv, 0.0).rgb;
}

// The scene's bright part, in linear light: a 4×4 box (four bilinear taps)
// then a soft-knee threshold on the brightest channel.
@fragment
fn fs_prefilter(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv * blur.uv_scale;
    let t = blur.texel;
    let lo = t * 0.5;
    let a = textureSampleLevel(t_src, s_src, clamp(uv + vec2<f32>(-t.x, -t.y), lo, blur.uv_max), 0.0).rgb;
    let b = textureSampleLevel(t_src, s_src, clamp(uv + vec2<f32>(t.x, -t.y), lo, blur.uv_max), 0.0).rgb;
    let c = textureSampleLevel(t_src, s_src, clamp(uv + vec2<f32>(-t.x, t.y), lo, blur.uv_max), 0.0).rgb;
    let d = textureSampleLevel(t_src, s_src, clamp(uv + vec2<f32>(t.x, t.y), lo, blur.uv_max), 0.0).rgb;
    let l = decode3((a + b + c + d) * 0.25) * blur.exposure;
    let br = max(max(l.r, l.g), l.b);
    let k = blur.knee;
    var soft = clamp(br - blur.threshold + k, 0.0, 2.0 * k);
    soft = soft * soft / (4.0 * k + 1e-5);
    let w = max(soft, br - blur.threshold) / max(br, 1e-5);
    // Bounded: one hot pixel must not flood the chain.
    return vec4<f32>(min(l * w, vec3<f32>(64.0)), 1.0);
}

@fragment
fn fs_down(in: VsOut) -> @location(0) vec4<f32> {
    let h = blur.texel * 0.5;
    let sum = src(in.uv) * 4.0
        + src(in.uv - h)
        + src(in.uv + h)
        + src(in.uv + vec2<f32>(h.x, -h.y))
        + src(in.uv - vec2<f32>(h.x, -h.y));
    return vec4<f32>(sum * 0.125, 1.0);
}

// Added onto the next larger level (blending One + One).
@fragment
fn fs_up(in: VsOut) -> @location(0) vec4<f32> {
    let h = blur.texel * 0.5 * blur.spread;
    var sum = src(in.uv + vec2<f32>(-h.x * 2.0, 0.0));
    sum += src(in.uv + vec2<f32>(-h.x, h.y)) * 2.0;
    sum += src(in.uv + vec2<f32>(0.0, h.y * 2.0));
    sum += src(in.uv + vec2<f32>(h.x, h.y)) * 2.0;
    sum += src(in.uv + vec2<f32>(h.x * 2.0, 0.0));
    sum += src(in.uv + vec2<f32>(h.x, -h.y)) * 2.0;
    sum += src(in.uv + vec2<f32>(0.0, -h.y * 2.0));
    sum += src(in.uv + vec2<f32>(-h.x, -h.y)) * 2.0;
    return vec4<f32>(sum / 12.0, 1.0);
}

// --- the speed blur's gather ---------------------------------------------------

struct Speed {
    // The scene's texel and drawn corner, as the prefilter's.
    texel: vec2<f32>,
    uv_scale: vec2<f32>,
    uv_max: vec2<f32>,
    center: vec2<f32>,
    strength: f32,
    // `SpeedBlur::GATHER_TAPS`, and 1 / `SpeedBlur::COMPOSITE_TAPS`.
    taps: f32,
    sub: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32,
    _pad4: f32,
};

@group(0) @binding(2) var<uniform> speed: Speed;

// `SpeedBlur::weight`: 0 inside the inner radius, 1 past the feather.
fn speed_weight(uv: vec2<f32>, center: vec2<f32>, inner: f32, feather: f32) -> f32 {
    let d = (uv - center) * 2.0;
    let r = sqrt(dot(d, d) * 0.5);
    let t = clamp((r - inner) / feather, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// Interleaved gradient noise (Jimenez 2014): a fixed 0..1 per pixel, so the
// taps' jitter never crawls on a still picture.
fn ign(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// The light over the first `sub` of a full-length streak from this pixel
// (in the picture's UVs): the mean of the display values' squares (gamma 2,
// close to sRGB's light and a multiply instead of three `pow` a tap),
// stored as its square root, a display value (an 8-bit chain keeps the
// darks). Full length everywhere, not this pixel's: the composite reads it
// for pixels farther out, whose streaks pass here.
@fragment
fn fs_speed_gather(in: VsOut) -> @location(0) vec4<f32> {
    let reach = max(1.0 - speed.strength, 0.05);
    // The scale from one tap to the next, and the first tap's (jittered).
    let step = log2(reach) * speed.sub / speed.taps;
    let ratio = exp2(step);
    var scale = exp2(step * ign(floor(in.clip.xy)));
    let d = in.uv - speed.center;
    let lo = speed.texel * 0.5;
    var sum = vec3<f32>(0.0);
    let n = u32(speed.taps);
    for (var j = 0u; j < n; j = j + 1u) {
        let uv = (speed.center + d * scale) * speed.uv_scale;
        let x = max(textureSampleLevel(t_src, s_src, clamp(uv, lo, speed.uv_max), 0.0).rgb, vec3<f32>(0.0));
        sum += x * x;
        scale *= ratio;
    }
    return vec4<f32>(sqrt(sum / speed.taps), 1.0);
}

// --- the composite ------------------------------------------------------------

struct Comp {
    // 1 / the scene's allocated size, and its drawn corner.
    texel: vec2<f32>,
    uv_scale: vec2<f32>,
    uv_max: vec2<f32>,
    exposure: f32,
    tonemap: u32,
    // The bloom's weight (intensity / its levels; 0 = no bloom).
    bloom: f32,
    vignette: f32,
    grain: f32,
    seed: u32,
    // Texels per axis of the LUT (0 = no grading).
    lut_n: f32,
    sharpen: f32,
    // 0 = the picture, 1 = the bloom alone.
    view: u32,
    // The speed blur's strength (0 = none), center, inner radius, feather
    // and `SpeedBlur::COMPOSITE_TAPS`.
    speed: f32,
    speed_center: vec2<f32>,
    speed_inner: f32,
    speed_feather: f32,
    speed_taps: f32,
    // 1 / `SpeedBlur::FADE_PIXELS` and the picture's size in pixels.
    speed_fade: f32,
    speed_size: vec2<f32>,
    // The second LUT's texels per axis (0 = none) and its weight
    // (`PostSettings::grading_to`, `grading_mix`).
    lut_b_n: f32,
    lut_mix: f32,
};

@group(0) @binding(0) var t_scene: texture_2d<f32>;
@group(0) @binding(1) var t_bloom: texture_2d<f32>;
@group(0) @binding(2) var t_lut: texture_3d<f32>;
@group(0) @binding(3) var s_lin: sampler;
@group(0) @binding(4) var<uniform> comp: Comp;
@group(0) @binding(5) var t_speed: texture_2d<f32>;
@group(0) @binding(6) var t_lut_b: texture_3d<f32>;

fn rows(r0: vec3<f32>, r1: vec3<f32>, r2: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(r0, v), dot(r1, v), dot(r2, v));
}

fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

// `trino_core::post::tonemap`: linear light in, linear 0..1 out.
fn tonemap(l0: vec3<f32>) -> vec3<f32> {
    let l = max(l0, vec3<f32>(0.0));
    switch comp.tonemap {
        case 1u: {
            return l / (vec3<f32>(1.0) + l);
        }
        case 2u: {
            return clamp((l * (2.51 * l + 0.03)) / (l * (2.43 * l + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
        }
        case 3u: {
            let i = rows(
                vec3<f32>(0.84247906, 0.0784336, 0.079223745),
                vec3<f32>(0.042328242, 0.8784686, 0.07916613),
                vec3<f32>(0.042375655, 0.0784336, 0.87914297),
                l,
            );
            let lo = -12.47393;
            let hi = 4.026069;
            let e = clamp(log2(max(i, vec3<f32>(1e-10))), vec3<f32>(lo), vec3<f32>(hi));
            let v = agx_contrast((e - lo) / (hi - lo));
            let o = rows(
                vec3<f32>(1.196879, -0.09802088, -0.09902974),
                vec3<f32>(-0.05289685, 1.1519031, -0.09896118),
                vec3<f32>(-0.052971635, -0.09804345, 1.1510737),
                v,
            );
            return min(pow(max(o, vec3<f32>(0.0)), vec3<f32>(2.2)), vec3<f32>(1.0));
        }
        default: {
            return min(l, vec3<f32>(1.0));
        }
    }
}

fn scene_at(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(t_scene, s_lin, clamp(uv, comp.texel * 0.5, comp.uv_max), 0.0).rgb;
}

// One pixel of `trino_core::post::tone`, plus the bloom (linear light).
fn toned(display: vec3<f32>, bloom: vec3<f32>) -> vec3<f32> {
    let l = decode3(display) * comp.exposure + bloom;
    return clamp(encode3(tonemap(l)), vec3<f32>(0.0), vec3<f32>(1.0));
}


fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

@fragment
fn fs_post(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv * comp.uv_scale;
    let bloom = textureSampleLevel(t_bloom, s_lin, in.uv, 0.0).rgb * comp.bloom;
    if comp.view == 1u {
        return vec4<f32>(clamp(encode3(bloom), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    let center = textureSampleLevel(t_scene, s_lin, clamp(uv, comp.texel * 0.5, comp.uv_max), 0.0);
    // The speed blur's share of this pixel (`SpeedBlur::fade`): 0 inside
    // the inner radius and without it (the sharp pixel alone), 1 once the
    // streak is a few pixels long (the blur alone).
    var fade = 0.0;
    var reach = 1.0;
    let sd = in.uv - comp.speed_center;
    if comp.speed > 0.0 {
        let w = speed_weight(in.uv, comp.speed_center, comp.speed_inner, comp.speed_feather);
        reach = max(1.0 - comp.speed * w, 0.05);
        fade = clamp((1.0 - reach) * length(sd * comp.speed_size) * comp.speed_fade, 0.0, 1.0);
    }
    var c = vec3<f32>(0.0);
    if fade < 1.0 {
        c = toned(center.rgb, bloom);
        if comp.sharpen > 0.0 {
            // The upscale's contrast-adaptive sharpening, on toned values.
            let t = comp.texel;
            let n = toned(scene_at(uv + vec2<f32>(0.0, -t.y)), bloom);
            let w = toned(scene_at(uv + vec2<f32>(-t.x, 0.0)), bloom);
            let e = toned(scene_at(uv + vec2<f32>(t.x, 0.0)), bloom);
            let s = toned(scene_at(uv + vec2<f32>(0.0, t.y)), bloom);
            let lo = min(min(min(n, w), min(e, s)), c);
            let hi = max(max(max(n, w), max(e, s)), c);
            let amp = sqrt(clamp(min(lo, vec3<f32>(1.0) - hi) / max(hi, vec3<f32>(1.0 / 255.0)), vec3<f32>(0.0), vec3<f32>(1.0)));
            let k = amp * (-1.0 / mix(8.0, 5.0, clamp(comp.sharpen, 0.0, 1.0)));
            c = clamp((c + (n + w + e + s) * k) / (vec3<f32>(1.0) + 4.0 * k), vec3<f32>(0.0), vec3<f32>(1.0));
        }
    }
    if fade > 0.0 {
        // The gather read along the whole streak, one tap per gathered
        // sub-streak, geometrically (`SpeedBlur::along`); light averaged
        // as squares, like the gather.
        let ratio = exp2(log2(reach) / comp.speed_taps);
        var scale = 1.0;
        var sum = vec3<f32>(0.0);
        let n = u32(comp.speed_taps);
        for (var k = 0u; k < n; k = k + 1u) {
            let a = textureSampleLevel(t_speed, s_lin, comp.speed_center + sd * scale, 0.0).rgb;
            sum += a * a;
            scale *= ratio;
        }
        c = mix(c, toned(sqrt(sum / comp.speed_taps), bloom), fade);
    }
    if comp.lut_n > 0.0 || comp.lut_b_n > 0.0 {
        var a = c;
        if comp.lut_n > 0.0 {
            let n = comp.lut_n;
            let uvw = c * ((n - 1.0) / n) + vec3<f32>(0.5 / n);
            a = textureSampleLevel(t_lut, s_lin, uvw, 0.0).rgb;
        }
        // Mixed toward the second LUT's grading of the same color
        // (`trino_core::post::grade_mix`).
        if comp.lut_b_n > 0.0 && comp.lut_mix > 0.0 {
            let n = comp.lut_b_n;
            let uvw = c * ((n - 1.0) / n) + vec3<f32>(0.5 / n);
            let b = textureSampleLevel(t_lut_b, s_lin, uvw, 0.0).rgb;
            a = a + (b - a) * comp.lut_mix;
        }
        c = a;
    }
    if comp.vignette > 0.0 {
        let d = (in.uv - vec2<f32>(0.5)) * 2.0;
        let r = sqrt(dot(d, d) * 0.5);
        c *= 1.0 - comp.vignette * smoothstep(0.35, 0.95, r);
    }
    if comp.grain > 0.0 {
        let p = vec2<u32>(in.clip.xy);
        let h = pcg(p.x + pcg(p.y + pcg(comp.seed)));
        c += vec3<f32>((f32(h >> 8u) / 16777216.0 - 0.5) * comp.grain * 0.16);
    }
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), center.a);
}
