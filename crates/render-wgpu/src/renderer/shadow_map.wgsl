// Shadow maps (the PC's Map tier, ADR 0024).
//
// Two passes, after the 3D and before the screen-space AO and the 2D:
//   vs_light          the casters' world triangles into a depth map from
//                     the light (orthographic, `trino_core::shadow::map_fit`),
//                     depth only;
//   fs_shadow_map     a fullscreen pass over the scene's depth: each pixel's
//                     world position (the T&L's projection run backwards,
//                     ao.wgsl's way), pushed off its surface along the
//                     normal rebuilt from its neighbors, then looked up in
//                     the map through a comparison sampler on a grid of
//                     taps (PCF); the shadow color is blended over the
//                     frame by how much of the light the taps found
//                     blocked, faded out toward the shadow distance, the
//                     fog and surfaces turning away from the light (the
//                     lighting already darkens those).
// The map's texture type is spelled SHADOWMAP here: the MSAA and WebGL2
// variants rewrite the scene depth's type, and the map's is set after.

struct MapParams {
    size: vec2<f32>,
    center: vec2<f32>,
    focal: f32,
    near: f32,
    fog_near: f32,
    // <= fog_near: no fog fade.
    fog_far: f32,
    // View to world, three rows (the view matrix's inverse).
    inv0: vec4<f32>,
    inv1: vec4<f32>,
    inv2: vec4<f32>,
    // World to map: x, y -1..1, z 0..1 from the light.
    map0: vec4<f32>,
    map1: vec4<f32>,
    map2: vec4<f32>,
    // The shadow color; alpha is the strength.
    color: vec4<f32>,
    // Toward the light, world (w unused).
    to_light: vec4<f32>,
    // How far along the normal a pixel looks up the map, world units.
    offset: f32,
    // Map depth a pixel must be behind the caster by to be shadowed.
    bias: f32,
    // Tap spacing, map uv.
    spacing: f32,
    distance: f32,
    // Taps per side.
    taps: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};

@group(0) @binding(0) var<uniform> P: MapParams;
@group(0) @binding(1) var t_depth: texture_depth_2d;
@group(0) @binding(2) var t_shadow: SHADOWMAP;
@group(0) @binding(4) var s_shadow: sampler_comparison;

struct FullOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FullOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: FullOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

fn hi() -> vec2<i32> {
    return vec2<i32>(P.size) - vec2<i32>(1, 1);
}

// The depth at pixel `p` (the one read of it: WebGL2 reads it as a float
// texture, `depth_as_float` in mod.rs).
fn load_depth(p: vec2<i32>) -> f32 {
    return textureLoad(t_depth, p, 0);
}

fn depth_at(p: vec2<i32>) -> f32 {
    return load_depth(clamp(p, vec2<i32>(0, 0), hi()));
}

// View depth of a stored depth (`d = 1 - NEAR/z`).
fn view_z(d: f32) -> f32 {
    return P.near / max(1.0 - d, 1e-7);
}

// View-space position (x right, y up, z forward) of pixel `p` at depth `d`.
fn view_pos(p: vec2<i32>, d: f32) -> vec3<f32> {
    let z = view_z(d);
    let s = vec2<f32>(p) + vec2<f32>(0.5, 0.5);
    return vec3<f32>((s.x - P.center.x) * z / P.focal, -(s.y - P.center.y) * z / P.focal, z);
}

fn pos_at(p: vec2<i32>) -> vec3<f32> {
    let q = clamp(p, vec2<i32>(0, 0), hi());
    return view_pos(q, depth_at(q));
}

fn to_world(v: vec3<f32>) -> vec3<f32> {
    let h = vec4<f32>(v, 1.0);
    return vec3<f32>(dot(P.inv0, h), dot(P.inv1, h), dot(P.inv2, h));
}

fn dir_to_world(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(P.inv0.xyz, v), dot(P.inv1.xyz, v), dot(P.inv2.xyz, v));
}

@fragment
fn fs_shadow_map(in: FullOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    let d = depth_at(p);
    if d >= 1.0 {
        // Background (cleared): nothing to shadow.
        discard;
    }
    let c = view_pos(p, d);
    if c.z >= P.distance {
        discard;
    }
    // The normal from the neighbor nearer in depth on each axis (ao.wgsl):
    // a silhouette doesn't bend the surface's behind it.
    let l = pos_at(p - vec2<i32>(1, 0));
    let r = pos_at(p + vec2<i32>(1, 0));
    let u = pos_at(p - vec2<i32>(0, 1));
    let b = pos_at(p + vec2<i32>(0, 1));
    let dx = select(c - l, r - c, abs(r.z - c.z) < abs(c.z - l.z));
    let dy = select(c - u, b - c, abs(b.z - c.z) < abs(c.z - u.z));
    var n = normalize(cross(dx, dy));
    if dot(n, c) > 0.0 {
        n = -n;
    }
    let wn = normalize(dir_to_world(n));
    let ndl = dot(wn, P.to_light.xyz);
    if ndl <= 0.0 {
        discard;
    }
    // Off the surface by more the more it slopes away from the light: the
    // map's texels are longer along it there.
    let q = vec4<f32>(to_world(c) + wn * P.offset * (2.0 - ndl), 1.0);
    let m = vec3<f32>(dot(P.map0, q), dot(P.map1, q), dot(P.map2, q));
    let edge = max(abs(m.x), abs(m.y));
    if edge >= 1.0 || m.z >= 1.0 {
        discard;
    }
    let uv = vec2<f32>(m.x * 0.5 + 0.5, 0.5 - m.y * 0.5);
    let reference = m.z - P.bias;
    let k = (f32(P.taps) - 1.0) * 0.5;
    var lit = 0.0;
    for (var j = 0u; j < P.taps; j = j + 1u) {
        for (var i = 0u; i < P.taps; i = i + 1u) {
            let o = (vec2<f32>(f32(i), f32(j)) - vec2<f32>(k, k)) * P.spacing;
            lit = lit + textureSampleCompareLevel(t_shadow, s_shadow, uv + o, reference);
        }
    }
    let blocked = 1.0 - lit / f32(P.taps * P.taps);
    var fog = 0.0;
    if P.fog_far > P.fog_near {
        fog = clamp((c.z - P.fog_near) / (P.fog_far - P.fog_near), 0.0, 1.0);
    }
    let fade = clamp((P.distance - c.z) / (0.2 * P.distance), 0.0, 1.0)
        * (1.0 - smoothstep(0.9, 1.0, edge))
        * (1.0 - fog)
        * smoothstep(0.0, 0.2, ndl);
    let a = P.color.a * blocked * fade;
    if a <= 0.0 {
        discard;
    }
    return vec4<f32>(P.color.rgb, a);
}
