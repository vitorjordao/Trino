// GPU vertex T&L (issue #51, ADR 0016): the PC native profile's draws that
// `trino_core::render3d::pc_gpu_check` accepts. The vertex stage does what
// the CPU path (`tessellate_with`) does per vertex — model-view transform,
// rigid joints and smooth skinning, Gouraud / Flat / Unlit against the
// directional light and the ambient floor, palette slots, tint, emissive,
// fog — and the fragment stage adds the texture with its detail layer,
// like `fs_tri`. The hardware clips (near plane included: clip z is
// `view z - NEAR`, so depth is the CPU path's `1 - NEAR / z`) and culls.
// Layouts: gpu_tnl.rs (`MeshVertex`, `GpuInstance`, `MeshCtx`).

struct Globals {
    screen: vec2<f32>,
    look: u32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> globals: Globals;

// The draw's texture and detail layer (white when it has none): the same
// group as the screen-space triangle pass.
@group(1) @binding(0) var t_sprite: texture_2d<f32>;
@group(1) @binding(1) var s_sprite: sampler;
@group(1) @binding(2) var t_detail: texture_2d<f32>;
@group(1) @binding(3) var s_detail: sampler;

// What `TnlContext` derives from the camera, the light and the screen.
struct MeshCtx {
    // x: 2 * focal / width, y: 2 * focal / height, z: the near plane.
    proj: vec4<f32>,
    // xyz: the normalized direction the light travels; w: ambient.
    sun: vec4<f32>,
    // rgb: the light color as 0..1 factors; w: 1 when both colors are
    // white (the single-intensity path).
    sun_color: vec4<f32>,
    // rgb: the ambient color as 0..1 factors.
    ambient: vec4<f32>,
    // rgb: the fog color, 0..255; w: 1 when fog is on.
    fog_color: vec4<f32>,
    // x: fog near, y: fog far.
    fog_range: vec4<f32>,
};

@group(2) @binding(0) var<uniform> ctx: MeshCtx;

// A draw's joint matrices in model space (`Mesh::pose`): 32 rows of three.
struct Joints {
    rows: array<vec4<f32>, 96>,
};

@group(2) @binding(1) var<uniform> joints: Joints;

struct MeshIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // The triangle's model-space `(p1 - p0) x (p2 - p0)` (flat shading).
    @location(2) face: vec3<f32>,
    @location(3) color: vec4<u32>,
    @location(4) uv: vec2<f32>,
    // x: joint, y: second joint (smooth skinning), z: color slot.
    @location(5) skin: vec4<u32>,
    // The second joint's weight (0 without `SKW2`).
    @location(6) weight: f32,
};

struct InstIn {
    // The model matrix and model-view matrix, rows.
    @location(7) m0: vec4<f32>,
    @location(8) m1: vec4<f32>,
    @location(9) m2: vec4<f32>,
    @location(10) mv0: vec4<f32>,
    @location(11) mv1: vec4<f32>,
    @location(12) mv2: vec4<f32>,
    // x: tint, y: emissive (RGBA8 packed), z: flags, w: the detail
    // layer's UV scale (f32 bits, 0 = none).
    @location(13) misc: vec4<u32>,
    // The palette's four colors (RGBA8 packed; white = unchanged).
    @location(14) palette: vec4<u32>,
};

// Flags (`GpuInstance::misc.z`).
const SHADING_MASK: u32 = 3u;
const SHADING_FLAT: u32 = 1u;
const SHADING_UNLIT: u32 = 2u;
const FLAG_AFFINE: u32 = 4u;
const FLAG_SKINNED: u32 = 8u;

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    // The fogged color, interpolated perspective-correctly (textured or
    // fogged draws: the CPU path splits those) or affinely (the rest: the
    // CPU path's w = 1 triangles). Affine is `(color * w) / w`, both
    // interpolated perspective-correctly: `@interpolate(linear)` loses
    // clipped triangles on Mesa's llvmpipe (CI's lavapipe).
    @location(0) color: vec4<f32>,
    @location(1) color_w: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) view_z: f32,
    // x: 1 = affine color, y: the detail layer's UV scale.
    @location(4) @interpolate(flat) params: vec2<f32>,
};

fn unpack(c: u32) -> vec4<u32> {
    return vec4<u32>(c & 255u, (c >> 8u) & 255u, (c >> 16u) & 255u, c >> 24u);
}

fn xform_point(r0: vec4<f32>, r1: vec4<f32>, r2: vec4<f32>, p: vec3<f32>) -> vec3<f32> {
    let q = vec4<f32>(p, 1.0);
    return vec3<f32>(dot(r0, q), dot(r1, q), dot(r2, q));
}

fn xform_dir(r0: vec4<f32>, r1: vec4<f32>, r2: vec4<f32>, d: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(r0.xyz, d), dot(r1.xyz, d), dot(r2.xyz, d));
}

// Row `a` of a 3x4 product `A * B` (`Mat34::mul`), B's rows given.
fn mul_row(a: vec4<f32>, b0: vec4<f32>, b1: vec4<f32>, b2: vec4<f32>) -> vec4<f32> {
    return a.x * b0 + a.y * b1 + a.z * b2 + vec4<f32>(0.0, 0.0, 0.0, a.w);
}

// One joint's posed position and normal, in view and world space.
struct Posed {
    view: vec3<f32>,
    normal: vec3<f32>,
};

fn posed(j: u32, inst: InstIn, pos: vec3<f32>, n: vec3<f32>) -> Posed {
    let s0 = joints.rows[j * 3u];
    let s1 = joints.rows[j * 3u + 1u];
    let s2 = joints.rows[j * 3u + 2u];
    var out: Posed;
    out.view = xform_point(
        mul_row(inst.mv0, s0, s1, s2),
        mul_row(inst.mv1, s0, s1, s2),
        mul_row(inst.mv2, s0, s1, s2),
        pos,
    );
    out.normal = xform_dir(
        mul_row(inst.m0, s0, s1, s2),
        mul_row(inst.m1, s0, s1, s2),
        mul_row(inst.m2, s0, s1, s2),
        n,
    );
    return out;
}

@vertex
fn vs_mesh(v: MeshIn, inst: InstIn) -> MeshOut {
    let flags = inst.misc.z;
    let shading = flags & SHADING_MASK;
    var p: vec3<f32>;
    var n: vec3<f32>;
    if (flags & FLAG_SKINNED) != 0u {
        // Rigid joints, and the second joint blended in by its weight
        // (0 = exactly the first: `p + (p2 - p) * 0`).
        let a = posed(v.skin.x, inst, v.pos, v.normal);
        let b = posed(v.skin.y, inst, v.pos, v.normal);
        p = a.view + (b.view - a.view) * v.weight;
        n = a.normal + (b.normal - a.normal) * v.weight;
    } else {
        p = xform_point(inst.mv0, inst.mv1, inst.mv2, v.pos);
        n = xform_dir(inst.m0, inst.m1, inst.m2, v.normal);
    }
    if shading == SHADING_FLAT {
        // The world-space face normal: (M a) x (M b) = cof(M) (a x b), with
        // the cofactor matrix's columns c1 x c2, c2 x c0, c0 x c1 (static
        // meshes only: `PcGpuReject::FlatSkinned`).
        let c0 = vec3<f32>(inst.m0.x, inst.m1.x, inst.m2.x);
        let c1 = vec3<f32>(inst.m0.y, inst.m1.y, inst.m2.y);
        let c2 = vec3<f32>(inst.m0.z, inst.m1.z, inst.m2.z);
        n = cross(c1, c2) * v.face.x + cross(c2, c0) * v.face.y + cross(c0, c1) * v.face.z;
    }

    // Light factors (`Setup::shade_with`).
    var k = vec3<f32>(1.0);
    if shading != SHADING_UNLIT {
        let diffuse = -dot(normalize(n), ctx.sun.xyz);
        let d = (1.0 - ctx.sun.w) * select(0.0, diffuse, diffuse > 0.0);
        if ctx.sun_color.w > 0.5 {
            k = vec3<f32>(ctx.sun.w + d);
        } else {
            k = ctx.sun.w * ctx.ambient.rgb + d * ctx.sun_color.rgb;
        }
    }

    // The baked color times its palette entry (`palette_mul`: rounded,
    // slot 0 kept), then tint x light, then emissive (`finish_unfogged`).
    var c = v.color;
    let slot = min(v.skin.z, 4u);
    if slot > 0u {
        var entry = inst.palette.x;
        if slot == 2u {
            entry = inst.palette.y;
        } else if slot == 3u {
            entry = inst.palette.z;
        } else if slot == 4u {
            entry = inst.palette.w;
        }
        let e = unpack(entry);
        c = vec4<u32>((c.rgb * e.rgb + vec3<u32>(127u)) / 255u, c.a);
    }
    let tint = vec4<f32>(unpack(inst.misc.x)) / 255.0;
    let lit = min(floor(vec3<f32>(c.rgb) * tint.rgb * k), vec3<f32>(255.0));
    let glow = vec3<f32>(unpack(inst.misc.y).rgb);
    let rgb = min(lit + glow, vec3<f32>(255.0));
    let alpha = floor(f32(c.a) * tint.a);
    // Fog per vertex by view depth (`Setup::fog`, truncated like
    // `lerp_color`), interpolated like the CPU path's vertex colors: the
    // oracle. Per pixel looked better on big floors the CPU path doesn't
    // split, and missed the CPU frame by 2.7/255 on the showcase river.
    var fogged = rgb;
    if ctx.fog_color.w > 0.5 {
        let f = clamp((p.z - ctx.fog_range.x) / (ctx.fog_range.y - ctx.fog_range.x), 0.0, 1.0);
        fogged = floor(rgb + (ctx.fog_color.rgb - rgb) * f);
    }
    let color = vec4<f32>(fogged, alpha) / 255.0;

    var out: MeshOut;
    out.clip = vec4<f32>(p.x * ctx.proj.x, p.y * ctx.proj.y, p.z - ctx.proj.z, p.z);
    out.color = color;
    out.color_w = color * p.z;
    out.uv = v.uv;
    out.view_z = p.z;
    out.params = vec2<f32>(
        select(0.0, 1.0, (flags & FLAG_AFFINE) != 0u),
        bitcast<f32>(inst.misc.w),
    );
    return out;
}

// The detail layer's factor (white without one): `detail_texel` in
// shaders.wgsl.
fn detail_factor(uv: vec2<f32>, scale: f32) -> vec4<f32> {
    let d = textureSample(t_detail, s_detail, uv * max(scale, 1.0));
    if scale > 0.0 {
        return vec4<f32>(d.rgb, 1.0);
    }
    return vec4<f32>(1.0);
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    // Sampled unconditionally: textureSample requires uniform control flow.
    let detail = detail_factor(in.uv, in.params.y);
    let texel = textureSample(t_sprite, s_sprite, in.uv);
    var c = in.color;
    if in.params.x > 0.5 {
        c = in.color_w / in.view_z;
    }
    return texel * c * detail;
}
