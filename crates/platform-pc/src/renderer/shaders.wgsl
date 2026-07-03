// Sprite pass: instanced quads onto the internal offscreen framebuffer.
// Instance layout must match batch.rs::Instance (stride 56).

struct Globals {
    // Internal framebuffer resolution in pixels.
    screen: vec2<f32>,
    _pad: vec2<f32>,
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

@fragment
fn fs_sprite(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(t_sprite, s_sprite, in.uv) * in.tint;
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
