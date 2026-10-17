// Sprite pass: instanced quads onto the internal offscreen framebuffer.
// Instance layout must match batch.rs::Instance (stride 56).

struct Globals {
    // Internal framebuffer resolution in pixels.
    screen: vec2<f32>,
    // 0 = native, 1 = N64 look (3-point filtering + RGBA5551 dither).
    look: u32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct VsIn {
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec2<f32>,   // top-left corner, pixels
    @location(1) size: vec2<f32>,  // final size, pixels (scale pre-applied)
    @location(2) rotation: f32,    // radians, around sprite center
    @location(3) uv0: vec2<f32>,
    @location(4) uv1: vec2<f32>,
    @location(5) tint: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
};

@vertex
fn vs_sprite(in: VsIn) -> VsOut {
    // Triangle-strip corners: (0,0) (1,0) (0,1) (1,1).
    let corner = vec2<f32>(f32(in.vi & 1u), f32(in.vi >> 1u));
    let center = in.pos + in.size * 0.5;
    let local = (corner - vec2<f32>(0.5, 0.5)) * in.size;
    let c = cos(in.rotation);
    let s = sin(in.rotation);
    let rotated = vec2<f32>(local.x * c - local.y * s, local.x * s + local.y * c);
    let p = center + rotated;
    // Pixel space (Y-down, origin top-left) to NDC.
    let ndc = vec2<f32>(p.x / globals.screen.x * 2.0 - 1.0, 1.0 - p.y / globals.screen.y * 2.0);

    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = mix(in.uv0, in.uv1, corner);
    out.tint = in.tint;
    return out;
}

@group(1) @binding(0) var t_sprite: texture_2d<f32>;
@group(1) @binding(1) var s_sprite: sampler;

// N64 three-point bilinear: the RDP interpolates the 3 texels of the
// triangle half that contains the sample point, not a 2x2 quad — the
// source of the "N64 smear" on magnified textures.
fn sample_3point(uv: vec2<f32>) -> vec4<f32> {
    let dims = textureDimensions(t_sprite);
    let st = uv * vec2<f32>(dims) - vec2<f32>(0.5, 0.5);
    let base = floor(st);
    let f = st - base;
    let b = vec2<i32>(base);
    let hi = vec2<i32>(dims) - vec2<i32>(1, 1);
    let lo = vec2<i32>(0, 0);
    let t00 = textureLoad(t_sprite, clamp(b, lo, hi), 0);
    let t10 = textureLoad(t_sprite, clamp(b + vec2<i32>(1, 0), lo, hi), 0);
    let t01 = textureLoad(t_sprite, clamp(b + vec2<i32>(0, 1), lo, hi), 0);
    let t11 = textureLoad(t_sprite, clamp(b + vec2<i32>(1, 1), lo, hi), 0);
    if f.x + f.y < 1.0 {
        return t00 + f.x * (t10 - t00) + f.y * (t01 - t00);
    }
    return t11 + (1.0 - f.x) * (t01 - t11) + (1.0 - f.y) * (t10 - t11);
}

// 16-bit framebuffer write, as the RDP does it (angrylion-rdp-plus
// rdp/dither.c): per channel, 8-bit c rounds up to the next multiple of 8
// when its low 3 bits exceed the magic-square dither value, then keeps the
// top 5 bits; alpha is 1 bit. Stored expanded like a 5551 readback
// ((c5 << 3) | (c5 >> 2)), which is what the N64 golden dumps decode to.
fn quantize_5551(color: vec4<f32>, px: vec2<u32>) -> vec4<f32> {
    var magic = array<f32, 16>(
        0.0, 6.0, 1.0, 7.0,
        4.0, 2.0, 5.0, 3.0,
        3.0, 5.0, 2.0, 4.0,
        7.0, 1.0, 6.0, 0.0,
    );
    let m = magic[(px.y % 4u) * 4u + (px.x % 4u)];
    let c8 = floor(clamp(color.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0 + 0.5);
    let hi = floor(c8 / 8.0);
    let up = select(vec3<f32>(0.0), vec3<f32>(1.0), c8 - hi * 8.0 > vec3<f32>(m));
    let c5 = min(hi + up, vec3<f32>(31.0));
    let a = select(0.0, 1.0, color.a >= 0.5);
    return vec4<f32>(expand5(c5), a);
}

// 5-bit channel values -> the stored 8-bit expansion, normalized.
fn expand5(c5: vec3<f32>) -> vec3<f32> {
    return (c5 * 8.0 + floor(c5 / 4.0)) / 255.0;
}

@fragment
fn fs_sprite(in: VsOut) -> @location(0) vec4<f32> {
    // Sampled unconditionally: textureSample requires uniform control flow.
    let plain = textureSample(t_sprite, s_sprite, in.uv) * in.tint;
    var c = plain;
    if globals.look == 1u {
        c = sample_3point(in.uv) * in.tint;
    }
    // fill_rect (UVs at the -2 sentinel): a flat color blended with its own
    // alpha — the RDP blends with the fragment's full-precision alpha; only
    // the stored color is 5551.
    if in.uv.x < -1.0 {
        if globals.look == 1u {
            let q = quantize_5551(in.tint, vec2<u32>(in.clip.xy));
            return vec4<f32>(q.rgb, in.tint.a);
        }
        return in.tint;
    }
    if globals.look == 1u {
        return quantize_5551(c, vec2<u32>(in.clip.xy));
    }
    return plain;
}

// ---------------------------------------------------------------------------
// Triangle pass (3D): screen-space triangles produced by the engine's
// software T&L (trino_core::render3d): lit vertex colors × one texture per
// segment (a 1x1 white texture when untextured). Same offscreen target and
// N64-look quantization as sprites.

struct TriIn {
    @location(0) pos: vec2<f32>,   // pixels, internal resolution
    @location(1) color: vec4<f32>,
    @location(2) depth: f32,       // normalized (0 = near), z-buffer value
    @location(3) w: f32,           // view depth (textured) or 1.0
    @location(4) uv: vec2<f32>,
    // x: 1 = point filter (else the N64 look's 3-point); y: the detail
    // layer's UV scale (0 = none).
    @location(10) tex: vec2<f32>,
};

struct TriOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) tex: vec2<f32>,
};

// The detail layer of a 3D draw (group 1 with the texture; white when the
// draw has none).
@group(1) @binding(2) var t_detail: texture_2d<f32>;
@group(1) @binding(3) var s_detail: sampler;

@vertex
fn vs_tri(in: TriIn) -> TriOut {
    let ndc = vec2<f32>(
        in.pos.x / globals.screen.x * 2.0 - 1.0,
        1.0 - in.pos.y / globals.screen.y * 2.0,
    );
    var out: TriOut;
    // Scaling by w undoes itself in the perspective divide but makes the
    // rasterizer interpolate varyings perspective-correctly (textured
    // triangles carry their view depth; untextured ones w = 1 = affine).
    out.clip = vec4<f32>(ndc, in.depth, 1.0) * in.w;
    out.color = in.color;
    out.uv = in.uv;
    out.tex = in.tex;
    return out;
}

fn wrap_texel(p: vec2<i32>, dims: vec2<i32>) -> vec2<i32> {
    return ((p % dims) + dims) % dims;
}

// 3-point sampling for wrapping (tiled) 3D textures.
fn sample_3point_wrap(uv: vec2<f32>) -> vec4<f32> {
    return sample_3point_wrap_in(t_sprite, uv);
}

// The RDP's point sampling of a wrapping texture (`FILTER_POINT`).
fn sample_point_wrap(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let dims = vec2<i32>(textureDimensions(t));
    return textureLoad(t, wrap_texel(vec2<i32>(floor(uv * vec2<f32>(dims))), dims), 0);
}

// The N64 look's texel: point or 3-point, by the draw's filter.
fn sample_n64(t: texture_2d<f32>, uv: vec2<f32>, point: bool) -> vec4<f32> {
    if point {
        return sample_point_wrap(t, uv);
    }
    return sample_3point_wrap_in(t, uv);
}

// The detail layer's factor (white without one).
fn detail_texel(uv: vec2<f32>, tex: vec2<f32>) -> vec4<f32> {
    let d = textureSample(t_detail, s_detail, uv * max(tex.y, 1.0));
    if tex.y > 0.0 {
        return vec4<f32>(d.rgb, 1.0);
    }
    return vec4<f32>(1.0);
}

fn sample_3point_wrap_in(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let dims = vec2<i32>(textureDimensions(t));
    let st = uv * vec2<f32>(dims) - vec2<f32>(0.5, 0.5);
    let base = floor(st);
    let f = st - base;
    let b = vec2<i32>(base);
    let t00 = textureLoad(t, wrap_texel(b, dims), 0);
    let t10 = textureLoad(t, wrap_texel(b + vec2<i32>(1, 0), dims), 0);
    let t01 = textureLoad(t, wrap_texel(b + vec2<i32>(0, 1), dims), 0);
    let t11 = textureLoad(t, wrap_texel(b + vec2<i32>(1, 1), dims), 0);
    if f.x + f.y < 1.0 {
        return t00 + f.x * (t10 - t00) + f.y * (t01 - t00);
    }
    return t11 + (1.0 - f.x) * (t01 - t11) + (1.0 - f.y) * (t10 - t11);
}

// Billboards: clamped sampling of a sprite region (no bleeding from the
// neighbouring flipbook frame).
// Transparent texels are cut out (alpha test, like the N64/3DS paths), so
// they neither cover nor write depth over what is behind them.
@fragment
fn fs_tri_clamp(in: TriOut) -> @location(0) vec4<f32> {
    let texel = textureSample(t_sprite, s_sprite, in.uv);
    var c = texel * in.color;
    if globals.look == 1u {
        c = sample_3point(in.uv) * in.color;
    }
    if texel.a < 0.5 {
        discard;
    }
    if globals.look == 1u {
        return quantize_5551(c, vec2<u32>(in.clip.xy));
    }
    return c;
}

@fragment
fn fs_tri(in: TriOut) -> @location(0) vec4<f32> {
    // Sampled unconditionally: textureSample requires uniform control flow.
    let detail = detail_texel(in.uv, in.tex);
    let plain = textureSample(t_sprite, s_sprite, in.uv) * in.color * detail;
    if globals.look == 1u {
        let point = in.tex.x > 0.5;
        var d = vec4<f32>(1.0);
        if in.tex.y > 0.0 {
            d = vec4<f32>(sample_n64(t_detail, in.uv * in.tex.y, point).rgb, 1.0);
        }
        let c = sample_n64(t_sprite, in.uv, point) * in.color * d;
        return quantize_5551(c, vec2<u32>(in.clip.xy));
    }
    return plain;
}

// ---------------------------------------------------------------------------
// Lit triangle pass: the per-pixel lighting tier (native profile). The
// vertex color already holds ambient + the global light's diffuse (per
// vertex, like every target); this pass adds the local lights' diffuse,
// specular highlights and wetness per pixel. Layout: batch.rs::TriVertex,
// lights.rs::GpuSet.

struct GpuLight {
    pos_radius: vec4<f32>,
    color_spot: vec4<f32>,
    dir_cos: vec4<f32>,
    cone: vec4<f32>,
};

struct LightSet {
    sun_dir: vec4<f32>,    // xyz: direction the light travels; w: ambient
    sun_color: vec4<f32>,  // rgb; w: wetness
    eye: vec4<f32>,        // xyz: camera; w: local light count
    lights: array<GpuLight, 8>,
};

struct LightSets {
    sets: array<LightSet, 8>,
};

@group(2) @binding(0) var<uniform> light_sets: LightSets;

struct LitIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) depth: f32,
    @location(3) w: f32,
    @location(4) uv: vec2<f32>,
    @location(5) world: vec3<f32>,
    @location(6) normal: vec3<f32>,
    @location(7) base: vec4<f32>,
    @location(8) gloss: f32,
    @location(9) light_set: f32,
    @location(10) tex: vec2<f32>,
};

struct LitOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) world: vec3<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) base: vec4<f32>,
    @location(5) @interpolate(flat) params: vec2<f32>,
    @location(6) @interpolate(flat) tex: vec2<f32>,
};

@vertex
fn vs_tri_lit(in: LitIn) -> LitOut {
    let ndc = vec2<f32>(
        in.pos.x / globals.screen.x * 2.0 - 1.0,
        1.0 - in.pos.y / globals.screen.y * 2.0,
    );
    var out: LitOut;
    // Always the true view depth: world positions and normals interpolate
    // perspective-correctly (a light pool on a big floor stays round).
    out.clip = vec4<f32>(ndc, in.depth, 1.0) * in.w;
    out.color = in.color;
    out.uv = in.uv;
    out.world = in.world;
    out.normal = in.normal;
    out.base = in.base;
    out.params = vec2<f32>(in.gloss, in.light_set);
    out.tex = in.tex;
    return out;
}

// Blinn-Phong lobe for gloss 0..1: tighter and stronger as it rises.
fn specular(n: vec3<f32>, l: vec3<f32>, v: vec3<f32>, gloss: f32) -> f32 {
    let h = normalize(l + v);
    let e = mix(8.0, 256.0, gloss * gloss);
    // Energy-ish normalization keeps tight lobes from vanishing.
    return pow(max(dot(n, h), 0.0), e) * gloss * (e + 8.0) / 64.0;
}

fn shade_lit(in: LitOut, texel: vec4<f32>) -> vec4<f32> {
    var c = in.color * texel;
    if dot(in.normal, in.normal) < 1e-8 {
        return c; // unlit draw
    }
    // The set index and the mask of the lights that reach this draw.
    let packed = u32(in.params.y + 0.5);
    // Pointers, never `let` copies: a copied set puts its lights in private
    // memory, which a dynamic index then reads back from scratch per pixel.
    let lset = &light_sets.sets[packed & 7u];
    let mask = packed >> 3u;
    let n = normalize(in.normal);
    let albedo = in.base.rgb * texel.rgb;
    let v = normalize((*lset).eye.xyz - in.world);
    // Wet: surfaces facing up darken and turn glossy.
    let wet = (*lset).sun_color.w * smoothstep(0.55, 0.9, n.y);
    var gloss = max(in.params.x, wet * 0.9);
    var rgb = c.rgb * (1.0 - 0.4 * wet);
    let alb = albedo * (1.0 - 0.4 * wet);
    // The global light's highlight (its diffuse is in the vertex color).
    let ls = -normalize((*lset).sun_dir.xyz);
    if gloss > 0.0 && dot(n, ls) > 0.0 {
        rgb += (*lset).sun_color.rgb * specular(n, ls, v, gloss) * (1.0 - (*lset).sun_dir.w);
    }
    let count = u32((*lset).eye.w + 0.5);
    for (var i = 0u; i < count; i++) {
        if (mask & (1u << i)) == 0u {
            continue;
        }
        let li = &(*lset).lights[i];
        let pr = (*li).pos_radius;
        let to = pr.xyz - in.world;
        let d2 = dot(to, to);
        let r2 = pr.w * pr.w;
        if d2 >= r2 || d2 <= 0.0 {
            continue;
        }
        let k = 1.0 - d2 / r2;
        var att = k * k;
        let l = to * inverseSqrt(d2);
        let cs = (*li).color_spot;
        if cs.w > 0.5 {
            let dc = (*li).dir_cos;
            let t = clamp((dot(-l, dc.xyz) - dc.w) * (*li).cone.x, 0.0, 1.0);
            att *= t * t * (3.0 - 2.0 * t);
        }
        let ndl = max(dot(n, l), 0.0);
        rgb += alb * cs.rgb * att * ndl;
        if gloss > 0.0 && ndl > 0.0 {
            rgb += cs.rgb * att * specular(n, l, v, gloss);
        }
    }
    return vec4<f32>(rgb, c.a);
}

@fragment
fn fs_tri_lit(in: LitOut) -> @location(0) vec4<f32> {
    // Sampled unconditionally: textureSample requires uniform control flow.
    let texel = textureSample(t_sprite, s_sprite, in.uv) * detail_texel(in.uv, in.tex);
    let c = shade_lit(in, texel);
    if globals.look == 1u {
        return quantize_5551(c, vec2<u32>(in.clip.xy));
    }
    return c;
}

// ---------------------------------------------------------------------------
// Water pass (ADR 0015): translucent, depth-tested without writing depth,
// drawn after the batch's other triangles. The vertices are the engine's
// displaced, lit water vertices (trino_core::water) with their surface
// (world position, normal, unlit color) and their water set (water.rs).
// - fs_water_vertex: the console tiers (N64 look, 3DS profile): the two
//   flowing layers blended half and half times the vertex color, like the
//   N64's 2-cycle combiner and the 3DS's TexEnv stages.
// - fs_water: the per-pixel tier (native): the swell's and the ripples'
//   normals per pixel, Schlick Fresnel, the sun's glint and soft foam where
//   the water meets the ground (the scene's depth, copied before the pass).

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
};

struct WaterSets {
    sets: array<WaterSet, 8>,
};

@group(3) @binding(0) var<uniform> water_sets: WaterSets;
@group(3) @binding(1) var t_scene_depth: texture_depth_2d;

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

@fragment
fn fs_water_vertex(in: LitOut) -> @location(0) vec4<f32> {
    let ws = &water_sets.sets[u32(in.params.y + 0.5) & 7u];
    let scale = in.tex.y;
    let duv = water_detail_uv(in.uv, scale, (*ws).detail.yz);
    // Sampled unconditionally: textureSample requires uniform control flow.
    var t0 = textureSample(t_sprite, s_sprite, in.uv);
    var t1 = textureSample(t_detail, s_detail, duv);
    if globals.look == 1u {
        let point = in.tex.x > 0.5;
        t0 = sample_n64(t_sprite, in.uv, point);
        t1 = sample_n64(t_detail, duv, point);
    }
    var texel = t0.rgb;
    if scale > 0.0 {
        texel = mix(t0.rgb, t1.rgb, 0.5);
    }
    let c = vec4<f32>(in.color.rgb * texel, in.color.a);
    if globals.look == 1u {
        // The RDP blends with the full alpha; the stored color is 5551.
        let q = quantize_5551(c, vec2<u32>(in.clip.xy));
        return vec4<f32>(q.rgb, c.a);
    }
    return c;
}

// The swell's and the ripples' height gradient at world (x, z) (xy), and
// the ripples' height (z): the functions the core displaces the vertices
// with, exact here and per pixel.
fn water_gradient(si: u32, p: vec2<f32>) -> vec3<f32> {
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
        g += d * (a * W_TAU * k * cos(W_TAU * t));
    }
    let n = u32((*ws).detail.w + 0.5);
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
        let s = r.w * amp.z * decay;
        let t = (d - rad) / RIPPLE_WAVELENGTH;
        let de = 2.0 * e * (-2.0 * q / RIPPLE_WIDTH);
        let dh = s * (de * cos(W_TAU * t) - e * e * (W_TAU / RIPPLE_WAVELENGTH) * sin(W_TAU * t));
        g += dv * (dh / d);
        rh += s * e * e * cos(W_TAU * t);
    }
    // Detail the vertices can't carry: two short, fast capillary waves.
    let clock = (*ws).sun_color.w;
    let fine = select(0.5, 1.0, amp.x > 0.0);
    let c1 = vec2<f32>(0.83, 0.56);
    let c2 = vec2<f32>(-0.42, 0.91);
    g += c1 * (0.012 * fine * W_TAU * 2.3 * cos(W_TAU * (2.3 * dot(c1, p) - 1.3 * clock)));
    g += c2 * (0.008 * fine * W_TAU * 3.7 * cos(W_TAU * (3.7 * dot(c2, p) - 1.7 * clock)));
    return vec3<f32>(g, rh);
}

@fragment
fn fs_water(in: LitOut) -> @location(0) vec4<f32> {
    let si = u32(in.params.y + 0.5) & 7u;
    let ws = &water_sets.sets[si];
    let scale = in.tex.y;
    let duv = water_detail_uv(in.uv, scale, (*ws).detail.yz);
    let t0 = textureSample(t_sprite, s_sprite, in.uv);
    let t1 = textureSample(t_detail, s_detail, duv);
    var texel = t0.rgb;
    if scale > 0.0 {
        texel = mix(t0.rgb, t1.rgb, 0.5);
    }
    let p = in.world;
    let gh = water_gradient(si, p.xz);
    let n = normalize(vec3<f32>(-gh.x, 1.0, -gh.y));
    // The global light from the per-pixel normal.
    let sun = -normalize((*ws).sun_dir.xyz);
    let amb = (*ws).sun_dir.w;
    let k = amb * (*ws).ambient.rgb + (1.0 - amb) * max(dot(n, sun), 0.0) * (*ws).sun_color.rgb;
    var rgb = in.base.rgb * texel * k;
    // Fresnel: the sky at grazing angles, where water also turns opaque.
    let v = normalize((*ws).eye.xyz - p);
    let f = 0.02 + 0.98 * pow(1.0 - max(dot(n, v), 0.0), 5.0);
    rgb = mix(rgb, (*ws).sky.rgb, f * 0.85);
    var a = in.base.a + (1.0 - in.base.a) * f;
    // The sun's glint.
    let gloss = (*ws).amp.w;
    if gloss > 0.0 && dot(n, sun) > 0.0 {
        rgb += (*ws).sun_color.rgb * specular(n, sun, v, gloss) * 1.5;
    }
    // Depth: the water column under this pixel, from the scene's depth.
    let zw = dot(p - (*ws).eye.xyz, (*ws).fwd.xyz);
    let dz = textureLoad(t_scene_depth, vec2<i32>(in.clip.xy), 0);
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

// ---------------------------------------------------------------------------
// Blit pass: offscreen framebuffer -> window surface, nearest-neighbor.
// Integer scaling and letterboxing are done with the render-pass viewport.

struct BlitOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) vi: u32) -> BlitOut {
    // Fullscreen triangle.
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: BlitOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

@group(0) @binding(0) var t_frame: texture_2d<f32>;
@group(0) @binding(1) var s_frame: sampler;

@fragment
fn fs_blit(in: BlitOut) -> @location(0) vec4<f32> {
    return textureSample(t_frame, s_frame, in.uv);
}

// N64 Video Interface dedither ("restore") filter, as angrylion-rdp-plus
// implements it (vi/restore.c): for each channel, the pixel's 5-bit value
// x8 plus, for each of its 8 neighbours, +1 if the neighbour's 5-bit value
// is higher and -1 if lower (on top of the stored expansion, see
// quantize_5551).
fn vi_c5(c: vec4<f32>) -> vec3<f32> {
    return floor(floor(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0 + 0.5) / 8.0);
}

@fragment
fn fs_vi_dedither(in: BlitOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    let hi = vec2<i32>(textureDimensions(t_frame)) - vec2<i32>(1, 1);
    let center = textureLoad(t_frame, p, 0);
    let c5 = vi_c5(center);
    var sum = vec3<f32>(0.0);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            if dx == 0 && dy == 0 {
                continue;
            }
            let q = clamp(p + vec2<i32>(dx, dy), vec2<i32>(0, 0), hi);
            sum += sign(vi_c5(textureLoad(t_frame, q, 0)) - c5);
        }
    }
    return vec4<f32>(clamp(expand5(c5) + sum / 255.0, vec3<f32>(0.0), vec3<f32>(1.0)), center.a);
}
