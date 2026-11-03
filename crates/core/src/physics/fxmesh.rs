//! Triangle-mesh colliders in Q16.16 (issue #115): a [`MeshInstance`]
//! converted once, at load, into world-space fixed-point triangles and BVH
//! bounds in caller buffers, so a `CharacterController<Fx>` can walk on it
//! with integer arithmetic only. Every query is integer: the same bits on
//! every target by construction, as the rest of the fixed-point controller.
//!
//! - **The conversion** ([`FxMesh::build`]) runs the instance's `f32`
//!   transform once per vertex and rounds to Q16.16 (`Fx::from_f32`); f32
//!   adds and products are IEEE on every target, so the converted mesh is
//!   identical everywhere (the `physics` self-test pins a hash of a course
//!   over one on PC, ares and Azahar).
//! - **Normals** are unit Q16.16, normalized with an integer square root.
//! - **Range.** Products are taken in `i64` on the raw bits, so coordinates
//!   and edges stay within ±256 units (the fixed-point controller's own
//!   range is tighter: ~150).

use super::mesh::LEAF;
use super::scalar::{Box3, Fx, MeshQuery, MeshSupport, Scalar, V3};
use super::{MeshInstance, TCOL_MAX_DEPTH};

/// One triangle of an [`FxMesh`]: world corners and unit normal.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FxTri {
    pub v: [V3Fx; 3],
    pub n: V3Fx,
}

/// A Q16.16 point as raw bits (`Fx` per axis), `Copy` and `Default`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct V3Fx {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl V3Fx {
    fn of(v: V3<Fx>) -> Self {
        V3Fx {
            x: v.x.0,
            y: v.y.0,
            z: v.z.0,
        }
    }

    fn fx(self) -> V3<Fx> {
        V3::new(Fx(self.x), Fx(self.y), Fx(self.z))
    }
}

/// A BVH node of an [`FxMesh`]: world bounds and the TCOL's links.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FxNode {
    pub min: V3Fx,
    pub max: V3Fx,
    a: u32,
    b: u32,
}

/// A mesh collider in fixed point (see the module docs).
#[derive(Clone, Copy, Debug)]
pub struct FxMesh<'a> {
    tris: &'a [FxTri],
    nodes: &'a [FxNode],
    /// Layer bits, like a [`Solid`](super::Solid)'s.
    pub layer: u16,
}

/// Floor of the square root of `n`.
fn isqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = 1u64 << (64 - n.leading_zeros()).div_ceil(2);
    loop {
        let y = (x + n / x) >> 1;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// The unit normal of `(b - a) × (c - a)` in Q16.16 (zero for a
/// degenerate triangle), by integers only.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn unit_normal(t: &[V3Fx; 3]) -> V3Fx {
    let e1 = sub3(raw(t[1]), raw(t[0]));
    let e2 = sub3(raw(t[2]), raw(t[0]));
    let c = [
        e1[1]
            .wrapping_mul(e2[2])
            .wrapping_sub(e1[2].wrapping_mul(e2[1])),
        e1[2]
            .wrapping_mul(e2[0])
            .wrapping_sub(e1[0].wrapping_mul(e2[2])),
        e1[0]
            .wrapping_mul(e2[1])
            .wrapping_sub(e1[1].wrapping_mul(e2[0])),
    ];
    let mut m = 0u64;
    for v in c {
        m = m.max(v.unsigned_abs());
    }
    if m == 0 {
        return V3Fx::default();
    }
    // Bring the largest component under 2^30 so the squares fit.
    let k = (64 - m.leading_zeros()).saturating_sub(30);
    let mut sq = 0u64;
    let mut sh = [0i64; 3];
    for i in 0..3 {
        sh[i] = c[i] >> k;
        sq += (sh[i] * sh[i]) as u64;
    }
    let len = isqrt(sq) as i64;
    if len == 0 {
        return V3Fx::default();
    }
    V3Fx {
        x: ((sh[0] << 16) / len) as i32,
        y: ((sh[1] << 16) / len) as i32,
        z: ((sh[2] << 16) / len) as i32,
    }
}

#[inline]
fn raw(p: V3Fx) -> [i64; 3] {
    [p.x as i64, p.y as i64, p.z as i64]
}

#[inline]
fn sub3(a: [i64; 3], b: [i64; 3]) -> [i64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot3(a: [i64; 3], b: [i64; 3]) -> i64 {
    a[0].wrapping_mul(b[0])
        .wrapping_add(a[1].wrapping_mul(b[1]))
        .wrapping_add(a[2].wrapping_mul(b[2]))
}

impl<'a> FxMesh<'a> {
    /// Buffer lengths [`build`](Self::build) needs: (triangles, nodes).
    pub fn size(inst: &MeshInstance) -> (usize, usize) {
        (inst.mesh.triangle_count(), inst.mesh.node_count())
    }

    /// Convert `inst` (its `f32` world transform applied once) into `tris`
    /// and `nodes` (lengths from [`size`](Self::size) or longer). `None`
    /// when a buffer is too small.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn build(
        inst: &MeshInstance,
        tris: &'a mut [FxTri],
        nodes: &'a mut [FxNode],
    ) -> Option<FxMesh<'a>> {
        let (nt, nn) = Self::size(inst);
        if tris.len() < nt || nodes.len() < nn {
            return None;
        }
        for (t, out) in tris[..nt].iter_mut().enumerate() {
            let w = inst.triangle(t);
            let mut v = [V3Fx::default(); 3];
            for k in 0..3 {
                v[k] = V3Fx::of(V3::from_f32(w[k]));
            }
            *out = FxTri {
                v,
                n: unit_normal(&v),
            };
        }
        // Bounds bottom-up: children come after their parent.
        for n in (0..nn).rev() {
            let (_, a, b) = inst.mesh.node(n);
            let (mut lo, mut hi) = (V3Fx::default(), V3Fx::default());
            let mut first = true;
            let mut grow = |p: V3Fx, q: V3Fx| {
                if first {
                    (lo, hi, first) = (p, q, false);
                } else {
                    lo = V3Fx {
                        x: lo.x.min(p.x),
                        y: lo.y.min(p.y),
                        z: lo.z.min(p.z),
                    };
                    hi = V3Fx {
                        x: hi.x.max(q.x),
                        y: hi.y.max(q.y),
                        z: hi.z.max(q.z),
                    };
                }
            };
            if b & LEAF != 0 {
                for t in &tris[a as usize..(a + (b & !LEAF)) as usize] {
                    for p in t.v {
                        grow(p, p);
                    }
                }
            } else {
                let (ca, cb) = (nodes[a as usize], nodes[b as usize]);
                grow(ca.min, ca.max);
                grow(cb.min, cb.max);
            }
            nodes[n] = FxNode {
                min: lo,
                max: hi,
                a,
                b,
            };
        }
        let (tris, nodes): (&'a [FxTri], &'a [FxNode]) = (tris, nodes);
        Some(FxMesh {
            tris: &tris[..nt],
            nodes: &nodes[..nn],
            layer: inst.layer,
        })
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    pub fn triangle(&self, t: usize) -> &FxTri {
        &self.tris[t]
    }

    /// Visit the triangles whose leaves touch `q`; stops when `f` is false.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn for_each_tri(&self, q: &Box3<Fx>, mut f: impl FnMut(usize) -> bool) {
        let mut stack = [0u32; TCOL_MAX_DEPTH + 2];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let n = self.nodes[stack[sp] as usize];
            let touches = n.min.x <= q.max.x.0
                && q.min.x.0 <= n.max.x
                && n.min.y <= q.max.y.0
                && q.min.y.0 <= n.max.y
                && n.min.z <= q.max.z.0
                && q.min.z.0 <= n.max.z;
            if !touches {
                continue;
            }
            if n.b & LEAF != 0 {
                for t in n.a as usize..(n.a + (n.b & !LEAF)) as usize {
                    if !f(t) {
                        return;
                    }
                }
            } else {
                stack[sp] = n.b;
                stack[sp + 1] = n.a;
                sp += 2;
            }
        }
    }
}

/// Does the XZ rectangle overlap the triangle's XZ shadow (SAT, integers)?
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn rect_tri_xz(min: V3Fx, max: V3Fx, t: &FxTri) -> bool {
    let v = &t.v;
    let (tx0, tx1) = (
        v[0].x.min(v[1].x).min(v[2].x),
        v[0].x.max(v[1].x).max(v[2].x),
    );
    let (tz0, tz1) = (
        v[0].z.min(v[1].z).min(v[2].z),
        v[0].z.max(v[1].z).max(v[2].z),
    );
    if tx1 <= min.x || tx0 >= max.x || tz1 <= min.z || tz0 >= max.z {
        return false;
    }
    let corners = [
        (min.x, min.z),
        (max.x, min.z),
        (max.x, max.z),
        (min.x, max.z),
    ];
    for i in 0..3 {
        let (a, b, c) = (v[i], v[(i + 1) % 3], v[(i + 2) % 3]);
        let third = side_xz(a, b, c.x, c.z);
        if third == 0 {
            continue;
        }
        let mut apart = true;
        for &(x, z) in &corners {
            let s = side_xz(a, b, x, z);
            apart &= s == 0 || (s < 0) != (third < 0);
        }
        if apart {
            return false;
        }
    }
    true
}

/// The side of `(x, z)` against the XZ edge `a → b` (raw products).
fn side_xz(a: V3Fx, b: V3Fx, x: i32, z: i32) -> i64 {
    let (nx, nz) = (b.z as i64 - a.z as i64, a.x as i64 - b.x as i64);
    nx.wrapping_mul(x as i64 - a.x as i64)
        .wrapping_add(nz.wrapping_mul(z as i64 - a.z as i64))
}

/// Do the triangle `v` (relative to the box center) and the box of half
/// extents `h` overlap along `ax`?
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn sat_hit(v: &[[i64; 3]; 3], h: [i64; 3], ax: [i64; 3]) -> bool {
    let p = [dot3(v[0], ax), dot3(v[1], ax), dot3(v[2], ax)];
    let r = dot3(h, [ax[0].abs(), ax[1].abs(), ax[2].abs()]);
    let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
    !(lo >= r || hi <= -r)
}

/// Triangle / box overlap by separating axes on raw Q16.16 bits (`i64`
/// products): the f32 [`MeshInstance`]'s test in integers.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn tri_box(c: [i64; 3], h: [i64; 3], t: &FxTri) -> bool {
    let v = [
        sub3(raw(t.v[0]), c),
        sub3(raw(t.v[1]), c),
        sub3(raw(t.v[2]), c),
    ];
    for k in 0..3 {
        let ed = sub3(v[(k + 1) % 3], v[k]);
        let axes = [[0, -ed[2], ed[1]], [ed[2], 0, -ed[0]], [-ed[1], ed[0], 0]];
        for ax in axes {
            if ax != [0, 0, 0] && !sat_hit(&v, h, ax) {
                return false;
            }
        }
    }
    for u in [[1, 0, 0], [0, 1, 0], [0, 0, 1]] {
        if !sat_hit(&v, h, u) {
            return false;
        }
    }
    let n = raw(t.n);
    n == [0, 0, 0] || sat_hit(&v, h, n)
}

impl MeshQuery<Fx> for FxMesh<'_> {
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn support(&self, foot: &Box3<Fx>, up_to: Fx, min_normal_y: f32) -> Option<MeshSupport<Fx>> {
        let mny = Fx::from_f32(min_normal_y).0;
        let column = Box3::new(
            V3::new(foot.min.x, Fx(-(1 << 30)), foot.min.z),
            V3::new(foot.max.x, up_to, foot.max.z),
        );
        let (fmin, fmax) = (V3Fx::of(foot.min), V3Fx::of(foot.max));
        let mut best: Option<MeshSupport<Fx>> = None;
        self.for_each_tri(&column, |ti| {
            let t = &self.tris[ti];
            if t.n.y < mny || t.n.y <= 0 || !rect_tri_xz(fmin, fmax, t) {
                return true;
            }
            let n = t.n.fx();
            let p = t.v[0].fx();
            let x = Fx(if t.n.x < 0 { fmax.x } else { fmin.x });
            let z = Fx(if t.n.z < 0 { fmax.z } else { fmin.z });
            let y = p.y - (n.x * (x - p.x) + n.z * (z - p.z)) / n.y;
            let top = Fx(t.v[0].y.max(t.v[1].y).max(t.v[2].y));
            let low = Fx(t.v[0].y.min(t.v[1].y).min(t.v[2].y));
            let y = Scalar::clamp(y, low, top);
            if y <= up_to
                && best.is_none_or(|b| y > b.height || (y == b.height && (ti as u32) < b.tri))
            {
                best = Some(MeshSupport {
                    height: y,
                    normal: n,
                    tri: ti as u32,
                });
            }
            true
        });
        best
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn blocks(&self, body: &Box3<Fx>, min_normal_y: f32) -> bool {
        let mny = Fx::from_f32(min_normal_y).0;
        let (lo, hi) = (V3Fx::of(body.min), V3Fx::of(body.max));
        let c = [
            (lo.x as i64 + hi.x as i64) >> 1,
            (lo.y as i64 + hi.y as i64) >> 1,
            (lo.z as i64 + hi.z as i64) >> 1,
        ];
        let h = [
            (hi.x as i64 - lo.x as i64) >> 1,
            (hi.y as i64 - lo.y as i64) >> 1,
            (hi.z as i64 - lo.z as i64) >> 1,
        ];
        let mut hit = false;
        self.for_each_tri(body, |ti| {
            let t = &self.tris[ti];
            if t.n.y < mny && tri_box(c, h, t) {
                hit = true;
                return false;
            }
            true
        });
        hit
    }
}
