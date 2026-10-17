//! Baked vertex ambient occlusion: the console tier of `trino_core::ao`
//! (ADR 0014). `ao = { ... }` on a `[models.*]` entry darkens each
//! vertex color by how much of the sky above it the model itself hides —
//! creases, inner corners, the underside of an overhang, and (with
//! `ground = true`) the band where it meets a floor at its lowest point.
//!
//! The colors are the model's, so every target draws the same occlusion
//! at no run-time cost: the N64's RSP and CPU T&L, the 3DS's PICA200 and
//! the PC all read vertex colors already.
//!
//! Each vertex casts a fixed set of cosine-weighted rays into the
//! hemisphere around its normal and counts the model's own triangles
//! (through a small BVH) within `distance`, nearer hits weighing more.
//! The directions come from `trino_core::math3d`'s own sin/cos/sqrt, so a
//! bake writes the same bytes on every host.

use serde::Deserialize;
use trino_core::math3d::{cos, sin, sqrt};

use crate::model::MeshData;

/// `ao = true` or `ao = { strength = .., distance = .., rays = .., ground
/// = .. }` in a `[models.*]` entry.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum AoDecl {
    Flag(bool),
    Params(AoBake),
}

impl AoDecl {
    /// The bake settings, or `None` for `ao = false`.
    pub fn bake(&self) -> Option<AoBake> {
        match self {
            AoDecl::Flag(true) => Some(AoBake::default()),
            AoDecl::Flag(false) => None,
            AoDecl::Params(p) => Some(p.clone()),
        }
    }
}

/// Vertex AO bake settings.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AoBake {
    /// How dark a fully occluded vertex gets, 0 (unchanged) to 1 (black).
    pub strength: f32,
    /// How far occluders count, model units. 0 = a fifth of the model's
    /// bounding-box diagonal.
    pub distance: f32,
    /// Rays per vertex (8..=1024).
    pub rays: u32,
    /// Also occlude with a floor at the model's lowest point: props and
    /// characters darken where they stand.
    pub ground: bool,
}

impl Default for AoBake {
    fn default() -> Self {
        AoBake {
            strength: 0.8,
            distance: 0.0,
            rays: 64,
            ground: false,
        }
    }
}

impl AoBake {
    /// The manifest's inline-table form (what the editor writes).
    pub fn to_toml(&self) -> String {
        let d = AoBake::default();
        let mut parts = vec![format!("strength = {}", num(self.strength))];
        if self.distance > 0.0 {
            parts.push(format!("distance = {}", num(self.distance)));
        }
        if self.rays != d.rays {
            parts.push(format!("rays = {}", self.rays));
        }
        if self.ground {
            parts.push("ground = true".into());
        }
        format!("{{ {} }}", parts.join(", "))
    }
}

/// Shortest decimal that round-trips, always with a decimal point (TOML
/// floats).
fn num(v: f32) -> String {
    let s = format!("{v}");
    if s.contains('.') { s } else { format!("{s}.0") }
}

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(a: V3) -> Option<V3> {
    let l2 = dot(a, a);
    (l2 > 1e-20).then(|| scale(a, 1.0 / sqrt(l2)))
}

/// Darken `mesh`'s vertex colors (RGB; alpha stays) by their baked
/// occlusion. Meshes without normals or colors are left alone.
pub fn bake_vertex_ao(mesh: &mut MeshData, opts: &AoBake) {
    let n = mesh.positions.len();
    if n == 0 || mesh.normals.len() != n || mesh.colors.len() != n {
        return;
    }
    let occ = occlusion(mesh, opts);
    let strength = opts.strength.clamp(0.0, 1.0);
    for (c, o) in mesh.colors.iter_mut().zip(occ) {
        let k = 1.0 - strength * o;
        for ch in &mut c[..3] {
            *ch = (*ch as f32 * k + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
}

/// Occlusion per vertex, 0 (open sky) to 1 (fully hidden).
pub fn occlusion(mesh: &MeshData, opts: &AoBake) -> Vec<f32> {
    let tris: Vec<[V3; 3]> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| mesh.positions[i as usize]))
        .collect();
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &mesh.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let diag = sqrt(dot(sub(hi, lo), sub(hi, lo))).max(1e-6);
    let distance = if opts.distance > 0.0 {
        opts.distance
    } else {
        diag * 0.2
    };
    // Rays leave a hair above the surface, so they never hit the
    // triangles of their own vertex, and start a hair behind it along the
    // ray, so a vertex lying on another surface's plane (where a floor
    // meets a wall) still sees that surface.
    let eps = diag * 1e-4;
    let ground = opts.ground.then_some(lo[1]);
    let dirs = hemisphere(opts.rays.clamp(8, 1024));
    let bvh = Bvh::new(&tris);
    mesh.positions
        .iter()
        .zip(&mesh.normals)
        .map(|(&p, &nrm)| {
            let Some(n) = normalize(nrm) else {
                return 0.0;
            };
            // Any vector not parallel to n makes the tangent frame.
            let up = if n[1].abs() < 0.9 {
                [0.0, 1.0, 0.0]
            } else {
                [1.0, 0.0, 0.0]
            };
            let t = normalize(cross(up, n)).unwrap_or([1.0, 0.0, 0.0]);
            let b = cross(n, t);
            let lifted = add(p, scale(n, eps));
            let mut occ = 0.0;
            for d in &dirs {
                let dir = add(add(scale(t, d[0]), scale(b, d[1])), scale(n, d[2]));
                let origin = sub(lifted, scale(dir, eps));
                let mut hit = bvh.nearest(&tris, origin, dir, distance);
                if let Some(g) = ground
                    && dir[1] < -1e-6
                {
                    let tg = (g - origin[1]) / dir[1];
                    if tg >= 0.0 && tg < hit.unwrap_or(distance) {
                        hit = Some(tg);
                    }
                }
                if let Some(t) = hit {
                    occ += 1.0 - t / distance;
                }
            }
            occ / dirs.len() as f32
        })
        .collect()
}

/// `count` cosine-weighted unit directions around +Z on a golden-angle
/// spiral (tangent space: x, y along the surface, z along the normal).
fn hemisphere(count: u32) -> Vec<V3> {
    (0..count)
        .map(|i| {
            let h = (i as f32 + 0.5) / count as f32;
            let phi = i as f32 * 2.399_963;
            let (s, c) = (sqrt(h), sqrt(1.0 - h));
            [cos(phi) * s, sin(phi) * s, c]
        })
        .collect()
}

/// Möller–Trumbore, both sides: the distance along `dir` to `tri`.
fn intersect(tri: &[V3; 3], o: V3, dir: V3) -> Option<f32> {
    let e1 = sub(tri[1], tri[0]);
    let e2 = sub(tri[2], tri[0]);
    let pv = cross(dir, e2);
    let det = dot(e1, pv);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let tv = sub(o, tri[0]);
    let u = dot(tv, pv) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qv = cross(tv, e1);
    let v = dot(dir, qv) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, qv) * inv;
    (t > 0.0).then_some(t)
}

/// A bounding-volume hierarchy over the triangles: median splits on the
/// widest axis, leaves of up to 4.
struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

struct Node {
    lo: V3,
    hi: V3,
    /// Leaf: `count > 0`, triangles `order[first..first + count]`.
    /// Inner: children at `first` and `first + 1`.
    first: u32,
    count: u32,
}

impl Bvh {
    fn new(tris: &[[V3; 3]]) -> Bvh {
        let mut bvh = Bvh {
            nodes: Vec::new(),
            order: (0..tris.len() as u32).collect(),
        };
        if !tris.is_empty() {
            bvh.nodes.push(Node {
                lo: [0.0; 3],
                hi: [0.0; 3],
                first: 0,
                count: 0,
            });
            bvh.build(tris, 0, 0, tris.len());
        }
        bvh
    }

    fn build(&mut self, tris: &[[V3; 3]], node: usize, start: usize, end: usize) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for &i in &self.order[start..end] {
            for p in &tris[i as usize] {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        self.nodes[node].lo = lo;
        self.nodes[node].hi = hi;
        if end - start <= 4 {
            self.nodes[node].first = start as u32;
            self.nodes[node].count = (end - start) as u32;
            return;
        }
        let ext = sub(hi, lo);
        let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] {
            0
        } else if ext[1] >= ext[2] {
            1
        } else {
            2
        };
        let centroid = |i: u32| {
            let t = &tris[i as usize];
            t[0][axis] + t[1][axis] + t[2][axis]
        };
        // Stable sort by centroid (index as tie-break): deterministic.
        self.order[start..end]
            .sort_by(|&a, &b| centroid(a).total_cmp(&centroid(b)).then(a.cmp(&b)));
        let mid = (start + end) / 2;
        let left = self.nodes.len();
        for _ in 0..2 {
            self.nodes.push(Node {
                lo: [0.0; 3],
                hi: [0.0; 3],
                first: 0,
                count: 0,
            });
        }
        self.nodes[node].first = left as u32;
        self.build(tris, left, start, mid);
        self.build(tris, left + 1, mid, end);
    }

    /// The nearest hit closer than `max`.
    fn nearest(&self, tris: &[[V3; 3]], o: V3, dir: V3, max: f32) -> Option<f32> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = dir.map(|d| if d.abs() > 1e-12 { 1.0 / d } else { f32::MAX });
        let mut best = max;
        let mut found = false;
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i];
            if !slab(node, o, inv, best) {
                continue;
            }
            if node.count > 0 {
                let range = node.first as usize..(node.first + node.count) as usize;
                for &t in &self.order[range] {
                    if let Some(d) = intersect(&tris[t as usize], o, dir)
                        && d < best
                    {
                        best = d;
                        found = true;
                    }
                }
            } else {
                stack.push(node.first as usize);
                stack.push(node.first as usize + 1);
            }
        }
        found.then_some(best)
    }
}

/// Does the ray enter the node's box before `max`?
fn slab(node: &Node, o: V3, inv: V3, max: f32) -> bool {
    let (mut t0, mut t1) = (0.0f32, max);
    for k in 0..3 {
        let a = (node.lo[k] - o[k]) * inv[k];
        let b = (node.hi[k] - o[k]) * inv[k];
        let (near, far) = if a <= b { (a, b) } else { (b, a) };
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An L: a 2×2 floor quad at y = 0 and a 2×2 wall quad at z = 1 facing
    /// -z, meeting along their shared edge.
    fn corner() -> MeshData {
        let positions = vec![
            // floor
            [-1.0, 0.0, -1.0],
            [1.0, 0.0, -1.0],
            [1.0, 0.0, 1.0],
            [-1.0, 0.0, 1.0],
            // wall
            [-1.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 2.0, 1.0],
            [-1.0, 2.0, 1.0],
        ];
        let normals = vec![
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, -1.0],
        ];
        MeshData {
            positions,
            normals,
            colors: vec![[200, 200, 200, 255]; 8],
            indices: vec![0, 2, 1, 0, 3, 2, 4, 6, 5, 4, 7, 6],
            ..MeshData::default()
        }
    }

    #[test]
    fn the_corner_darkens_and_the_open_edges_dont() {
        let mut m = corner();
        let opts = AoBake {
            distance: 1.5,
            ..AoBake::default()
        };
        let occ = occlusion(&m, &opts);
        // The floor's far edge (z = -1) and the wall's top: open.
        for i in [0, 1, 6, 7] {
            assert!(occ[i] < 0.05, "vertex {i}: {}", occ[i]);
        }
        // Where floor meets wall: about half the hemisphere is wall.
        for i in [2, 3, 4, 5] {
            assert!(occ[i] > 0.2, "vertex {i}: {}", occ[i]);
        }
        bake_vertex_ao(&mut m, &opts);
        assert_eq!(m.colors[0], [200, 200, 200, 255]);
        assert!(m.colors[2][0] < 180 && m.colors[2][3] == 255);
        assert_eq!(m.colors[2][0], m.colors[2][1], "gray stays gray");
    }

    #[test]
    fn the_ground_plane_darkens_where_the_model_stands() {
        // A lone wall: nothing of its own occludes it, the ground does,
        // near its foot only.
        let mut m = corner();
        m.positions.drain(..4);
        m.normals.drain(..4);
        m.colors.drain(..4);
        m.indices = vec![0, 2, 1, 0, 3, 2];
        let opts = AoBake {
            distance: 1.0,
            ..AoBake::default()
        };
        assert!(occlusion(&m, &opts).iter().all(|&o| o == 0.0));
        let occ = occlusion(
            &m,
            &AoBake {
                ground: true,
                ..opts.clone()
            },
        );
        assert!(occ[0] > 0.2 && occ[1] > 0.2, "foot: {occ:?}");
        assert!(occ[2] == 0.0 && occ[3] == 0.0, "top: {occ:?}");
    }

    #[test]
    fn strength_zero_or_no_normals_change_nothing() {
        let mut m = corner();
        let before = m.colors.clone();
        bake_vertex_ao(
            &mut m,
            &AoBake {
                strength: 0.0,
                ..AoBake::default()
            },
        );
        assert_eq!(m.colors, before);
        m.normals.clear();
        bake_vertex_ao(&mut m, &AoBake::default());
        assert_eq!(m.colors, before);
    }

    #[test]
    fn the_bvh_finds_what_brute_force_finds() {
        // A 6×6 grid of quads under a ray fan: same nearest hits.
        let mut tris = Vec::new();
        for z in 0..6 {
            for x in 0..6 {
                let (x, z) = (x as f32, z as f32);
                let y = (x * 0.7 + z * 0.3).sin() * 0.2;
                tris.push([[x, y, z], [x + 1.0, y, z], [x, y, z + 1.0]]);
                tris.push([[x + 1.0, y, z], [x + 1.0, y, z + 1.0], [x, y, z + 1.0]]);
            }
        }
        let bvh = Bvh::new(&tris);
        for d in hemisphere(64) {
            let dir = [d[0], -d[2], d[1]];
            let o = [3.1, 2.0, 2.9];
            let brute = tris
                .iter()
                .filter_map(|t| intersect(t, o, dir))
                .filter(|&t| t < 50.0)
                .min_by(f32::total_cmp);
            assert_eq!(bvh.nearest(&tris, o, dir, 50.0), brute, "{dir:?}");
        }
    }

    #[test]
    fn the_manifest_forms_parse_and_write_back() {
        #[derive(Deserialize)]
        struct M {
            ao: AoDecl,
        }
        let p = |s: &str| toml::from_str::<M>(s).unwrap().ao.bake();
        assert_eq!(p("ao = true"), Some(AoBake::default()));
        assert_eq!(p("ao = false"), None);
        let tuned =
            p("ao = { strength = 0.5, distance = 0.4, rays = 128, ground = true }").unwrap();
        assert_eq!(
            tuned,
            AoBake {
                strength: 0.5,
                distance: 0.4,
                rays: 128,
                ground: true
            }
        );
        assert_eq!(
            tuned.to_toml(),
            "{ strength = 0.5, distance = 0.4, rays = 128, ground = true }"
        );
        assert_eq!(p(&format!("ao = {}", tuned.to_toml())), Some(tuned));
        assert_eq!(AoBake::default().to_toml(), "{ strength = 0.8 }");
        assert!(toml::from_str::<M>("ao = { strenght = 1.0 }").is_err());
    }
}
