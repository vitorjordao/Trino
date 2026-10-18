// Screen-space ambient occlusion (the PC native tier, ADR 0014).
//
// Four fullscreen passes over the frame's depth buffer, after the 3D and
// before the 2D drawn on top of it:
//   fs_ao       depth -> raw occlusion (R8): view-space position from the
//               depth `1 - NEAR/z`, the normal rebuilt from the nearer
//               neighbor on each axis, a hemisphere of samples around it;
//   fs_blur_h,  a separable 5-tap depth-aware (bilateral) blur, whose
//   fs_blur_v   [0.5, 1, 1, 1, 0.5] kernel spans exactly the 4x4 sample
//               rotation pattern, so flat areas come out smooth;
//   fs_apply    multiplies the frame by it (blend Dst * Src), or shows
//               it alone (no blend: the mask debug view).
// Tall pictures (`scale` 2) compute the occlusion and blur it at half
// resolution, each texel at the top-left pixel of its 2x2 block, and
// fs_apply upsamples it, weighting the four nearest texels by how close
// their depth is to the pixel's (edges stay sharp).
// Every input is fixed per pixel (no per-frame noise): the same frame
// renders the same pixels.

struct AoParams {
    size: vec2<f32>,
    center: vec2<f32>,
    focal: f32,
    near: f32,
    radius: f32,
    intensity: f32,
    bias: f32,
    samples: u32,
    fog_near: f32,
    // <= fog_near: no fog fade.
    fog_far: f32,
    // Frame pixels per occlusion texel, 1 or 2.
    scale: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};

@group(0) @binding(0) var<uniform> P: AoParams;
@group(0) @binding(1) var t_depth: texture_depth_2d;
@group(0) @binding(2) var t_ao: texture_2d<f32>;

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

// View-space position (x right, y up, z forward) of pixel `p` at depth `d`:
// the T&L's projection run backwards.
fn view_pos(p: vec2<i32>, d: f32) -> vec3<f32> {
    let z = view_z(d);
    let s = vec2<f32>(p) + vec2<f32>(0.5, 0.5);
    return vec3<f32>((s.x - P.center.x) * z / P.focal, -(s.y - P.center.y) * z / P.focal, z);
}

fn pos_at(p: vec2<i32>) -> vec3<f32> {
    let q = clamp(p, vec2<i32>(0, 0), hi());
    return view_pos(q, depth_at(q));
}

// 4x4 ordered (Bayer) pattern, 0..15: each pixel of a 4x4 block turns
// the sample kernel by a different sixteenth of a turn. Bit arithmetic,
// not a table: a dynamically indexed local array lives in scratch memory,
// per pixel (AGENTS.md › WGSL pitfall).
fn bayer2(x: u32, y: u32) -> u32 {
    return ((x ^ y) << 1u) | y;
}

fn bayer4(p: vec2<i32>) -> f32 {
    let x = u32(p.x);
    let y = u32(p.y);
    return f32(bayer2(x & 1u, y & 1u) * 4u + bayer2((x >> 1u) & 1u, (y >> 1u) & 1u));
}

@fragment
fn fs_ao(in: FullOut) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(in.clip.xy);
    let p = texel * i32(P.scale);
    let d = depth_at(p);
    if d >= 1.0 {
        // Background (cleared): nothing to occlude.
        return vec4<f32>(1.0, 0.0, 0.0, 1.0);
    }
    let c = view_pos(p, d);
    // The normal from the neighbor nearer in depth on each axis, so a
    // silhouette doesn't bend the normals of the surface behind it.
    let l = pos_at(p - vec2<i32>(1, 0));
    let r = pos_at(p + vec2<i32>(1, 0));
    let u = pos_at(p - vec2<i32>(0, 1));
    let b = pos_at(p + vec2<i32>(0, 1));
    var dx = r - c;
    if abs(c.z - l.z) < abs(r.z - c.z) {
        dx = c - l;
    }
    var dy = b - c;
    if abs(c.z - u.z) < abs(b.z - c.z) {
        dy = c - u;
    }
    var n = cross(dx, dy);
    if dot(n, n) < 1e-20 {
        return vec4<f32>(1.0, 0.0, 0.0, 1.0);
    }
    n = normalize(n);
    // Face the eye (at the origin).
    if dot(n, c) > 0.0 {
        n = -n;
    }
    let rot = (bayer4(texel) + 0.5) / 16.0 * 6.2831853;
    let rv = vec3<f32>(cos(rot), sin(rot), 0.0);
    var t = rv - n * dot(rv, n);
    if dot(t, t) < 1e-6 {
        t = cross(n, vec3<f32>(0.0, 1.0, 0.0));
    }
    t = normalize(t);
    let bt = cross(n, t);
    let count = max(P.samples, 1u);
    var occ = 0.0;
    for (var i = 0u; i < count; i++) {
        let fi = f32(i);
        // Cosine-weighted directions on a golden-angle spiral, lengths
        // spread toward the point (near occluders matter most).
        let h = (fi + 0.5) / f32(count);
        let phi = fi * 2.3999632;
        let sin_t = sqrt(h);
        let cos_t = sqrt(1.0 - h);
        let k = fract(fi * 0.618034 + 0.31);
        let len = P.radius * mix(0.15, 1.0, k * k);
        let dir = t * (cos(phi) * sin_t) + bt * (sin(phi) * sin_t) + n * cos_t;
        let s = c + dir * len;
        if s.z <= P.near {
            continue;
        }
        let q = P.center + vec2<f32>(s.x, -s.y) * (P.focal / s.z);
        let qi = vec2<i32>(floor(q));
        if qi.x < 0 || qi.y < 0 || qi.x > hi().x || qi.y > hi().y {
            continue;
        }
        let sd = load_depth(qi);
        if sd >= 1.0 {
            continue;
        }
        let sz = view_z(sd);
        if sz <= s.z - P.bias {
            // Occluders far in front of the point (another object across
            // the screen) count less and less.
            occ += smoothstep(0.0, 1.0, P.radius / abs(c.z - sz));
        }
    }
    var ao = clamp(1.0 - P.intensity * occ / f32(count), 0.0, 1.0);
    if P.fog_far > P.fog_near {
        let f = clamp((c.z - P.fog_near) / (P.fog_far - P.fog_near), 0.0, 1.0);
        ao = mix(ao, 1.0, f);
    }
    return vec4<f32>(ao, 0.0, 0.0, 1.0);
}

// Depth of occlusion texel `q` (the top-left pixel of its block).
fn texel_depth(q: vec2<i32>) -> f32 {
    return depth_at(q * i32(P.scale));
}

fn blur(p: vec2<i32>, step: vec2<i32>) -> f32 {
    let top = vec2<i32>(textureDimensions(t_ao)) - vec2<i32>(1, 1);
    let d = texel_depth(p);
    let center = textureLoad(t_ao, p, 0).r;
    if d >= 1.0 {
        return center;
    }
    let zc = view_z(d);
    // Depth tolerance: 8% of the view depth.
    let tol = zc * 0.08 + 1e-4;
    var sum = center;
    var weight = 1.0;
    for (var i = -2; i <= 2; i++) {
        if i == 0 {
            continue;
        }
        let q = p + step * i;
        if q.x < 0 || q.y < 0 || q.x > top.x || q.y > top.y {
            continue;
        }
        let qd = texel_depth(q);
        if qd >= 1.0 {
            continue;
        }
        var w = 1.0;
        if abs(i) == 2 {
            w = 0.5;
        }
        w *= clamp(1.0 - abs(view_z(qd) - zc) / tol, 0.0, 1.0);
        sum += textureLoad(t_ao, q, 0).r * w;
        weight += w;
    }
    return sum / weight;
}

@fragment
fn fs_blur_h(in: FullOut) -> @location(0) vec4<f32> {
    return vec4<f32>(blur(vec2<i32>(in.clip.xy), vec2<i32>(1, 0)), 0.0, 0.0, 1.0);
}

@fragment
fn fs_blur_v(in: FullOut) -> @location(0) vec4<f32> {
    return vec4<f32>(blur(vec2<i32>(in.clip.xy), vec2<i32>(0, 1)), 0.0, 0.0, 1.0);
}

// The occlusion at frame pixel `p`: its texel, or at half resolution the
// four nearest texels, bilinear weights times depth similarity.
fn occlusion_at(p: vec2<i32>) -> f32 {
    if P.scale == 1u {
        return textureLoad(t_ao, p, 0).r;
    }
    let d = depth_at(p);
    if d >= 1.0 {
        return 1.0;
    }
    let zc = view_z(d);
    let tol = zc * 0.08 + 1e-4;
    let top = vec2<i32>(textureDimensions(t_ao)) - vec2<i32>(1, 1);
    let f = vec2<f32>(p) / f32(P.scale);
    let base = vec2<i32>(floor(f));
    let fr = f - floor(f);
    var sum = 0.0;
    var weight = 0.0;
    for (var dy = 0; dy <= 1; dy++) {
        for (var dx = 0; dx <= 1; dx++) {
            let q = clamp(base + vec2<i32>(dx, dy), vec2<i32>(0, 0), top);
            let bx = select(1.0 - fr.x, fr.x, dx == 1);
            let by = select(1.0 - fr.y, fr.y, dy == 1);
            let qd = texel_depth(q);
            var near = 0.0;
            if qd < 1.0 {
                near = clamp(1.0 - abs(view_z(qd) - zc) / tol, 0.0, 1.0);
            }
            let w = bx * by * max(near, 1e-3);
            sum += textureLoad(t_ao, q, 0).r * w;
            weight += w;
        }
    }
    return sum / weight;
}

@fragment
fn fs_apply(in: FullOut) -> @location(0) vec4<f32> {
    let ao = occlusion_at(vec2<i32>(in.clip.xy));
    return vec4<f32>(ao, ao, ao, 1.0);
}
