//! Triangle-mesh colliders for static level geometry (issue #66 phase 3):
//! the `TCOL` blob (vertices, triangles and a BVH), written by
//! [`write_tcol`] into a caller buffer (the asset bake, the editor and the
//! tests share it) and read in place by [`MeshCollider`]. A
//! [`MeshInstance`] places one in the world under a matrix; the
//! [`CharacterController`](super::CharacterController) stands on its
//! walkable triangles and is stopped by its steep ones, and rays hit it.
//!
//! ```text
//! magic "TCOL"  u16 version (1)  u16 reserved
//! u32 vertex_count  u32 triangle_count  u32 node_count       (20 bytes)
//! vertices: f32*3                                            (12 bytes each)
//! triangles: u16*3 vertex indices, u16 reserved             (8 bytes each)
//! nodes: f32*3 min, f32*3 max, u32 a, u32 b                  (32 bytes each)
//!   leaf when b's top bit is set: triangles a .. a + (b & 0x7FFF_FFFF);
//!   otherwise children a and b, both after the node (node 0 is the root)
//! ```
//!
//! [`MeshCollider::parse`] validates every index, range and child order up
//! front, so queries never fail and the tree is acyclic.

use super::scalar::{Box3, MeshQuery, MeshSupport, V3};
use super::{Aabb, ray_box};
use crate::math::Vec3;
use crate::math3d::{Mat34, sqrt};

pub const TCOL_MAGIC: &[u8; 4] = b"TCOL";
pub const TCOL_VERSION: u16 = 1;
pub const TCOL_HEADER: usize = 20;
const NODE_BYTES: usize = 32;
const LEAF: u32 = 0x8000_0000;
/// Triangles per BVH leaf.
pub const TCOL_LEAF_TRIS: usize = 4;
/// Deepest tree a query walks (the writer's median splits stay far below).
pub const TCOL_MAX_DEPTH: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TcolError {
    Magic,
    Version(u16),
    Truncated,
    BadIndex,
    BadNode,
    TooDeep,
    /// The writer's output buffer is too small (needed bytes).
    Buffer(usize),
    /// More than 65 535 vertices.
    TooManyVertices,
}

/// Bytes [`write_tcol`] needs for `verts` vertices and `tris` triangles.
pub const fn tcol_bytes(verts: usize, tris: usize) -> usize {
    TCOL_HEADER + verts * 12 + tris * 8 + tcol_max_nodes(tris) * NODE_BYTES
}

const fn tcol_max_nodes(tris: usize) -> usize {
    if tris == 0 { 1 } else { 2 * tris }
}

#[inline]
fn wr_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

#[inline]
fn wr_f32(b: &mut [u8], at: usize, v: f32) {
    wr_u32(b, at, v.to_bits());
}

#[inline]
fn rd_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[inline]
fn rd_f32(b: &[u8], at: usize) -> f32 {
    f32::from_bits(rd_u32(b, at))
}

#[inline]
fn rd_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn tri_bounds(verts: &[Vec3], t: [u16; 3]) -> Aabb {
    let a = verts[t[0] as usize];
    Aabb::new(a, a)
        .union(&Aabb::new(verts[t[1] as usize], verts[t[1] as usize]))
        .union(&Aabb::new(verts[t[2] as usize], verts[t[2] as usize]))
}

/// Write a `TCOL` blob for `verts` / `tris` into `out` (length from
/// [`tcol_bytes`]), with `order` as scratch (one `u32` per triangle).
/// Returns the bytes written. Deterministic: the same input gives the same
/// bytes on every machine.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn write_tcol(
    verts: &[Vec3],
    tris: &[[u16; 3]],
    order: &mut [u32],
    out: &mut [u8],
) -> Result<usize, TcolError> {
    if verts.len() > u16::MAX as usize {
        return Err(TcolError::TooManyVertices);
    }
    if tris.iter().flatten().any(|&i| i as usize >= verts.len()) {
        return Err(TcolError::BadIndex);
    }
    let need = tcol_bytes(verts.len(), tris.len());
    if out.len() < need || order.len() < tris.len() {
        return Err(TcolError::Buffer(need));
    }
    let order = &mut order[..tris.len()];
    for (i, o) in order.iter_mut().enumerate() {
        *o = i as u32;
    }
    let nodes_at = TCOL_HEADER + verts.len() * 12 + tris.len() * 8;
    let mut nodes = 0usize;
    build_node(verts, tris, order, 0, out, nodes_at, &mut nodes, 0);
    out[..4].copy_from_slice(TCOL_MAGIC);
    out[4..6].copy_from_slice(&TCOL_VERSION.to_le_bytes());
    out[6..8].fill(0);
    wr_u32(out, 8, verts.len() as u32);
    wr_u32(out, 12, tris.len() as u32);
    wr_u32(out, 16, nodes as u32);
    for (i, v) in verts.iter().enumerate() {
        let at = TCOL_HEADER + i * 12;
        wr_f32(out, at, v.x);
        wr_f32(out, at + 4, v.y);
        wr_f32(out, at + 8, v.z);
    }
    let tris_at = TCOL_HEADER + verts.len() * 12;
    for (k, &ti) in order.iter().enumerate() {
        let t = tris[ti as usize];
        let at = tris_at + k * 8;
        out[at..at + 2].copy_from_slice(&t[0].to_le_bytes());
        out[at + 2..at + 4].copy_from_slice(&t[1].to_le_bytes());
        out[at + 4..at + 6].copy_from_slice(&t[2].to_le_bytes());
        out[at + 6..at + 8].fill(0);
    }
    // The node table ends the blob (fewer nodes than the bound is fine).
    Ok(nodes_at + nodes * NODE_BYTES)
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn build_node(
    verts: &[Vec3],
    tris: &[[u16; 3]],
    order: &mut [u32],
    first: usize,
    out: &mut [u8],
    nodes_at: usize,
    count: &mut usize,
    depth: usize,
) -> usize {
    let me = *count;
    *count += 1;
    let at = nodes_at + me * NODE_BYTES;
    let mut bounds = if order.is_empty() {
        Aabb::EMPTY
    } else {
        tri_bounds(verts, tris[order[0] as usize])
    };
    for &t in order.iter() {
        bounds = bounds.union(&tri_bounds(verts, tris[t as usize]));
    }
    let put = |out: &mut [u8], a: u32, b: u32| {
        wr_f32(out, at, bounds.min.x);
        wr_f32(out, at + 4, bounds.min.y);
        wr_f32(out, at + 8, bounds.min.z);
        wr_f32(out, at + 12, bounds.max.x);
        wr_f32(out, at + 16, bounds.max.y);
        wr_f32(out, at + 20, bounds.max.z);
        wr_u32(out, at + 24, a);
        wr_u32(out, at + 28, b);
    };
    if order.len() <= TCOL_LEAF_TRIS || depth + 1 >= TCOL_MAX_DEPTH {
        put(out, first as u32, LEAF | order.len() as u32);
        return me;
    }
    // Split at the median centroid along the widest axis.
    let c = |t: u32| tri_bounds(verts, tris[t as usize]).center();
    let mut cb = Aabb::new(c(order[0]), c(order[0]));
    for &t in order.iter() {
        let p = c(t);
        cb = cb.union(&Aabb::new(p, p));
    }
    let e = cb.max - cb.min;
    let axis = if e.x >= e.y && e.x >= e.z {
        0
    } else if e.y >= e.z {
        1
    } else {
        2
    };
    let key = |t: u32| {
        let p = c(t);
        match axis {
            0 => p.x,
            1 => p.y,
            _ => p.z,
        }
    };
    order.sort_unstable_by(|&a, &b| key(a).total_cmp(&key(b)).then(a.cmp(&b)));
    let mid = order.len() / 2;
    let (lo, hi) = order.split_at_mut(mid);
    let l = build_node(verts, tris, lo, first, out, nodes_at, count, depth + 1);
    let r = build_node(
        verts,
        tris,
        hi,
        first + mid,
        out,
        nodes_at,
        count,
        depth + 1,
    );
    put(out, l as u32, r as u32);
    me
}

/// A parsed `TCOL` blob (borrowed, read in place).
#[derive(Clone, Copy, Debug)]
pub struct MeshCollider<'a> {
    data: &'a [u8],
    verts: usize,
    tris: usize,
    nodes: usize,
}

impl<'a> MeshCollider<'a> {
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn parse(data: &'a [u8]) -> Result<Self, TcolError> {
        if data.len() < TCOL_HEADER {
            return Err(TcolError::Truncated);
        }
        if &data[..4] != TCOL_MAGIC {
            return Err(TcolError::Magic);
        }
        let v = rd_u16(data, 4);
        if v != TCOL_VERSION {
            return Err(TcolError::Version(v));
        }
        let (verts, tris, nodes) = (
            rd_u32(data, 8) as usize,
            rd_u32(data, 12) as usize,
            rd_u32(data, 16) as usize,
        );
        let need = TCOL_HEADER
            .checked_add(verts.checked_mul(12).ok_or(TcolError::Truncated)?)
            .and_then(|n| n.checked_add(tris.checked_mul(8)?))
            .and_then(|n| n.checked_add(nodes.checked_mul(NODE_BYTES)?))
            .ok_or(TcolError::Truncated)?;
        if data.len() < need || nodes == 0 {
            return Err(TcolError::Truncated);
        }
        let m = MeshCollider {
            data,
            verts,
            tris,
            nodes,
        };
        for t in 0..tris {
            if m.tri(t).iter().any(|&i| i as usize >= verts) {
                return Err(TcolError::BadIndex);
            }
        }
        for n in 0..nodes {
            let (_, a, b) = m.node(n);
            if b & LEAF != 0 {
                let count = (b & !LEAF) as usize;
                if (a as usize).checked_add(count).is_none_or(|e| e > tris) {
                    return Err(TcolError::BadNode);
                }
            } else if a as usize <= n
                || b as usize <= n
                || a as usize >= nodes
                || b as usize >= nodes
            {
                return Err(TcolError::BadNode);
            }
        }
        if m.depth_of(0, 0) > TCOL_MAX_DEPTH {
            return Err(TcolError::TooDeep);
        }
        Ok(m)
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn depth_of(&self, n: usize, d: usize) -> usize {
        if d > TCOL_MAX_DEPTH {
            return d;
        }
        let (_, a, b) = self.node(n);
        if b & LEAF != 0 {
            d + 1
        } else {
            self.depth_of(a as usize, d + 1)
                .max(self.depth_of(b as usize, d + 1))
        }
    }

    pub fn vertex_count(&self) -> usize {
        self.verts
    }

    pub fn triangle_count(&self) -> usize {
        self.tris
    }

    pub fn node_count(&self) -> usize {
        self.nodes
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn vertex(&self, i: usize) -> Vec3 {
        let at = TCOL_HEADER + i * 12;
        Vec3::new(
            rd_f32(self.data, at),
            rd_f32(self.data, at + 4),
            rd_f32(self.data, at + 8),
        )
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn tri(&self, t: usize) -> [u16; 3] {
        let at = TCOL_HEADER + self.verts * 12 + t * 8;
        [
            rd_u16(self.data, at),
            rd_u16(self.data, at + 2),
            rd_u16(self.data, at + 4),
        ]
    }

    /// Local bounds of the whole mesh.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn bounds(&self) -> Aabb {
        self.node(0).0
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn node(&self, n: usize) -> (Aabb, u32, u32) {
        let at = TCOL_HEADER + self.verts * 12 + self.tris * 8 + n * NODE_BYTES;
        let d = self.data;
        (
            Aabb::new(
                Vec3::new(rd_f32(d, at), rd_f32(d, at + 4), rd_f32(d, at + 8)),
                Vec3::new(rd_f32(d, at + 12), rd_f32(d, at + 16), rd_f32(d, at + 20)),
            ),
            rd_u32(d, at + 24),
            rd_u32(d, at + 28),
        )
    }

    /// Visit the triangles whose leaf boxes meet `q` (local space); the
    /// visit stops when `f` returns `false`.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn for_each_tri(&self, q: &Aabb, mut f: impl FnMut(usize) -> bool) {
        let mut stack = [0u32; TCOL_MAX_DEPTH + 2];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let (b, l, r) = self.node(stack[sp] as usize);
            if !touches(&b, q) {
                continue;
            }
            if r & LEAF != 0 {
                for t in l as usize..(l + (r & !LEAF)) as usize {
                    if !f(t) {
                        return;
                    }
                }
            } else {
                stack[sp] = r;
                stack[sp + 1] = l;
                sp += 2;
            }
        }
    }

    /// Nearest hit of a local-space ray: (t, triangle).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn ray(&self, o: Vec3, d: Vec3, max_t: f32) -> Option<(f32, usize)> {
        let mut best: Option<(f32, usize)> = None;
        let mut stack = [0u32; TCOL_MAX_DEPTH + 2];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let limit = best.map_or(max_t, |b| b.0);
            let (b, l, r) = self.node(stack[sp] as usize);
            let grown = Aabb::new(b.min - Vec3::ONE * 1e-4, b.max + Vec3::ONE * 1e-4);
            if ray_box(o, d, limit, &grown).is_none() {
                continue;
            }
            if r & LEAF != 0 {
                for t in l as usize..(l + (r & !LEAF)) as usize {
                    let [a, b2, c] = self.tri(t);
                    if let Some(h) = ray_tri(
                        o,
                        d,
                        self.vertex(a as usize),
                        self.vertex(b2 as usize),
                        self.vertex(c as usize),
                    ) && h <= limit
                        && best.is_none_or(|(bt, bi)| h < bt || (h == bt && t < bi))
                    {
                        best = Some((h, t));
                    }
                }
            } else {
                stack[sp] = r;
                stack[sp + 1] = l;
                sp += 2;
            }
        }
        best
    }
}

/// Inclusive overlap (a query box touching a leaf still visits it).
#[inline]
fn touches(a: &Aabb, b: &Aabb) -> bool {
    a.min.x <= b.max.x
        && b.min.x <= a.max.x
        && a.min.y <= b.max.y
        && b.min.y <= a.max.y
        && a.min.z <= b.max.z
        && b.min.z <= a.max.z
}

/// Möller–Trumbore, both faces; `t >= 0`.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn ray_tri(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t)
}

/// The inverse of an affine `Mat34` (rotation, scale, translation).
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn affine_inverse(m: &Mat34) -> Mat34 {
    let a = &m.m;
    let (m00, m01, m02) = (a[0][0], a[0][1], a[0][2]);
    let (m10, m11, m12) = (a[1][0], a[1][1], a[1][2]);
    let (m20, m21, m22) = (a[2][0], a[2][1], a[2][2]);
    let c00 = m11 * m22 - m12 * m21;
    let c01 = m12 * m20 - m10 * m22;
    let c02 = m10 * m21 - m11 * m20;
    let det = m00 * c00 + m01 * c01 + m02 * c02;
    let inv = if det.abs() < 1e-20 { 0.0 } else { 1.0 / det };
    let r = [
        [
            c00 * inv,
            (m02 * m21 - m01 * m22) * inv,
            (m01 * m12 - m02 * m11) * inv,
        ],
        [
            c01 * inv,
            (m00 * m22 - m02 * m20) * inv,
            (m02 * m10 - m00 * m12) * inv,
        ],
        [
            c02 * inv,
            (m01 * m20 - m00 * m21) * inv,
            (m00 * m11 - m01 * m10) * inv,
        ],
    ];
    let t = Vec3::new(a[0][3], a[1][3], a[2][3]);
    let mut out = [[0.0f32; 4]; 3];
    for i in 0..3 {
        out[i][..3].copy_from_slice(&r[i]);
        out[i][3] = -(r[i][0] * t.x + r[i][1] * t.y + r[i][2] * t.z);
    }
    Mat34 { m: out }
}

/// A [`MeshCollider`] placed in the world.
#[derive(Clone, Copy, Debug)]
pub struct MeshInstance<'a> {
    pub mesh: MeshCollider<'a>,
    pub world: Mat34,
    inv: Mat34,
    /// Layer bits, like a [`Solid`](super::Solid)'s.
    pub layer: u16,
}

/// A ray's hit on a mesh.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MeshHit {
    pub t: f32,
    pub tri: usize,
    /// World-space face normal, facing the ray.
    pub normal: Vec3,
}

impl<'a> MeshInstance<'a> {
    pub fn new(mesh: MeshCollider<'a>, world: Mat34) -> Self {
        MeshInstance {
            mesh,
            world,
            inv: affine_inverse(&world),
            layer: 1,
        }
    }

    pub fn on_layer(mut self, layer: u16) -> Self {
        self.layer = layer;
        self
    }

    /// World-space corners of triangle `t`.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn triangle(&self, t: usize) -> [Vec3; 3] {
        let [a, b, c] = self.mesh.tri(t);
        [
            self.world.transform_point(self.mesh.vertex(a as usize)),
            self.world.transform_point(self.mesh.vertex(b as usize)),
            self.world.transform_point(self.mesh.vertex(c as usize)),
        ]
    }

    /// World bounds.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn bounds(&self) -> Aabb {
        self.mesh.bounds().transformed(&self.world)
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn local_query(&self, q: &Aabb) -> Aabb {
        q.transformed(&self.inv)
    }

    /// Nearest hit of a world ray (`dir` normalized).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<MeshHit> {
        let o = self.inv.transform_point(origin);
        let d = self.inv.transform_dir(dir);
        let (t, tri) = self.mesh.ray(o, d, max_t)?;
        let [a, b, c] = self.triangle(tri);
        let mut n = (b - a).cross(c - a);
        if n.dot(dir) > 0.0 {
            n = -n;
        }
        Some(MeshHit {
            t,
            tri,
            normal: n.normalized(),
        })
    }

    /// Triangles meeting a world box (tools, overlays): returns how many,
    /// `out` keeps the first ones visited.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn tris_in(&self, q: &Aabb, out: &mut [u32]) -> usize {
        let mut n = 0;
        self.mesh.for_each_tri(&self.local_query(q), |t| {
            if n < out.len() {
                out[n] = t as u32;
            }
            n += 1;
            true
        });
        n
    }
}

#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn unit_normal(t: &[Vec3; 3]) -> Vec3 {
    let n = (t[1] - t[0]).cross(t[2] - t[0]);
    let l = sqrt(n.dot(n));
    if l < 1e-12 { Vec3::ZERO } else { n * (1.0 / l) }
}

/// Does the XZ rectangle overlap triangle `t`'s XZ shadow (SAT)?
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn rect_tri_xz(min: Vec3, max: Vec3, t: &[Vec3; 3]) -> bool {
    let (tx0, tx1) = (
        t[0].x.min(t[1].x).min(t[2].x),
        t[0].x.max(t[1].x).max(t[2].x),
    );
    let (tz0, tz1) = (
        t[0].z.min(t[1].z).min(t[2].z),
        t[0].z.max(t[1].z).max(t[2].z),
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
        let (a, b, c) = (t[i], t[(i + 1) % 3], t[(i + 2) % 3]);
        // Edge normal in XZ, oriented away from the third vertex.
        let (nx, nz) = (b.z - a.z, a.x - b.x);
        let side = |x: f32, z: f32| nx * (x - a.x) + nz * (z - a.z);
        let third = side(c.x, c.z);
        if third == 0.0 {
            continue;
        }
        if corners.iter().all(|&(x, z)| side(x, z) * third <= 0.0) {
            return false;
        }
    }
    true
}

/// Akenine-Möller triangle / box overlap (strict enough for walls).
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn tri_box(c: Vec3, h: Vec3, t: &[Vec3; 3]) -> bool {
    let v = [t[0] - c, t[1] - c, t[2] - c];
    let e = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    let axes_hit = |ax: Vec3| {
        let p = [v[0].dot(ax), v[1].dot(ax), v[2].dot(ax)];
        let r = h.x * ax.x.abs() + h.y * ax.y.abs() + h.z * ax.z.abs();
        let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
        !(lo >= r || hi <= -r)
    };
    let units = [
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
    ];
    for u in units {
        for ed in e {
            let ax = u.cross(ed);
            if ax.dot(ax) > 1e-12 && !axes_hit(ax) {
                return false;
            }
        }
    }
    for u in units {
        if !axes_hit(u) {
            return false;
        }
    }
    let n = e[0].cross(e[1]);
    n.dot(n) <= 1e-12 || axes_hit(n)
}

impl MeshQuery<f32> for MeshInstance<'_> {
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn support(&self, foot: &Box3<f32>, up_to: f32, min_normal_y: f32) -> Option<MeshSupport<f32>> {
        let column = Aabb::new(
            Vec3::new(foot.min.x, up_to - 1.0e4, foot.min.z),
            Vec3::new(foot.max.x, up_to, foot.max.z),
        );
        let (fmin, fmax) = (Vec3::from(foot.min), Vec3::from(foot.max));
        let mut best: Option<MeshSupport<f32>> = None;
        self.mesh.for_each_tri(&self.local_query(&column), |ti| {
            let t = self.triangle(ti);
            let n = unit_normal(&t);
            if n.y < min_normal_y || n.y <= 0.0 || !rect_tri_xz(fmin, fmax, &t) {
                return true;
            }
            // The plane's highest point over the footprint, within the
            // triangle's own height range.
            let x = if n.x < 0.0 { fmax.x } else { fmin.x };
            let z = if n.z < 0.0 { fmax.z } else { fmin.z };
            let y = t[0].y - (n.x * (x - t[0].x) + n.z * (z - t[0].z)) / n.y;
            let top = t[0].y.max(t[1].y).max(t[2].y);
            let low = t[0].y.min(t[1].y).min(t[2].y);
            let y = y.clamp(low, top);
            if y <= up_to
                && best.is_none_or(|b| y > b.height || (y == b.height && (ti as u32) < b.tri))
            {
                best = Some(MeshSupport {
                    height: y,
                    normal: V3::from(n),
                    tri: ti as u32,
                });
            }
            true
        });
        best
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn blocks(&self, body: &Box3<f32>, min_normal_y: f32) -> bool {
        let q = body.to_aabb();
        let (c, h) = (q.center(), q.half());
        let mut hit = false;
        self.mesh.for_each_tri(&self.local_query(&q), |ti| {
            let t = self.triangle(ti);
            let n = unit_normal(&t);
            if n.y < min_normal_y && tri_box(c, h, &t) {
                hit = true;
                return false;
            }
            true
        });
        hit
    }
}
