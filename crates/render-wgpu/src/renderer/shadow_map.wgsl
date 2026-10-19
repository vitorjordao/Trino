// Shadow maps (the PC's Map tier, ADR 0025).
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
    // Taps per side (the widest filter has twice as many).
    taps: u32,
    // Filter radius gained per world unit between blocker and receiver,
    // map uv (0: an even filter).
    growth: f32,
    // Blocker distance at which the shadow is gone, world (0: no fade).
    fade: f32,
    // The map's depth range, world: map z to world units.
    depth: f32,
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

// The map's depth at texel `q` (the blocker search: no comparison). WebGL2
// reads it through a second, float binding of the map and a nearest
// sampler (shadow_map.rs rewrites this line).
fn map_depth(q: vec2<i32>) -> f32 {
    return textureLoad(t_shadow, q, 0);
}

// The map texels in front of `reference` on an `n`×`n` grid `step` (uv)
// apart around `uv`: the sum of their depths and how many (x, y). A texel
// counts once it is in front by more than a texel's depth per texel it is
// off center, so a receiver sloping up to 45° to the light doesn't find
// itself.
fn blockers(uv: vec2<f32>, reference: f32, n: u32, step: f32, texel: f32) -> vec2<f32> {
    let size = vec2<f32>(textureDimensions(t_shadow));
    let hi_t = vec2<i32>(size) - vec2<i32>(1, 1);
    let c = (f32(n) - 1.0) * 0.5;
    var sum = 0.0;
    var found = 0.0;
    for (var j = 0u; j < n; j = j + 1u) {
        for (var i = 0u; i < n; i = i + 1u) {
            let o = (vec2<f32>(f32(i), f32(j)) - vec2<f32>(c, c)) * step;
            let q = clamp(vec2<i32>((uv + o) * size), vec2<i32>(0, 0), hi_t);
            let d = map_depth(q);
            if d < reference - P.bias * length(o) / texel {
                sum = sum + d;
                found = found + 1.0;
            }
        }
    }
    return vec2<f32>(sum, found);
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
    var side = P.taps;
    var spacing = P.spacing;
    var lift = 1.0;
    if P.growth > 0.0 || P.fade > 0.0 {
        // The widest filter: three times the taps per side, 2.5 texels
        // apart (a bilinear compare covers 2), or the even filter if wider.
        let texel = 1.0 / f32(textureDimensions(t_shadow).x);
        let wide = 3u * P.taps;
        let gap = max(P.spacing, 2.5 * texel);
        let base = (f32(P.taps) - 1.0) * 0.5 * P.spacing;
        let widest = max(base, (f32(wide) - 1.0) * 0.5 * gap);
        // Blockers where the even filter looks (its taps, so the two agree
        // on thin casters), then a sparse grid as wide as the filter grows.
        let near = blockers(uv, reference, P.taps, P.spacing, texel);
        let far = blockers(uv, reference, 4u, widest * 2.0 / 3.0, texel);
        let found = near + far;
        var dist = 0.0;
        if found.y > 0.0 {
            // Their mean distance to the receiver along the light, world.
            dist = max(m.z - found.x / found.y, 0.0) * P.depth;
        }
        if P.fade > 0.0 {
            lift = clamp(1.0 - dist / P.fade, 0.0, 1.0);
        }
        let grow = min(dist * P.growth, widest - base);
        if grow > 0.0 {
            let r = base + grow;
            side = clamp(u32(ceil(2.0 * r / gap - 0.001)) + 1u, P.taps, wide);
            spacing = 2.0 * r / f32(side - 1u);
        }
    }
    let k = (f32(side) - 1.0) * 0.5;
    var lit = 0.0;
    for (var j = 0u; j < side; j = j + 1u) {
        for (var i = 0u; i < side; i = i + 1u) {
            let o = (vec2<f32>(f32(i), f32(j)) - vec2<f32>(k, k)) * spacing;
            lit = lit + textureSampleCompareLevel(t_shadow, s_shadow, uv + o, reference);
        }
    }
    let blocked = 1.0 - lit / f32(side * side);
    var fog = 0.0;
    if P.fog_far > P.fog_near {
        fog = clamp((c.z - P.fog_near) / (P.fog_far - P.fog_near), 0.0, 1.0);
    }
    let fade = clamp((P.distance - c.z) / (0.2 * P.distance), 0.0, 1.0)
        * (1.0 - smoothstep(0.9, 1.0, edge))
        * (1.0 - fog)
        * smoothstep(0.0, 0.2, ndl);
    let a = P.color.a * blocked * fade * lift;
    if a <= 0.0 {
        discard;
    }
    return vec4<f32>(P.color.rgb, a);
}
