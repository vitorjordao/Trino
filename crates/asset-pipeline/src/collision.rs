//! Collision meshes (issue #66 phase 3): a manifest model's `collision` key
//! bakes a `TCOL` blob (`trino_core::physics::MeshCollider`: triangles and
//! a BVH) at `collision/<name>`, on every target — collision is gameplay,
//! so a model's per-platform variants never change it.
//!
//! ```toml
//! [models.castle]
//! file = "models/castle.glb"
//! collision = "models/castle_col.glb"   # a simplified mesh (a master path)
//! # collision = "visual"                # the visual mesh itself
//! # collision = "auto-hull"             # the visual mesh's convex hull
//! ```
//!
//! The project's layer matrix (`assets/physics.ron`) bakes beside them as
//! `physics/layers`. Both go out with the UI blobs ([`crate::ui::bake`]).

use std::path::Path;

use trino_core::Vec3;
use trino_core::physics::{MeshCollider, tcol_bytes, write_tcol};

use crate::manifest::{Manifest, ModelDecl, Platform};
use crate::resolve::resolve_source;

/// Where a model's collision comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollisionSource {
    /// The visual mesh's triangles.
    Visual,
    /// The convex hull of the visual mesh.
    AutoHull,
    /// A separate (simplified) master, like `file`.
    File(String),
}

impl CollisionSource {
    pub fn parse(s: &str) -> CollisionSource {
        match s {
            "visual" | "mesh" => CollisionSource::Visual,
            "auto-hull" | "hull" => CollisionSource::AutoHull,
            other => CollisionSource::File(other.to_string()),
        }
    }
}

/// A model's baked collision and what it holds.
#[derive(Clone, Debug)]
pub struct CollisionMesh {
    /// `models/<name>`.
    pub model: String,
    /// `collision/<name>`.
    pub logical: String,
    pub source: CollisionSource,
    pub vertices: usize,
    pub triangles: usize,
    pub nodes: usize,
    pub bytes: Vec<u8>,
}

/// Triangles of a master (`.glb`/`.gltf` or a `.prim.ron`).
/// Positions and triangles of a master.
pub type Triangles = (Vec<[f32; 3]>, Vec<[u32; 3]>);

pub fn load_triangles(path: &Path) -> Result<Triangles, String> {
    let mesh = crate::model::load_mesh(path)?;
    let tris = mesh.indices.as_chunks::<3>().0.to_vec();
    Ok((mesh.positions, tris))
}

/// Merge equal positions and drop degenerate triangles: what a collider
/// needs (the visual mesh repeats a corner per normal / color).
pub fn weld(verts: &[[f32; 3]], tris: &[[u32; 3]]) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let mut map = std::collections::HashMap::new();
    let mut out_v = Vec::new();
    let mut remap = Vec::with_capacity(verts.len());
    for v in verts {
        let key = [v[0].to_bits(), v[1].to_bits(), v[2].to_bits()];
        let i = *map.entry(key).or_insert_with(|| {
            out_v.push(Vec3::new(v[0], v[1], v[2]));
            out_v.len() as u32 - 1
        });
        remap.push(i);
    }
    let mut out_t = Vec::new();
    for t in tris {
        let [a, b, c] = [
            remap[t[0] as usize],
            remap[t[1] as usize],
            remap[t[2] as usize],
        ];
        if a == b || b == c || a == c {
            continue;
        }
        let (pa, pb, pc) = (out_v[a as usize], out_v[b as usize], out_v[c as usize]);
        let n = (pb - pa).cross(pc - pa);
        if n.dot(n) <= 1e-14 {
            continue;
        }
        out_t.push([a, b, c]);
    }
    (out_v, out_t)
}

/// A `TCOL` blob of the triangles.
pub fn to_tcol(verts: &[Vec3], tris: &[[u32; 3]]) -> Result<Vec<u8>, String> {
    if verts.len() > u16::MAX as usize {
        return Err(format!(
            "{} vertices: a collision mesh holds at most 65535 (simplify it)",
            verts.len()
        ));
    }
    let tris16: Vec<[u16; 3]> = tris
        .iter()
        .map(|t| [t[0] as u16, t[1] as u16, t[2] as u16])
        .collect();
    let mut out = vec![0u8; tcol_bytes(verts.len(), tris16.len())];
    let mut order = vec![0u32; tris16.len()];
    let n = write_tcol(verts, &tris16, &mut order, &mut out).map_err(|e| format!("{e:?}"))?;
    out.truncate(n);
    Ok(out)
}

/// The convex hull of `points` as outward-facing triangles over the input
/// indices (incremental, f64, deterministic: points are added in order).
pub fn convex_hull(points: &[Vec3]) -> Result<Vec<[u32; 3]>, String> {
    type P = [f64; 3];
    let p: Vec<P> = points
        .iter()
        .map(|v| [v.x as f64, v.y as f64, v.z as f64])
        .collect();
    let sub = |a: P, b: P| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: P, b: P| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: P, b: P| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    if p.len() < 4 {
        return Err("auto-hull needs at least 4 points".into());
    }
    let scale = p
        .iter()
        .flat_map(|q| q.iter().map(|c| c.abs()))
        .fold(1.0f64, f64::max);
    let eps = scale * 1e-9;
    // An initial tetrahedron from extreme points.
    let i0 = (0..p.len())
        .min_by(|&a, &b| p[a][0].total_cmp(&p[b][0]).then(a.cmp(&b)))
        .unwrap();
    let far = |from: &dyn Fn(usize) -> f64| {
        (0..p.len())
            .max_by(|&a, &b| from(a).total_cmp(&from(b)).then(b.cmp(&a)))
            .unwrap()
    };
    let i1 = far(&|i| {
        let d = sub(p[i], p[i0]);
        dot(d, d)
    });
    let i2 = far(&|i| {
        let c = cross(sub(p[i1], p[i0]), sub(p[i], p[i0]));
        dot(c, c)
    });
    let n012 = cross(sub(p[i1], p[i0]), sub(p[i2], p[i0]));
    let i3 = far(&|i| dot(n012, sub(p[i], p[i0])).abs());
    if dot(n012, n012).sqrt() <= eps || dot(n012, sub(p[i3], p[i0])).abs() <= eps * scale {
        return Err("auto-hull: the mesh is flat (no volume); use `collision = \"visual\"`".into());
    }
    let centroid = {
        let s = [i0, i1, i2, i3].iter().fold([0.0; 3], |a, &i| {
            [a[0] + p[i][0], a[1] + p[i][1], a[2] + p[i][2]]
        });
        [s[0] / 4.0, s[1] / 4.0, s[2] / 4.0]
    };
    #[derive(Clone)]
    struct Face {
        v: [usize; 3],
        n: P,
        d: f64,
        alive: bool,
    }
    let make = |a: usize, b: usize, c: usize| {
        let n = cross(sub(p[b], p[a]), sub(p[c], p[a]));
        let l = dot(n, n).sqrt().max(1e-300);
        let n = [n[0] / l, n[1] / l, n[2] / l];
        Face {
            v: [a, b, c],
            n,
            d: dot(n, p[a]),
            alive: true,
        }
    };
    let oriented = |a: usize, b: usize, c: usize| {
        let f = make(a, b, c);
        if dot(f.n, centroid) - f.d > 0.0 {
            make(a, c, b)
        } else {
            f
        }
    };
    let mut faces = vec![
        oriented(i0, i1, i2),
        oriented(i0, i1, i3),
        oriented(i0, i2, i3),
        oriented(i1, i2, i3),
    ];
    for (i, &q) in p.iter().enumerate() {
        if [i0, i1, i2, i3].contains(&i) {
            continue;
        }
        let visible: Vec<usize> = (0..faces.len())
            .filter(|&f| faces[f].alive && dot(faces[f].n, q) - faces[f].d > eps)
            .collect();
        if visible.is_empty() {
            continue;
        }
        let mut edges = std::collections::BTreeSet::new();
        for &f in &visible {
            let v = faces[f].v;
            for k in 0..3 {
                edges.insert((v[k], v[(k + 1) % 3]));
            }
        }
        for &f in &visible {
            faces[f].alive = false;
        }
        for &(a, b) in &edges {
            if !edges.contains(&(b, a)) {
                faces.push(make(a, b, i));
            }
        }
    }
    let out: Vec<[u32; 3]> = faces
        .iter()
        .filter(|f| f.alive)
        .map(|f| [f.v[0] as u32, f.v[1] as u32, f.v[2] as u32])
        .collect();
    Ok(out)
}

/// Bake `decl`'s collision (None without a `collision` key).
pub fn bake_model(
    assets_root: &Path,
    platform: Platform,
    name: &str,
    decl: &ModelDecl,
) -> Result<Option<CollisionMesh>, String> {
    let Some(key) = &decl.collision else {
        return Ok(None);
    };
    let model = format!("models/{name}");
    let source = CollisionSource::parse(key);
    let err = |e: String| format!("collision of `{model}`: {e}");
    let file = match &source {
        CollisionSource::File(f) => f.as_str(),
        _ => decl.file.as_str(),
    };
    let path = resolve_source(assets_root, platform, file).map_err(err)?;
    let (verts, tris) = load_triangles(&path).map_err(err)?;
    let (verts, tris) = weld(&verts, &tris);
    let tris = match source {
        CollisionSource::AutoHull => convex_hull(&verts).map_err(err)?,
        _ => tris,
    };
    if tris.is_empty() {
        return Err(err("no triangles".into()));
    }
    let bytes = to_tcol(&verts, &tris).map_err(err)?;
    let m = MeshCollider::parse(&bytes).map_err(|e| err(format!("{e:?}")))?;
    Ok(Some(CollisionMesh {
        logical: trino_scene::collision_blob_path(&model),
        model,
        source,
        vertices: m.vertex_count(),
        triangles: m.triangle_count(),
        nodes: m.node_count(),
        bytes,
    }))
}

/// Every model's collision mesh for `platform`.
pub fn bake_all(
    assets_root: &Path,
    manifest: &Manifest,
    platform: Platform,
) -> Result<Vec<CollisionMesh>, String> {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (name, decl) in &manifest.models {
        match bake_model(assets_root, platform, name, decl) {
            Ok(Some(m)) => out.push(m),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors.join("\n"))
    }
}

/// The collision blobs and the layer matrix for [`crate::ui::bake`].
pub fn blobs(
    root: &Path,
    manifest: &Manifest,
    platform: Platform,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out: Vec<(String, Vec<u8>)> = bake_all(&root.join("assets"), manifest, platform)?
        .into_iter()
        .map(|m| (m.logical, m.bytes))
        .collect();
    if root.join(trino_scene::physics::PHYSICS_FILE).is_file() {
        let s = trino_scene::physics::PhysicsSettings::load(root)?;
        out.push((
            trino_scene::physics::LAYERS_BLOB_PATH.to_string(),
            s.to_blob(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Vec<Vec3> {
        let mut v = vec![];
        for x in [0.0, 1.0] {
            for y in [0.0, 1.0] {
                for z in [0.0, 1.0] {
                    v.push(Vec3::new(x, y, z));
                }
            }
        }
        // Interior points the hull must ignore.
        v.push(Vec3::new(0.5, 0.5, 0.5));
        v.push(Vec3::new(0.2, 0.7, 0.4));
        v
    }

    #[test]
    fn the_hull_of_a_cube_is_twelve_outward_triangles() {
        let pts = cube();
        let hull = convex_hull(&pts).unwrap();
        assert_eq!(hull.len(), 12);
        let c = Vec3::new(0.5, 0.5, 0.5);
        for t in &hull {
            let [a, b, d] = [pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]];
            assert!(!t.contains(&8) && !t.contains(&9), "interior point used");
            let n = (b - a).cross(d - a);
            assert!(n.dot(a - c) > 0.0, "outward");
        }
        let blob = to_tcol(&pts, &hull).unwrap();
        let m = MeshCollider::parse(&blob).unwrap();
        assert_eq!(m.triangle_count(), 12);
        // Deterministic.
        assert_eq!(convex_hull(&pts).unwrap(), hull);
    }

    #[test]
    fn flat_input_has_no_hull_and_welding_merges_corners() {
        let flat = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 1.0),
        ];
        assert!(convex_hull(&flat).unwrap_err().contains("flat"));
        let verts = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
        ];
        let (v, t) = weld(&verts, &[[0, 1, 2], [3, 1, 2], [0, 3, 1]]);
        assert_eq!(v.len(), 3);
        assert_eq!(t, vec![[0, 1, 2], [0, 1, 2]], "the degenerate one is gone");
        assert_eq!(
            CollisionSource::parse("auto-hull"),
            CollisionSource::AutoHull
        );
        assert_eq!(
            CollisionSource::parse("models/x_col.glb"),
            CollisionSource::File("models/x_col.glb".into())
        );
    }
}
