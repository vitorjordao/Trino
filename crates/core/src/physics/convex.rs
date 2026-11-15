//! Exact queries against small convex polytopes (issue #115): a triangle, a
//! triangle extruded along Y (what a Y-capsule sees of it), a ramp's wedge
//! and the wedge extruded along Y. Shape casts and overlaps against triangle
//! meshes and ramps go through here.
//!
//! - **A sphere swept against a convex polytope** is a ray against the
//!   polytope grown by the radius. That boundary is made of the faces moved
//!   out by `r` (both sides, so a flat triangle works), a cylinder round each
//!   edge and a sphere round each vertex. Every one of those pieces lies on
//!   or inside the grown solid, and together they cover its boundary, so the
//!   earliest hit among them is the exact first contact.
//! - **A Y-capsule** is a sphere swept along its vertical segment, so against
//!   a polytope `P` it is a sphere against `P` extruded by the half segment
//!   up and down (the Minkowski sum with the segment).
//! - **A box** is the swept separating-axis test: the box's three axes, the
//!   polytope's face normals and the cross products of the box's axes with
//!   the polytope's edges. The contact is the latest entry over those axes.
//!
//! Degenerate pieces (a zero-area face, a zero-length edge) are skipped: what
//! they would cover is covered by the edges and vertices around them.

use super::controller::RampDir;
use super::{Aabb, SKIN};
use crate::math::Vec3;
use crate::math3d::sqrt;

const MAX_V: usize = 8;
const MAX_F: usize = 6;
const MAX_E: usize = 12;

/// A convex polytope given by its vertices, faces (convex polygons of up to
/// four vertices, in order around the face) and edges. The constructors
/// compute the face normals, the scale and whether it has a volume once.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Convex {
    v: [Vec3; MAX_V],
    nv: u8,
    faces: [[u8; 4]; MAX_F],
    face_len: [u8; MAX_F],
    nf: u8,
    edges: [[u8; 2]; MAX_E],
    ne: u8,
    /// Newell's unit normal per face (in its winding's sense); bit `f` of
    /// `has_n` is clear for a face without area.
    n: [Vec3; MAX_F],
    has_n: u8,
    /// The centroid, when the polytope has a volume.
    centroid: Option<Vec3>,
}

/// Most separating axes against a box: 3 + faces + 3 per edge.
const MAX_AXES: usize = 3 + MAX_F + 3 * MAX_E;

impl Convex {
    const EMPTY: Convex = Convex {
        v: [Vec3::ZERO; MAX_V],
        nv: 0,
        faces: [[0; 4]; MAX_F],
        face_len: [0; MAX_F],
        nf: 0,
        edges: [[0; 2]; MAX_E],
        ne: 0,
        n: [Vec3::ZERO; MAX_F],
        has_n: 0,
        centroid: None,
    };

    fn face(&mut self, f: &[u8]) {
        let i = self.nf as usize;
        self.faces[i][..f.len()].copy_from_slice(f);
        self.face_len[i] = f.len() as u8;
        self.nf += 1;
    }

    fn edges_from(&mut self, list: &[[u8; 2]]) {
        self.edges[..list.len()].copy_from_slice(list);
        self.ne = list.len() as u8;
    }

    /// The normals, the scale and the volume test, once.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn finish(mut self) -> Convex {
        let mut scale = 1e-6f32;
        for i in 1..self.nv as usize {
            let d = self.v[i] - self.v[0];
            scale = scale.max(d.dot(d));
        }
        for f in 0..self.nf as usize {
            let fv = &self.faces[f][..self.face_len[f] as usize];
            let mut n = Vec3::ZERO;
            for i in 0..fv.len() {
                let a = self.v[fv[i] as usize];
                let b = self.v[fv[(i + 1) % fv.len()] as usize];
                n.x += (a.y - b.y) * (a.z + b.z);
                n.y += (a.z - b.z) * (a.x + b.x);
                n.z += (a.x - b.x) * (a.y + b.y);
            }
            let l2 = n.dot(n);
            if l2 > 1e-12 * scale * scale {
                self.n[f] = n * (1.0 / sqrt(l2));
                self.has_n |= 1 << f;
            }
        }
        let mut c = Vec3::ZERO;
        for &v in &self.v[..self.nv as usize] {
            c += v;
        }
        let c = c * (1.0 / self.nv.max(1) as f32);
        let tol = 1e-5 * sqrt(scale);
        'faces: for f in 0..self.nf as usize {
            if let Some(n) = self.normal(f) {
                let v0 = self.v[self.faces[f][0] as usize];
                for i in 0..self.nv as usize {
                    if (self.v[i] - v0).dot(n).abs() > tol {
                        self.centroid = Some(c);
                        break 'faces;
                    }
                }
            }
        }
        self
    }

    /// A triangle, extruded by `seg` up and down when `seg > 0` (a prism).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub(crate) fn triangle(t: &[Vec3; 3], seg: f32) -> Convex {
        let mut c = Convex::EMPTY;
        if seg <= 0.0 {
            c.v[..3].copy_from_slice(t);
            c.nv = 3;
            c.face(&[0, 1, 2]);
            c.edges_from(&[[0, 1], [1, 2], [2, 0]]);
            return c.finish();
        }
        let s = Vec3::new(0.0, seg, 0.0);
        for (i, p) in t.iter().enumerate() {
            c.v[i] = *p - s;
            c.v[i + 3] = *p + s;
        }
        c.nv = 6;
        c.face(&[0, 1, 2]);
        c.face(&[3, 4, 5]);
        c.face(&[0, 1, 4, 3]);
        c.face(&[1, 2, 5, 4]);
        c.face(&[2, 0, 3, 5]);
        c.edges_from(&[
            [0, 1],
            [1, 2],
            [2, 0],
            [3, 4],
            [4, 5],
            [5, 3],
            [0, 3],
            [1, 4],
            [2, 5],
        ]);
        c.finish()
    }

    /// A ramp's wedge (the box `b` under the slope that rises toward `dir`),
    /// extruded by `seg` up and down: eight vertices, the low wall `2·seg`
    /// tall (none for the bare wedge).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub(crate) fn wedge(b: &Aabb, dir: RampDir, seg: f32) -> Convex {
        let along_x = matches!(dir, RampDir::PosX | RampDir::NegX);
        let (lo_side, hi_side) = match dir {
            RampDir::PosX => (b.min.x, b.max.x),
            RampDir::NegX => (b.max.x, b.min.x),
            RampDir::PosZ => (b.min.z, b.max.z),
            RampDir::NegZ => (b.max.z, b.min.z),
        };
        let (o0, o1) = if along_x {
            (b.min.z, b.max.z)
        } else {
            (b.min.x, b.max.x)
        };
        let seg = seg.max(0.0);
        let mut c = Convex::EMPTY;
        // Vertex index: side * 4 + other * 2 + top.
        for (k, v) in c.v.iter_mut().enumerate() {
            let (side, other, top) = (k >> 2, (k >> 1) & 1, k & 1);
            let a = if side == 0 { lo_side } else { hi_side };
            let o = if other == 0 { o0 } else { o1 };
            let y = match (side, top) {
                (_, 0) => b.min.y - seg,
                (0, _) => b.min.y + seg,
                _ => b.max.y + seg,
            };
            *v = if along_x {
                Vec3::new(a, y, o)
            } else {
                Vec3::new(o, y, a)
            };
        }
        c.nv = 8;
        // Bottom, slope, low wall, high wall, the two sides.
        c.face(&[0, 2, 6, 4]);
        c.face(&[1, 5, 7, 3]);
        c.face(&[0, 1, 3, 2]);
        c.face(&[4, 6, 7, 5]);
        c.face(&[0, 4, 5, 1]);
        c.face(&[2, 3, 7, 6]);
        c.edges_from(&[
            [0, 4],
            [2, 6],
            [1, 5],
            [3, 7],
            [0, 2],
            [4, 6],
            [1, 3],
            [5, 7],
            [0, 1],
            [2, 3],
            [4, 5],
            [6, 7],
        ]);
        c.finish()
    }

    #[inline]
    fn vert(&self, i: u8) -> Vec3 {
        self.v[i as usize]
    }

    #[inline]
    fn normal(&self, f: usize) -> Option<Vec3> {
        (self.has_n & (1 << f) != 0).then_some(self.n[f])
    }

    /// Is `q` (on face `f`'s plane) inside the face polygon? `n` is the
    /// face's Newell normal.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn in_face(&self, f: usize, n: Vec3, q: Vec3) -> bool {
        let fv = &self.faces[f][..self.face_len[f] as usize];
        for i in 0..fv.len() {
            let a = self.vert(fv[i]);
            let b = self.vert(fv[(i + 1) % fv.len()]);
            if (b - a).cross(q - a).dot(n) < 0.0 {
                return false;
            }
        }
        true
    }

    /// The closest point of the polytope to `p` and its squared distance
    /// (zero inside).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub(crate) fn closest(&self, p: Vec3) -> (Vec3, f32) {
        if let Some(c) = self.centroid {
            let mut inside = true;
            for f in 0..self.nf as usize {
                if let Some(n) = self.normal(f) {
                    let v0 = self.vert(self.faces[f][0]);
                    let out = if n.dot(c - v0) > 0.0 { -n } else { n };
                    if out.dot(p - v0) > 0.0 {
                        inside = false;
                        break;
                    }
                }
            }
            if inside {
                return (p, 0.0);
            }
        }
        let mut best = (self.v[0], f32::INFINITY);
        // Faces, then edges, then vertices: the nearest candidate.
        for k in 0..(self.nf + self.ne + self.nv) as usize {
            let q = if k < self.nf as usize {
                let Some(n) = self.normal(k) else {
                    continue;
                };
                let v0 = self.vert(self.faces[k][0]);
                let q = p - n * n.dot(p - v0);
                if !self.in_face(k, n, q) {
                    continue;
                }
                q
            } else if k < (self.nf + self.ne) as usize {
                let e = self.edges[k - self.nf as usize];
                let (a, b) = (self.vert(e[0]), self.vert(e[1]));
                let ab = b - a;
                let l2 = ab.dot(ab);
                let s = if l2 > 0.0 {
                    crate::math::clamp_or((p - a).dot(ab) / l2, 0.0, 1.0)
                } else {
                    0.0
                };
                a + ab * s
            } else {
                self.v[k - (self.nf + self.ne) as usize]
            };
            let d = p - q;
            let d2 = d.dot(d);
            if d2 < best.1 {
                best = (q, d2);
            }
        }
        best
    }

    /// A sphere of radius `r` centered `c`, moving along `dir` (unit) up to
    /// `limit`, that does not start inside: its first contact (distance and
    /// normal out of the polytope).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub(crate) fn sphere_sweep(
        &self,
        c: Vec3,
        r: f32,
        dir: Vec3,
        limit: f32,
    ) -> Option<(f32, Vec3)> {
        let mut best = limit;
        let mut found: Option<Vec3> = None;
        // Faces moved out by r (both sides), edge cylinders, vertex spheres.
        for k in 0..(2 * self.nf + self.ne + self.nv) as usize {
            let (t, n) = if k < 2 * self.nf as usize {
                let f = k >> 1;
                let Some(nf) = self.normal(f) else {
                    continue;
                };
                let v0 = self.vert(self.faces[f][0]);
                let n = if k & 1 == 0 { nf } else { -nf };
                let denom = n.dot(dir);
                if denom >= -1e-12 {
                    continue;
                }
                let t = (n.dot(v0) + r - n.dot(c)) / denom;
                if !self.in_face(f, nf, c + dir * t - n * r) {
                    continue;
                }
                (t, n)
            } else if k < (2 * self.nf + self.ne) as usize {
                let e = self.edges[k - 2 * self.nf as usize];
                let a = self.vert(e[0]);
                let ed = self.vert(e[1]) - a;
                let ee = ed.dot(ed);
                if ee < 1e-12 {
                    continue;
                }
                let m = c - a;
                let (md, nd) = (m.dot(ed), dir.dot(ed));
                let qa = dir.dot(dir) - nd * nd / ee;
                if qa < 1e-12 {
                    continue;
                }
                let qb = m.dot(dir) - md * nd / ee;
                let qc = m.dot(m) - md * md / ee - r * r;
                let disc = qb * qb - qa * qc;
                if disc < 0.0 {
                    continue;
                }
                let t = (-qb - sqrt(disc)) / qa;
                let s = (md + nd * t) / ee;
                if !(0.0..=1.0).contains(&s) {
                    continue;
                }
                (t, unit_or_zero(m + dir * t - ed * s))
            } else {
                let oc = c - self.v[k - (2 * self.nf + self.ne) as usize];
                let hb = oc.dot(dir);
                let disc = hb * hb - (oc.dot(oc) - r * r);
                if disc < 0.0 {
                    continue;
                }
                let t = -hb - sqrt(disc);
                (t, unit_or_zero(oc + dir * t))
            };
            if (0.0..=best).contains(&t) && (found.is_none() || t < best) {
                best = t;
                found = Some(n);
            }
        }
        found.map(|n| (best, n))
    }

    /// The separating-axis candidates against a box into `out`: the box's
    /// axes, the face normals, the box axes crossed with the edges.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn axes(&self, out: &mut [Vec3; MAX_AXES]) -> usize {
        let units = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ];
        out[..3].copy_from_slice(&units);
        let mut n = 3;
        for f in 0..self.nf as usize {
            if let Some(v) = self.normal(f) {
                out[n] = v;
                n += 1;
            }
        }
        for e in 0..self.ne as usize {
            let ed = self.vert(self.edges[e][1]) - self.vert(self.edges[e][0]);
            let ee = ed.dot(ed);
            if ee < 1e-12 {
                continue;
            }
            for u in units {
                let ax = u.cross(ed);
                let l2 = ax.dot(ax);
                if l2 > 1e-10 * ee {
                    out[n] = ax * (1.0 / sqrt(l2));
                    n += 1;
                }
            }
        }
        n
    }

    /// A box (center `c`, half `h`) against the polytope: `Err(())` when it
    /// overlaps at the start (touching does not), else its first contact
    /// moving along `dir` (unit) up to `limit` and the contact normal.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub(crate) fn box_cast(
        &self,
        c: Vec3,
        h: Vec3,
        dir: Vec3,
        limit: f32,
    ) -> Result<Option<(f32, Vec3)>, ()> {
        let mut axes = [Vec3::ZERO; MAX_AXES];
        let na = self.axes(&mut axes);
        let (mut t_in, mut t_out, mut normal) = (f32::NEG_INFINITY, f32::INFINITY, Vec3::ZERO);
        let (mut separated, mut ok) = (false, true);
        for ax in &axes[..na] {
            let ax = *ax;
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for &v in &self.v[..self.nv as usize] {
                let p = v.dot(ax);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            let (cp, r) = (
                c.dot(ax),
                h.x * ax.x.abs() + h.y * ax.y.abs() + h.z * ax.z.abs(),
            );
            separated |= !(cp + r > lo && cp - r < hi);
            if !ok {
                continue;
            }
            let s = dir.dot(ax);
            if s.abs() < 1e-9 {
                ok = cp + r > lo && cp - r < hi;
                continue;
            }
            let (enter, exit, n) = if s > 0.0 {
                ((lo - (cp + r)) / s, (hi - (cp - r)) / s, -ax)
            } else {
                ((hi - (cp - r)) / s, (lo - (cp + r)) / s, ax)
            };
            if enter > t_in {
                t_in = enter;
                normal = n;
            }
            t_out = t_out.min(exit);
            ok = t_in <= t_out;
        }
        if !separated {
            return Err(());
        }
        Ok((ok && t_in <= limit && t_out >= 0.0).then(|| (t_in.max(0.0), normal)))
    }

    /// Does the box (center `c`, half extents `h`) overlap the polytope?
    /// Touching does not.
    pub(crate) fn overlaps_box(&self, c: Vec3, h: Vec3) -> bool {
        self.box_cast(c, h, Vec3::ZERO, 0.0).is_err()
    }
}

#[inline]
fn unit_or_zero(v: Vec3) -> Vec3 {
    let l = sqrt(v.dot(v));
    if l < 1e-9 { Vec3::ZERO } else { v * (1.0 / l) }
}

/// The lowest point of a cast shape centered at `c` (one-way platforms).
pub(crate) fn shape_bottom(shape: &super::CastShape, c: Vec3) -> f32 {
    shape.bounds(c).min.y
}

/// A one-way platform takes part in a cast or overlap only for a shape
/// whose bottom starts at or above its top (within the contact skin): it is
/// solid from above, like the controller's.
pub(crate) fn one_way_takes(shape: &super::CastShape, from: Vec3, top: f32) -> bool {
    shape_bottom(shape, from) >= top - SKIN
}
