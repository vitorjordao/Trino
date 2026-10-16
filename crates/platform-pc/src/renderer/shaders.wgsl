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
};

struct TriOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

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
    return out;
}

fn wrap_texel(p: vec2<i32>, dims: vec2<i32>) -> vec2<i32> {
    return ((p % dims) + dims) % dims;
}

// 3-point sampling for wrapping (tiled) 3D textures.
fn sample_3point_wrap(uv: vec2<f32>) -> vec4<f32> {
    let dims = vec2<i32>(textureDimensions(t_sprite));
    let st = uv * vec2<f32>(dims) - vec2<f32>(0.5, 0.5);
    let base = floor(st);
    let f = st - base;
    let b = vec2<i32>(base);
    let t00 = textureLoad(t_sprite, wrap_texel(b, dims), 0);
    let t10 = textureLoad(t_sprite, wrap_texel(b + vec2<i32>(1, 0), dims), 0);
    let t01 = textureLoad(t_sprite, wrap_texel(b + vec2<i32>(0, 1), dims), 0);
    let t11 = textureLoad(t_sprite, wrap_texel(b + vec2<i32>(1, 1), dims), 0);
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
    let plain = textureSample(t_sprite, s_sprite, in.uv) * in.color;
    if globals.look == 1u {
        return quantize_5551(sample_3point_wrap(in.uv) * in.color, vec2<u32>(in.clip.xy));
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
};

struct LitOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) world: vec3<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) base: vec4<f32>,
    @location(5) @interpolate(flat) params: vec2<f32>,
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
    let lset = light_sets.sets[packed & 7u];
    let mask = packed >> 3u;
    let n = normalize(in.normal);
    let albedo = in.base.rgb * texel.rgb;
    let v = normalize(lset.eye.xyz - in.world);
    // Wet: surfaces facing up darken and turn glossy.
    let wet = lset.sun_color.w * smoothstep(0.55, 0.9, n.y);
    var gloss = max(in.params.x, wet * 0.9);
    var rgb = c.rgb * (1.0 - 0.4 * wet);
    let alb = albedo * (1.0 - 0.4 * wet);
    // The global light's highlight (its diffuse is in the vertex color).
    let ls = -normalize(lset.sun_dir.xyz);
    if gloss > 0.0 && dot(n, ls) > 0.0 {
        rgb += lset.sun_color.rgb * specular(n, ls, v, gloss) * (1.0 - lset.sun_dir.w);
    }
    let count = u32(lset.eye.w + 0.5);
    for (var i = 0u; i < count; i++) {
        if (mask & (1u << i)) == 0u {
            continue;
        }
        let li = lset.lights[i];
        let to = li.pos_radius.xyz - in.world;
        let d2 = dot(to, to);
        let r2 = li.pos_radius.w * li.pos_radius.w;
        if d2 >= r2 || d2 <= 0.0 {
            continue;
        }
        let k = 1.0 - d2 / r2;
        var att = k * k;
        let l = to * inverseSqrt(d2);
        if li.color_spot.w > 0.5 {
            let t = clamp((dot(-l, li.dir_cos.xyz) - li.dir_cos.w) * li.cone.x, 0.0, 1.0);
            att *= t * t * (3.0 - 2.0 * t);
        }
        let ndl = max(dot(n, l), 0.0);
        rgb += alb * li.color_spot.rgb * att * ndl;
        if gloss > 0.0 && ndl > 0.0 {
            rgb += li.color_spot.rgb * att * specular(n, l, v, gloss);
        }
    }
    return vec4<f32>(rgb, c.a);
}

@fragment
fn fs_tri_lit(in: LitOut) -> @location(0) vec4<f32> {
    // Sampled unconditionally: textureSample requires uniform control flow.
    let texel = textureSample(t_sprite, s_sprite, in.uv);
    let c = shade_lit(in, texel);
    if globals.look == 1u {
        return quantize_5551(c, vec2<u32>(in.clip.xy));
    }
    return c;
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
