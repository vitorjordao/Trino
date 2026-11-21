// Water surfaces (ADR 0015), shared by the two modules that draw them: the
// screen-space triangle pass (`shaders.wgsl`: the CPU T&L's displaced water
// triangles) and the GPU T&L path's water pipelines (`shaders_tnl.wgsl` +
// `shaders_tnl_water.wgsl`: the mesh displaced in the vertex stage, ADR 0016
// §13). Both shade a pixel with `water_shade`, so the two paths differ only
// in how the surface's world position, UV and color reach the fragment.
// Needs, from the module it is appended to: group 1's texture and detail
// layer (`t_sprite`, `s_sprite`, `t_detail`, `s_detail`) and `specular`.
// Layout: water.rs (`GpuWaterSet`).

struct WaterSet {
    dirs: vec4<f32>,       // swell directions (x z, x z)
    k_phase: vec4<f32>,    // frequencies (cycles/unit), phases (turns)
    amp: vec4<f32>,        // heights, ripple response, gloss
    detail: vec4<f32>,     // detail scale, scroll u v, ripple count
    ripples: array<vec4<f32>, 4>, // x, z, age, strength
    sun_dir: vec4<f32>,    // direction the light travels; w: ambient
    sun_color: vec4<f32>,  // rgb; w: the clock (seconds)
    ambient: vec4<f32>,    // ambient color
    sky: vec4<f32>,        // reflected sky (the fog color with fog); w: fog on
    fog: vec4<f32>,        // near, far, the T&L's near plane
    eye: vec4<f32>,
    fwd: vec4<f32>,        // the view's forward axis
    scroll: vec4<f32>,     // xy: the texture's scroll (the GPU path adds it)
    emissive: vec4<f32>,   // rgb: the draw's emissive, 0..1
};

struct WaterSets {
    sets: array<WaterSet, 8>,
};

@group(3) @binding(0) var<uniform> water_sets: WaterSets;
@group(3) @binding(1) var t_scene_depth: texture_depth_2d;

// The scene's depth at pixel `p` (the one read of it: WebGL2 reads it as a
// float texture, `depth_as_float` in mod.rs).
fn scene_depth(p: vec2<i32>) -> f32 {
    return textureLoad(t_scene_depth, p, 0);
}

const W_TAU: f32 = 6.28318530718;
// trino_core::water's ripple constants.
const RIPPLE_SPEED: f32 = 1.6;
const RIPPLE_WAVELENGTH: f32 = 0.55;
const RIPPLE_WIDTH: f32 = 0.7;
const RIPPLE_LIFE: f32 = 2.2;

// The detail layer's UV: the base UV (scrolled by the flow) times the
// layer's scale, plus its own scroll (water.rs folds the base's out).
fn water_detail_uv(uv: vec2<f32>, scale: f32, scroll: vec2<f32>) -> vec2<f32> {
    return uv * max(scale, 1.0) + scroll;
}

// How much of a wave `wavelength` units long a pixel `fp` units wide can
// show: all of it while a crest spans four pixels or more, none from two
// on. Past that the wave aliases: far water flickered in moiré rings and
// its Fresnel term (which grows fast at grazing angles) turned to noise.
fn resolvable(wavelength: f32, fp: f32) -> f32 {
    return 1.0 - smoothstep(0.25 * wavelength, 0.5 * wavelength, fp);
}

// The swell's and the ripples' height gradient at world (x, z) (xy), and
// the ripples' height (z): the functions the core displaces the vertices
// with, exact here and per pixel, each faded out where the pixel (`fp`
// world units wide) is too coarse for its wavelength.
fn water_gradient(si: u32, p: vec2<f32>, fp: f32) -> vec3<f32> {
    let ws = &water_sets.sets[si];
    var g = vec2<f32>(0.0);
    var rh = 0.0;
    let dirs = (*ws).dirs;
    let kp = (*ws).k_phase;
    let amp = (*ws).amp;
    for (var i = 0; i < 2; i++) {
        var d = dirs.xy;
        var k = kp.x;
        var ph = kp.z;
        var a = amp.x;
        if i == 1 {
            d = dirs.zw;
            k = kp.y;
            ph = kp.w;
            a = amp.y;
        }
        let t = k * dot(d, p) - ph;
        let fade = resolvable(1.0 / max(k, 1e-6), fp);
        g += d * (a * W_TAU * k * cos(W_TAU * t) * fade);
    }
    let n = u32((*ws).detail.w + 0.5);
    let ripple_fade = resolvable(RIPPLE_WAVELENGTH, fp);
    for (var i = 0u; i < n; i++) {
        let r = (*ws).ripples[i];
        let dv = p - r.xy;
        let d = length(dv);
        let rad = r.z * RIPPLE_SPEED;
        let q = (d - rad) / RIPPLE_WIDTH;
        let e = 1.0 - q * q;
        if e <= 0.0 || d < 1e-4 {
            continue;
        }
        let decay = pow(max(1.0 - r.z / RIPPLE_LIFE, 0.0), 2.0);
        let s = r.w * amp.z * decay * ripple_fade;
        let t = (d - rad) / RIPPLE_WAVELENGTH;
        let de = 2.0 * e * (-2.0 * q / RIPPLE_WIDTH);
        let dh = s * (de * cos(W_TAU * t) - e * e * (W_TAU / RIPPLE_WAVELENGTH) * sin(W_TAU * t));
        g += dv * (dh / d);
        rh += s * e * e * cos(W_TAU * t);
    }
    // Detail the vertices can't carry: two short, fast capillary waves
    // (0.43 and 0.27 units), the first to go with distance.
    let clock = (*ws).sun_color.w;
    let fine = select(0.5, 1.0, amp.x > 0.0);
    let c1 = vec2<f32>(0.83, 0.56);
    let c2 = vec2<f32>(-0.42, 0.91);
    let f1 = fine * resolvable(1.0 / 2.3, fp);
    let f2 = fine * resolvable(1.0 / 3.7, fp);
    g += c1 * (0.012 * f1 * W_TAU * 2.3 * cos(W_TAU * (2.3 * dot(c1, p) - 1.3 * clock)));
    g += c2 * (0.008 * f2 * W_TAU * 3.7 * cos(W_TAU * (3.7 * dot(c2, p) - 1.7 * clock)));
    return vec3<f32>(g, rh);
}

// The per-pixel tier's water at a pixel: the surface at world `p` (UV `uv`,
// unlit color `base`, the detail layer's scale `scale`) in water set `si`,
// at framebuffer position `frag`. The swell's and the ripples' normals,
// the global light, the draw's emissive, Schlick Fresnel toward the sky,
// the sun's glint, the water column against the scene's depth (shallows
// and soft foam) and fog. Called from uniform control flow (it samples and
// takes derivatives).
fn water_shade(si: u32, p: vec3<f32>, uv: vec2<f32>, base: vec4<f32>, scale: f32, frag: vec2<f32>) -> vec4<f32> {
    let ws = &water_sets.sets[si];
    let duv = water_detail_uv(uv, scale, (*ws).detail.yz);
    let t0 = textureSample(t_sprite, s_sprite, uv);
    let t1 = textureSample(t_detail, s_detail, duv);
    var texel = t0.rgb;
    if scale > 0.0 {
        texel = mix(t0.rgb, t1.rgb, 0.5);
    }
    // The pixel's footprint on the surface (world units): the larger of its
    // two sides, long at grazing angles.
    let fp = max(length(dpdx(p.xz)), length(dpdy(p.xz)));
    let gh = water_gradient(si, p.xz, fp);
    let n = normalize(vec3<f32>(-gh.x, 1.0, -gh.y));
    // The global light from the per-pixel normal, plus the draw's glow (a
    // lava flow), both under the flowing layers as on a mesh.
    let sun = -normalize((*ws).sun_dir.xyz);
    let amb = (*ws).sun_dir.w;
    let k = amb * (*ws).ambient.rgb + (1.0 - amb) * max(dot(n, sun), 0.0) * (*ws).sun_color.rgb;
    var rgb = (base.rgb * k + (*ws).emissive.rgb) * texel;
    // Fresnel: the sky at grazing angles, where water also turns opaque.
    let v = normalize((*ws).eye.xyz - p);
    let f = 0.02 + 0.98 * pow(1.0 - max(dot(n, v), 0.0), 5.0);
    rgb = mix(rgb, (*ws).sky.rgb, f * 0.85);
    var a = base.a + (1.0 - base.a) * f;
    // The sun's glint.
    let gloss = (*ws).amp.w;
    if gloss > 0.0 && dot(n, sun) > 0.0 {
        rgb += (*ws).sun_color.rgb * specular(n, sun, v, gloss) * 1.5;
    }
    // Depth: the water column under this pixel, from the scene's depth.
    let zw = dot(p - (*ws).eye.xyz, (*ws).fwd.xyz);
    let dz = scene_depth(vec2<i32>(frag));
    let zs = (*ws).fog.z / max(1.0 - dz, 1e-6);
    let thick = zs - zw;
    // Shallow water shows more of the bed; soft foam where it meets the
    // ground, broken up by the detail layer.
    a *= mix(0.6, 1.0, clamp(thick / 0.8, 0.0, 1.0));
    let edge = clamp(1.0 - thick / 0.14, 0.0, 1.0);
    let foam = edge * edge * smoothstep(0.55, 0.9, t1.r + edge * 0.35);
    // Ripple crests catch the light: thin bright rings.
    let crest = clamp(gh.z * 18.0, 0.0, 1.0);
    let white = max(foam * 0.8, crest * 0.6);
    rgb = mix(rgb, vec3<f32>(0.93, 0.97, 1.0), white);
    a = max(a, max(foam * 0.9, crest * 0.7));
    if (*ws).sky.w > 0.5 {
        let fog = (*ws).fog;
        let ff = clamp((zw - fog.x) / (fog.y - fog.x), 0.0, 1.0);
        rgb = mix(rgb, (*ws).sky.rgb, ff);
    }
    return vec4<f32>(min(rgb, vec3<f32>(1.0)), clamp(a, 0.0, 1.0));
}

// `trino_core::water::sin_turns`: sin(2π t) by a parabola and a quartic
// refinement, the core's own sine. The GPU path displaces the vertices with
// it so its surface is the CPU path's to the bit pattern of the formula.
fn sin_turns(t: f32) -> f32 {
    let u = 2.0 * fract(t + 0.5) - 1.0;
    let y = 4.0 * u * (1.0 - abs(u));
    return 0.225 * (y * abs(y) - y) + y;
}

// The surface's height above its rest at world (x, z): the swell's two
// sines and the ripples' rings (`WaveSet::sample`, `ripple_sample`).
fn water_height(si: u32, p: vec2<f32>) -> f32 {
    let ws = &water_sets.sets[si];
    let dirs = (*ws).dirs;
    let kp = (*ws).k_phase;
    let amp = (*ws).amp;
    var h = amp.x * sin_turns(kp.x * dot(dirs.xy, p) - kp.z);
    h += amp.y * sin_turns(kp.y * dot(dirs.zw, p) - kp.w);
    let n = u32((*ws).detail.w + 0.5);
    for (var i = 0u; i < n; i++) {
        let r = (*ws).ripples[i];
        let dv = p - r.xy;
        let rad = r.z * RIPPLE_SPEED;
        let d = sqrt(dot(dv, dv));
        let q = (d - rad) * (1.0 / RIPPLE_WIDTH);
        let e = 1.0 - q * q;
        if e <= 0.0 {
            continue;
        }
        let k = 1.0 - r.z / RIPPLE_LIFE;
        let decay = select(0.0, k * k, k > 0.0);
        let s = r.w * amp.z * decay;
        let t = (d - rad) * (1.0 / RIPPLE_WAVELENGTH);
        h += s * e * e * sin_turns(t + 0.25);
    }
    return h;
}
