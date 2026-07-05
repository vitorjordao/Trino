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

// 16-bit framebuffer approximation: RDP magic-square ordered dither, then
// RGBA5551 quantization (5 bits per channel, 1-bit alpha).
fn quantize_5551(color: vec4<f32>, px: vec2<u32>) -> vec4<f32> {
    var magic = array<f32, 16>(
        0.0, 6.0, 1.0, 7.0,
        4.0, 2.0, 5.0, 3.0,
        3.0, 5.0, 2.0, 4.0,
        7.0, 1.0, 6.0, 0.0,
    );
    let d = (magic[(px.y % 4u) * 4u + (px.x % 4u)] - 3.5) / 8.0;
    let rgb = clamp(
        floor(clamp(color.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 31.0 + 0.5 + d) / 31.0,
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    let a = select(0.0, 1.0, color.a >= 0.5);
    return vec4<f32>(rgb, a);
}

@fragment
fn fs_sprite(in: VsOut) -> @location(0) vec4<f32> {
    // Sampled unconditionally: textureSample requires uniform control flow.
    let plain = textureSample(t_sprite, s_sprite, in.uv) * in.tint;
    if globals.look == 1u {
        let c = sample_3point(in.uv) * in.tint;
        return quantize_5551(c, vec2<u32>(in.clip.xy));
    }
    return plain;
}

// ---------------------------------------------------------------------------
// Triangle pass (3D): screen-space vertex-colored triangles produced by the
// engine's software T&L (trino_core::render3d). Same offscreen target and
// N64-look quantization as sprites.

struct TriIn {
    @location(0) pos: vec2<f32>,   // pixels, internal resolution
    @location(1) color: vec4<f32>,
};

struct TriOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_tri(in: TriIn) -> TriOut {
    let ndc = vec2<f32>(
        in.pos.x / globals.screen.x * 2.0 - 1.0,
        1.0 - in.pos.y / globals.screen.y * 2.0,
    );
    var out: TriOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_tri(in: TriOut) -> @location(0) vec4<f32> {
    if globals.look == 1u {
        return quantize_5551(in.color, vec2<u32>(in.clip.xy));
    }
    return in.color;
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
