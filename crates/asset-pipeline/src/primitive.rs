//! Procedural placeholder meshes — Trino's **fake objects**.
//!
//! A `*.prim.ron` file describes a parametric shape (box, sphere, cylinder,
//! ...) that bakes to the same TMDL as a real glTF model. Declare it in the
//! manifest like any model:
//!
//! ```toml
//! [models.crate]
//! file = "models/proxies/crate.prim.ron"
//! ```
//!
//! Level design and gameplay can start immediately with correctly sized,
//! colored stand-ins on **every target** (it is just a model). When the art
//! is ready, point `file` at the `.glb` — the logical path, and therefore
//! the handle, every scene entity, material and line of game code that
//! references it, stay the same.
//!
//! The editor's *Create → Fake object* dialog writes these files; so can an
//! AI agent or a human with a text editor. Saving one hot-reloads it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::MeshData;

/// Shape and its dimensions (meters).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Shape {
    /// Axis-aligned box of `size` (x, y, z).
    Box { size: [f32; 3] },
    /// UV sphere; `segments` around, `segments / 2` rings.
    Sphere { radius: f32 },
    /// Upright cylinder (Y axis).
    Cylinder { radius: f32, height: f32 },
    /// Upright cone, apex up.
    Cone { radius: f32, height: f32 },
    /// Upright capsule; `height` is the total height including the caps.
    Capsule { radius: f32, height: f32 },
    /// Flat plane on XZ, facing +Y.
    Plane { size: [f32; 2] },
    /// Wedge rising from -Z (height 0) to +Z (full height).
    Ramp { size: [f32; 3] },
    /// Ring on XZ (coins, wheels, portals).
    Torus { radius: f32, tube: f32 },
}

/// Where the mesh origin sits — the transform pivot in scenes.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pivot {
    #[default]
    Center,
    /// Origin at the bottom center: drop it on the floor at y = 0.
    Bottom,
}

/// A placeholder model, as stored in `*.prim.ron`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Primitive {
    pub shape: Shape,
    /// Base vertex color (RGBA).
    #[serde(default = "default_color")]
    pub color: [u8; 4],
    /// Optional color at the top: vertex colors blend bottom → top, which
    /// reads as volume even under flat light.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_color: Option<[u8; 4]>,
    #[serde(default)]
    pub pivot: Pivot,
    /// Subdivisions around round shapes (clamped to 3..=48).
    #[serde(default = "default_segments")]
    pub segments: u32,
    /// Smooth normals on round surfaces (false = faceted low-poly look).
    #[serde(default = "default_true")]
    pub smooth: bool,
}

fn default_color() -> [u8; 4] {
    [200, 200, 200, 255]
}
fn default_segments() -> u32 {
    12
}
fn default_true() -> bool {
    true
}

impl Default for Primitive {
    fn default() -> Self {
        Primitive {
            shape: Shape::Box {
                size: [1.0, 1.0, 1.0],
            },
            color: default_color(),
            top_color: None,
            pivot: Pivot::Center,
            segments: default_segments(),
            smooth: true,
        }
    }
}

impl Shape {
    /// Human-readable kind name (editor labels, file names).
    pub fn kind(&self) -> &'static str {
        match self {
            Shape::Box { .. } => "box",
            Shape::Sphere { .. } => "sphere",
            Shape::Cylinder { .. } => "cylinder",
            Shape::Cone { .. } => "cone",
            Shape::Capsule { .. } => "capsule",
            Shape::Plane { .. } => "plane",
            Shape::Ramp { .. } => "ramp",
            Shape::Torus { .. } => "torus",
        }
    }

    /// Every shape with unit-ish default dimensions (editor palette).
    pub fn all_defaults() -> [Shape; 8] {
        [
            Shape::Box {
                size: [1.0, 1.0, 1.0],
            },
            Shape::Sphere { radius: 0.5 },
            Shape::Cylinder {
                radius: 0.5,
                height: 1.0,
            },
            Shape::Cone {
                radius: 0.5,
                height: 1.0,
            },
            Shape::Capsule {
                radius: 0.35,
                height: 1.6,
            },
            Shape::Plane { size: [4.0, 4.0] },
            Shape::Ramp {
                size: [1.0, 1.0, 2.0],
            },
            Shape::Torus {
                radius: 0.5,
                tube: 0.15,
            },
        ]
    }
}

/// Accumulates triangles with per-vertex color resolved from height.
struct Builder {
    data: MeshData,
    bottom: f32,
    top: f32,
    color: [u8; 4],
    top_color: Option<[u8; 4]>,
    /// Identical (position, normal, color) vertices are shared: smaller
    /// blobs and fewer vertices for the console's T&L to transform.
    dedup: std::collections::HashMap<[u32; 7], u32>,
}

impl Builder {
    fn color_at(&self, y: f32) -> [u8; 4] {
        match self.top_color {
            None => self.color,
            Some(top) => {
                let span = self.top - self.bottom;
                let t = if span > 1e-6 {
                    ((y - self.bottom) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                std::array::from_fn(|i| {
                    (self.color[i] as f32 + (top[i] as f32 - self.color[i] as f32) * t).round()
                        as u8
                })
            }
        }
    }

    fn vertex(&mut self, p: [f32; 3], n: [f32; 3]) -> u32 {
        let c = self.color_at(p[1]);
        let n = normalize(n);
        // Quantize the key so float noise from different construction paths
        // (ring seams, poles) still merges.
        let q = |v: f32| (v * 8192.0).round() as i32 as u32;
        let key = [
            q(p[0]),
            q(p[1]),
            q(p[2]),
            q(n[0]),
            q(n[1]),
            q(n[2]),
            u32::from_le_bytes(c),
        ];
        if let Some(&i) = self.dedup.get(&key) {
            return i;
        }
        self.data.positions.push(p);
        self.data.normals.push(n);
        self.data.colors.push(c);
        let i = (self.data.positions.len() - 1) as u32;
        self.dedup.insert(key, i);
        i
    }

    /// Flat-shaded triangle (CCW seen from outside).
    fn tri(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
        let n = cross(sub(b, a), sub(c, a));
        let (ia, ib, ic) = (self.vertex(a, n), self.vertex(b, n), self.vertex(c, n));
        self.data.indices.extend([ia, ib, ic]);
    }

    /// Flat quad a-b-c-d (CCW).
    fn quad(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
        self.tri(a, b, c);
        self.tri(a, c, d);
    }

    /// Smooth triangle with explicit normals.
    fn tri_n(&mut self, v: [([f32; 3], [f32; 3]); 3]) {
        let i: [u32; 3] = std::array::from_fn(|k| self.vertex(v[k].0, v[k].1));
        self.data.indices.extend(i);
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-12 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

impl Primitive {
    pub fn load(path: &Path) -> Result<Primitive, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn from_ron(text: &str) -> Result<Primitive, String> {
        let p: Primitive =
            ron::from_str(text).map_err(|e| format!("primitive parse error: {e}"))?;
        p.validate()?;
        Ok(p)
    }

    pub fn to_ron(&self) -> String {
        let body = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .expect("primitive serialization cannot fail");
        format!(
            "// Trino placeholder model (\"fake object\"). Edit and save: it hot-reloads.\n\
             // Replace with real art by pointing the manifest `file` at a .glb —\n\
             // the logical path (and every reference to it) stays the same.\n{body}\n"
        )
    }

    fn validate(&self) -> Result<(), String> {
        let positive = |name: &str, v: f32| {
            if v > 0.0 && v.is_finite() {
                Ok(())
            } else {
                Err(format!(
                    "{} `{name}` must be > 0 (got {v})",
                    self.shape.kind()
                ))
            }
        };
        match &self.shape {
            Shape::Box { size } | Shape::Ramp { size } => {
                for (axis, v) in ["x", "y", "z"].iter().zip(size) {
                    positive(axis, *v)?;
                }
            }
            Shape::Plane { size } => {
                positive("x", size[0])?;
                positive("z", size[1])?;
            }
            Shape::Sphere { radius } => positive("radius", *radius)?,
            Shape::Cylinder { radius, height } | Shape::Cone { radius, height } => {
                positive("radius", *radius)?;
                positive("height", *height)?;
            }
            Shape::Capsule { radius, height } => {
                positive("radius", *radius)?;
                if *height < 2.0 * radius {
                    return Err(format!(
                        "capsule `height` ({height}) must be at least 2 × radius ({radius})"
                    ));
                }
            }
            Shape::Torus { radius, tube } => {
                positive("radius", *radius)?;
                positive("tube", *tube)?;
            }
        }
        Ok(())
    }

    /// Height extent `(bottom, top)` before the pivot shift.
    fn extent(&self) -> (f32, f32) {
        match &self.shape {
            Shape::Box { size } | Shape::Ramp { size } => (-size[1] * 0.5, size[1] * 0.5),
            Shape::Sphere { radius } => (-radius, *radius),
            Shape::Cylinder { height, .. }
            | Shape::Cone { height, .. }
            | Shape::Capsule { height, .. } => (-height * 0.5, height * 0.5),
            Shape::Plane { .. } => (0.0, 0.0),
            Shape::Torus { tube, .. } => (-tube, *tube),
        }
    }

    /// Generate the mesh.
    pub fn build(&self) -> MeshData {
        let (bottom, top) = self.extent();
        let mut b = Builder {
            data: MeshData::default(),
            bottom,
            top,
            color: self.color,
            top_color: self.top_color,
            dedup: std::collections::HashMap::new(),
        };
        let seg = self.segments.clamp(3, 48) as usize;
        let tau = std::f32::consts::TAU;
        let ring = |i: usize| {
            let a = i as f32 / seg as f32 * tau;
            (a.cos(), a.sin())
        };
        match &self.shape {
            Shape::Box { size } => {
                let [x, y, z] = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
                let p = |sx: f32, sy: f32, sz: f32| [sx * x, sy * y, sz * z];
                b.quad(
                    p(-1., -1., 1.),
                    p(1., -1., 1.),
                    p(1., 1., 1.),
                    p(-1., 1., 1.),
                ); // +Z
                b.quad(
                    p(1., -1., -1.),
                    p(-1., -1., -1.),
                    p(-1., 1., -1.),
                    p(1., 1., -1.),
                ); // -Z
                b.quad(
                    p(1., -1., 1.),
                    p(1., -1., -1.),
                    p(1., 1., -1.),
                    p(1., 1., 1.),
                ); // +X
                b.quad(
                    p(-1., -1., -1.),
                    p(-1., -1., 1.),
                    p(-1., 1., 1.),
                    p(-1., 1., -1.),
                ); // -X
                b.quad(
                    p(-1., 1., 1.),
                    p(1., 1., 1.),
                    p(1., 1., -1.),
                    p(-1., 1., -1.),
                ); // +Y
                b.quad(
                    p(-1., -1., -1.),
                    p(1., -1., -1.),
                    p(1., -1., 1.),
                    p(-1., -1., 1.),
                ); // -Y
            }
            Shape::Ramp { size } => {
                let [x, y, z] = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
                let (bl, br) = ([-x, -y, -z], [x, -y, -z]);
                let (fl, fr) = ([-x, -y, z], [x, -y, z]);
                let (tl, tr) = ([-x, y, z], [x, y, z]);
                b.quad(fl, fr, tr, tl); // tall back face (+Z)
                b.quad(bl, br, fr, fl); // bottom (-Y)
                b.quad(bl, tl, tr, br); // slope, facing up and -Z
                b.tri(bl, fl, tl); // left side
                b.tri(br, tr, fr); // right side
            }
            Shape::Plane { size } => {
                let (x, z) = (size[0] * 0.5, size[1] * 0.5);
                b.quad([-x, 0., z], [x, 0., z], [x, 0., -z], [-x, 0., -z]);
            }
            Shape::Cylinder { radius, height } | Shape::Cone { radius, height } => {
                let cone = matches!(self.shape, Shape::Cone { .. });
                let (y0, y1) = (-height * 0.5, height * 0.5);
                let top_r = if cone { 0.0 } else { *radius };
                // Slope of the side normal for cones.
                let ny = if cone { radius / height } else { 0.0 };
                for i in 0..seg {
                    let (c0, s0) = ring(i);
                    let (c1, s1) = ring(i + 1);
                    let p0 = [c0 * radius, y0, s0 * radius];
                    let p1 = [c1 * radius, y0, s1 * radius];
                    let q0 = [c0 * top_r, y1, s0 * top_r];
                    let q1 = [c1 * top_r, y1, s1 * top_r];
                    if self.smooth {
                        let n0 = [c0, ny, s0];
                        let n1 = [c1, ny, s1];
                        let (cm, sm) = ring(i);
                        let apex_n = [(cm + c1) * 0.5, ny, (sm + s1) * 0.5];
                        b.tri_n([(p0, n0), (q1, if cone { apex_n } else { n1 }), (p1, n1)]);
                        if !cone {
                            b.tri_n([(p0, n0), (q0, n0), (q1, n1)]);
                        }
                    } else {
                        b.tri(p0, q1, p1);
                        if !cone {
                            b.tri(p0, q0, q1);
                        }
                    }
                    // Caps.
                    b.tri([0., y0, 0.], p0, p1);
                    if !cone {
                        b.tri([0., y1, 0.], q1, q0);
                    }
                }
            }
            Shape::Sphere { radius } => {
                let rings = (seg / 2).max(2);
                self.lathe(&mut b, rings, |t| {
                    let phi = t * std::f32::consts::PI;
                    // (ring radius, y, normal xz scale, normal y)
                    (
                        phi.sin() * radius,
                        -phi.cos() * radius,
                        phi.sin(),
                        -phi.cos(),
                    )
                });
            }
            Shape::Capsule { radius, height } => {
                let half = height * 0.5 - radius;
                let rings = (seg / 2).max(2) | 1; // odd → an equator band
                self.lathe(&mut b, rings + 1, |t| {
                    // Two hemispheres split at t = 0.5, separated by `half`.
                    let phi = t * std::f32::consts::PI;
                    let y = -phi.cos() * radius + if t < 0.5 { -half } else { half };
                    (phi.sin() * radius, y, phi.sin(), -phi.cos())
                });
            }
            Shape::Torus { radius, tube } => {
                let tube_seg = (seg / 2).max(3);
                for i in 0..seg {
                    for k in 0..tube_seg {
                        let pt = |i: usize, k: usize| {
                            let (ca, sa) = ring(i);
                            let b = k as f32 / tube_seg as f32 * tau;
                            let (cb, sb) = (b.cos(), b.sin());
                            let p = [
                                (radius + tube * cb) * ca,
                                tube * sb,
                                (radius + tube * cb) * sa,
                            ];
                            let n = [cb * ca, sb, cb * sa];
                            (p, n)
                        };
                        let (a, bb, c, d) =
                            (pt(i, k), pt(i + 1, k), pt(i + 1, k + 1), pt(i, k + 1));
                        if self.smooth {
                            b.tri_n([a, c, bb]);
                            b.tri_n([a, d, c]);
                        } else {
                            b.tri(a.0, c.0, bb.0);
                            b.tri(a.0, d.0, c.0);
                        }
                    }
                }
            }
        }

        if self.pivot == Pivot::Bottom {
            for p in &mut b.data.positions {
                p[1] -= bottom;
            }
        }
        b.data
    }

    /// Surface of revolution around Y: `profile(t)` for t in 0..=1 gives
    /// (ring radius, y, normal xz factor, normal y).
    fn lathe(&self, b: &mut Builder, rings: usize, profile: impl Fn(f32) -> (f32, f32, f32, f32)) {
        let seg = self.segments.clamp(3, 48) as usize;
        let tau = std::f32::consts::TAU;
        let at = |i: usize, r: usize| {
            let (rad, y, nxz, ny) = profile(r as f32 / rings as f32);
            let a = i as f32 / seg as f32 * tau;
            let (c, s) = (a.cos(), a.sin());
            ([c * rad, y, s * rad], [c * nxz, ny, s * nxz])
        };
        for r in 0..rings {
            for i in 0..seg {
                let (p00, p10, p11, p01) = (at(i, r), at(i + 1, r), at(i + 1, r + 1), at(i, r + 1));
                if self.smooth {
                    b.tri_n([p00, p01, p11]);
                    b.tri_n([p00, p11, p10]);
                } else {
                    b.tri(p00.0, p01.0, p11.0);
                    b.tri(p00.0, p11.0, p10.0);
                }
            }
        }
        // Drop degenerate (zero-area) triangles produced at the poles.
        let mut keep = Vec::with_capacity(b.data.indices.len());
        for t in b.data.indices.as_chunks::<3>().0 {
            let [a, bb, c] = t.map(|i| b.data.positions[i as usize]);
            let n = cross(sub(bb, a), sub(c, a));
            if n[0] * n[0] + n[1] * n[1] + n[2] * n[2] > 1e-14 {
                keep.extend_from_slice(t);
            }
        }
        b.data.indices = keep;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::Mesh;

    fn all() -> Vec<Primitive> {
        Shape::all_defaults()
            .into_iter()
            .flat_map(|shape| {
                [true, false].map(|smooth| Primitive {
                    shape: shape.clone(),
                    smooth,
                    ..Primitive::default()
                })
            })
            .collect()
    }

    #[test]
    fn every_shape_bakes_to_a_valid_tmdl() {
        for p in all() {
            let blob = p
                .build()
                .encode()
                .unwrap_or_else(|e| panic!("{:?}: {e}", p.shape));
            let mesh = Mesh::from_tmdl(&blob).unwrap();
            assert!(
                mesh.index_count >= 3 && mesh.index_count.is_multiple_of(3),
                "{:?}",
                p.shape
            );
            assert!(mesh.bounds().is_some());
        }
    }

    #[test]
    fn closed_shapes_have_outward_winding() {
        // For a closed mesh centered on the origin, the triangle normal
        // (from winding) must point away from the center.
        for p in all() {
            if matches!(p.shape, Shape::Plane { .. } | Shape::Torus { .. }) {
                continue;
            }
            let d = p.build();
            for t in d.indices.as_chunks::<3>().0 {
                let [a, b, c] = t.map(|i| d.positions[i as usize]);
                let n = cross(sub(b, a), sub(c, a));
                let centroid = [
                    (a[0] + b[0] + c[0]) / 3.0,
                    (a[1] + b[1] + c[1]) / 3.0,
                    (a[2] + b[2] + c[2]) / 3.0,
                ];
                let dot = n[0] * centroid[0] + n[1] * centroid[1] + n[2] * centroid[2];
                assert!(dot > -1e-6, "{:?} has an inward triangle", p.shape);
            }
        }
    }

    #[test]
    fn vertices_are_shared() {
        // A smooth sphere shares each vertex among up to 6 triangles.
        let d = Primitive {
            shape: Shape::Sphere { radius: 1.0 },
            ..Primitive::default()
        }
        .build();
        assert!(
            d.indices.len() >= d.positions.len() * 4,
            "{} idx / {} verts",
            d.indices.len(),
            d.positions.len()
        );
        // A flat box: 4 per face (each quad's 2 triangles share 2 vertices).
        let b = Primitive::default().build();
        assert_eq!(b.positions.len(), 24);
        assert_eq!(b.indices.len(), 36);
    }

    #[test]
    fn plane_faces_up_and_bottom_pivot_sits_on_zero() {
        let plane = Primitive {
            shape: Shape::Plane { size: [2.0, 2.0] },
            ..Primitive::default()
        }
        .build();
        let t = &plane.indices[0..3];
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| plane.positions[i as usize]);
        assert!(cross(sub(b, a), sub(c, a))[1] > 0.0, "plane must face +Y");

        let cyl = Primitive {
            shape: Shape::Cylinder {
                radius: 0.5,
                height: 2.0,
            },
            pivot: Pivot::Bottom,
            ..Primitive::default()
        }
        .build();
        let min_y = cyl.positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        let max_y = cyl.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!(min_y.abs() < 1e-5 && (max_y - 2.0).abs() < 1e-5);
    }

    #[test]
    fn top_color_blends_by_height() {
        let d = Primitive {
            color: [0, 0, 0, 255],
            top_color: Some([200, 100, 0, 255]),
            ..Primitive::default()
        }
        .build();
        for (p, c) in d.positions.iter().zip(&d.colors) {
            let expect = if p[1] > 0.0 {
                [200, 100, 0, 255]
            } else {
                [0, 0, 0, 255]
            };
            assert_eq!(*c, expect);
        }
    }

    #[test]
    fn ron_round_trip_and_validation() {
        for p in all() {
            let back = Primitive::from_ron(&p.to_ron()).unwrap();
            assert_eq!(p, back);
        }
        let bad = "(shape: Sphere(radius: -1.0))";
        assert!(
            Primitive::from_ron(bad)
                .unwrap_err()
                .contains("must be > 0")
        );
        let min = "(shape: Box(size: (1.0, 2.0, 3.0)))";
        let p = Primitive::from_ron(min).unwrap();
        assert_eq!(p.color, default_color());
    }
}
