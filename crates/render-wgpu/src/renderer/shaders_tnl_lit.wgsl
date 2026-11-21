// Appended to shaders_tnl.wgsl (gpu_tnl.rs `MESH_WGSL`).
// ---------------------------------------------------------------------------
// Per-pixel lighting on the GPU path: the draws a local light reaches, or
// glossy or wet ones (`LightTier::PerPixel`), shaded as the CPU path's lit
// pipeline shades them (`fs_tri_lit`, `shade_lit` in shaders.wgsl): the
// vertex color carries the sun's diffuse, the fragment adds the local
// lights, the specular and the wetness. The light sets are the lit
// pipeline's (lights.rs::GpuSet), bound as group 3.

const FLAG_LIT: u32 = 16u;

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

@group(3) @binding(0) var<uniform> light_sets: LightSets;

struct MeshLitOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) world: vec3<f32>,
    @location(3) normal: vec3<f32>,
    @location(4) base: vec4<f32>,
    // x: gloss 0..1, y: the light set | mask << 3, z: the detail scale.
    @location(5) @interpolate(flat) params: vec3<f32>,
};

@vertex
fn vs_mesh_lit(v: MeshIn, inst: InstIn) -> MeshLitOut {
    let m = mesh_vertex(v, inst);
    let flags = inst.misc.z;
    var out: MeshLitOut;
    out.clip = m.out.clip;
    // Always perspective-correct, as the CPU path's lit triangles.
    out.color = m.out.color;
    out.uv = m.out.uv;
    out.world = m.world;
    out.normal = m.normal;
    out.base = m.base;
    out.params = vec3<f32>(
        f32((flags >> 8u) & 255u) / 255.0,
        f32((flags >> 16u) & 2047u),
        m.out.params.y,
    );
    return out;
}

@fragment
fn fs_mesh_lit(in: MeshLitOut) -> @location(0) vec4<f32> {
    return mesh_lit_color(in);
}

@fragment
fn fs_mesh_lit_cut(in: MeshLitOut) -> @location(0) vec4<f32> {
    let c = mesh_lit_color(in);
    if c.a < 0.5 {
        discard;
    }
    return vec4<f32>(c.rgb, 1.0);
}

fn mesh_lit_color(in: MeshLitOut) -> vec4<f32> {
    // Sampled unconditionally: textureSample requires uniform control flow.
    let texel = textureSample(t_sprite, s_sprite, in.uv) * detail_factor(in.uv, in.params.z);
    let c = in.color * texel;
    if dot(in.normal, in.normal) < 1e-8 {
        return c;
    }
    let packed = u32(in.params.y + 0.5);
    let lset = &light_sets.sets[packed & 7u];
    let mask = packed >> 3u;
    let n = normalize(in.normal);
    let albedo = in.base.rgb * texel.rgb;
    let v = normalize((*lset).eye.xyz - in.world);
    let wet = (*lset).sun_color.w * smoothstep(0.55, 0.9, n.y);
    let gloss = max(in.params.x, wet * 0.9);
    var rgb = c.rgb * (1.0 - 0.4 * wet);
    let alb = albedo * (1.0 - 0.4 * wet);
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

