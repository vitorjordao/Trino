//! Software transform & lighting: the engine's whole 3D pipeline (Fase 7).
//!
//! Design (see `docs/adr/0003-software-tnl-3d.md`): the N64 is the ceiling,
//! and for the mesh budgets Trino targets, transforming vertices on the CPU
//! and handing the backends nothing but **screen-space colored triangles**
//! beats integrating three different hardware T&L stacks. Backends only
//! rasterize (`rdpq_triangle` / `C2D_DrawTriangle` / a wgpu vertex-color
//! pipeline); everything above that — model/view transform, perspective,
//! gouraud lighting, backface cull, painter's sort — lives here, in pure
//! deterministic f32, identical on every target.
//!
//! Limits (v1, on purpose): no z-buffer (painter's sort), no textures on 3D
//! geometry (vertex colors only).
//!
//! Triangles are clipped against the near plane and a guard-band frustum
//! (1.5x the screen), so geometry that crosses the near plane stays visible
//! and every projected coordinate stays bounded — important for the N64 RDP,
//! whose edge coefficients are fixed-point and overflow on huge offscreen
//! coordinates.
//!
//! Painter correctness for a z-bufferless renderer hinges on two things
//! done here (both found play-testing a 3D game — huge floor triangles
//! painted over the player, doors vanished behind wall quads):
//!
//! - Edges spanning more than [`DEPTH_SPLIT`] units of view depth are
//!   recursively bisected, so a 40-unit floor becomes depth-uniform strips
//!   instead of one triangle whose single sort key misrepresents most of
//!   its surface. The split rule depends only on the edge's endpoints, so
//!   neighboring triangles split shared edges at identical points — no
//!   T-junction cracks.
//! - The sort key ([`ScreenTri::depth`]) is the triangle's **farthest**
//!   vertex, not its centroid: a surface that extends behind an object
//!   standing on it always draws first.
//!
//! Cross-mesh ordering: backends collect the triangles of consecutive
//! `draw_model` calls into a batch and depth-sort the whole batch before
//! rasterizing (flushed on sprite draws, camera changes and `end_frame`),
//! so overlapping models layer correctly without the game sorting draws.

use crate::math::{Color, Vec2, Vec3};
use crate::math3d::Mat34;

/// Mesh blob format ("TMDL", little-endian):
/// `u32 vertex_count, u32 index_count`, then `positions f32*3*v`,
/// `normals f32*3*v`, `colors u8*4*v`, `indices u16*i`.
pub const TMDL_MAGIC: &[u8; 4] = b"TMDL";

/// A parsed (borrowed) mesh — zero-copy over the baked blob.
#[derive(Clone, Copy, Debug)]
pub struct Mesh<'a> {
    positions: &'a [u8],
    normals: &'a [u8],
    colors: &'a [u8],
    indices: &'a [u8],
    pub vertex_count: usize,
    pub index_count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MeshError {
    BadMagic,
    Truncated,
}

impl<'a> Mesh<'a> {
    pub fn from_tmdl(bytes: &'a [u8]) -> Result<Self, MeshError> {
        if bytes.len() < 12 || &bytes[0..4] != TMDL_MAGIC {
            return Err(MeshError::BadMagic);
        }
        let v = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let i = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let pos_len = v * 12;
        let nrm_len = v * 12;
        let col_len = v * 4;
        let idx_len = i * 2;
        if bytes.len() < 12 + pos_len + nrm_len + col_len + idx_len {
            return Err(MeshError::Truncated);
        }
        let pos_at = 12;
        let nrm_at = pos_at + pos_len;
        let col_at = nrm_at + nrm_len;
        let idx_at = col_at + col_len;
        Ok(Mesh {
            positions: &bytes[pos_at..pos_at + pos_len],
            normals: &bytes[nrm_at..nrm_at + nrm_len],
            colors: &bytes[col_at..col_at + col_len],
            indices: &bytes[idx_at..idx_at + idx_len],
            vertex_count: v,
            index_count: i,
        })
    }

    #[inline]
    fn read_vec3(data: &[u8], i: usize) -> Vec3 {
        let at = i * 12;
        Vec3::new(
            f32::from_le_bytes(data[at..at + 4].try_into().unwrap()),
            f32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()),
            f32::from_le_bytes(data[at + 8..at + 12].try_into().unwrap()),
        )
    }

    #[inline]
    pub fn position(&self, i: usize) -> Vec3 {
        Self::read_vec3(self.positions, i)
    }

    #[inline]
    pub fn normal(&self, i: usize) -> Vec3 {
        Self::read_vec3(self.normals, i)
    }

    #[inline]
    pub fn color(&self, i: usize) -> Color {
        let at = i * 4;
        Color::rgba(
            self.colors[at],
            self.colors[at + 1],
            self.colors[at + 2],
            self.colors[at + 3],
        )
    }

    #[inline]
    pub fn index(&self, i: usize) -> u16 {
        u16::from_le_bytes(self.indices[i * 2..i * 2 + 2].try_into().unwrap())
    }
}

/// Perspective camera. World space is Y-up, right-handed; the projection
/// flips into the engine's Y-down screen space.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Camera3 {
    pub eye: Vec3,
    pub target: Vec3,
    /// Vertical field of view, radians.
    pub fov_y: f32,
}

impl Default for Camera3 {
    fn default() -> Self {
        Camera3 {
            eye: Vec3::new(0.0, 0.0, -5.0),
            target: Vec3::ZERO,
            fov_y: 60.0 * core::f32::consts::PI / 180.0,
        }
    }
}

impl Camera3 {
    pub fn view(&self) -> Mat34 {
        Mat34::look_at(self.eye, self.target, Vec3::new(0.0, 1.0, 0.0))
    }
}

/// Directional light + ambient floor.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Light {
    /// Direction the light travels (world space); normalized by the caller
    /// or in [`tessellate`].
    pub dir: Vec3,
    /// 0..1 base intensity every face receives.
    pub ambient: f32,
}

/// The engine's default key light: from above-left-behind the camera.
pub const DEFAULT_LIGHT: Light = Light {
    dir: Vec3::new(0.4, -0.8, 0.45),
    ambient: 0.35,
};

/// One screen-space triangle ready for a backend rasterizer.
#[derive(Clone, Copy, Debug)]
pub struct ScreenTri {
    pub pts: [Vec2; 3],
    pub colors: [Color; 3],
    /// Painter sort key: view-space depth of the triangle's **farthest**
    /// vertex (larger = farther). Farthest-vertex keys make a surface that
    /// extends behind an object draw before the object standing on it —
    /// centroid keys painted floors over players and walls over doors.
    pub depth: f32,
}

const NEAR: f32 = 0.05;
/// Guard band: side planes sit at 1.5x the screen so clipped coordinates stay
/// bounded (RDP fixed-point safety) without visible clipping at the borders.
const GUARD: f32 = 1.5;
/// A triangle clipped by up to 5 planes gains at most one vertex per plane.
const MAX_POLY: usize = 8;
/// Edges spanning more than this much view depth are bisected before
/// projection: painter sorting is only as good as how depth-uniform each
/// triangle is. The rule looks at one edge at a time, so neighbors split
/// shared edges identically (no cracks).
pub const DEPTH_SPLIT: f32 = 3.0;
/// Subdivision work stack; overflow emits the triangle unsplit (graceful).
const SPLIT_STACK: usize = 48;

/// Linear blend of two already-lit vertex colors (gouraud interpolation for
/// clip-generated vertices).
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color::rgba(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), mix(a.a, b.a))
}

/// Clip a view-space polygon against `dist(v) >= 0` (Sutherland-Hodgman).
/// Returns the new vertex count.
fn clip_plane(
    verts: &[(Vec3, Color); MAX_POLY],
    n_in: usize,
    out: &mut [(Vec3, Color); MAX_POLY],
    dist: impl Fn(Vec3) -> f32,
) -> usize {
    let mut n_out = 0;
    for i in 0..n_in {
        let (a, ca) = verts[i];
        let (b, cb) = verts[(i + 1) % n_in];
        let (da, db) = (dist(a), dist(b));
        if da >= 0.0 {
            if n_out == MAX_POLY {
                return n_out;
            }
            out[n_out] = (a, ca);
            n_out += 1;
        }
        if (da >= 0.0) != (db >= 0.0) {
            if n_out == MAX_POLY {
                return n_out;
            }
            let t = da / (da - db);
            out[n_out] = (a + (b - a) * t, lerp_color(ca, cb, t));
            n_out += 1;
        }
    }
    n_out
}

/// Transform, light, clip, depth-subdivide, project and cull `mesh`,
/// handing each visible triangle to `emit`. Triangles are **not** sorted
/// here: backends batch the triangles of consecutive `draw_model` calls and
/// depth-sort the whole batch (see the module docs). The callback API exists
/// because clipping + subdivision have no static output bound — backends
/// push into a growable buffer.
pub fn tessellate(
    mesh: &Mesh,
    model: &Mat34,
    camera: &Camera3,
    light: &Light,
    tint: Color,
    screen: Vec2,
    emit: &mut dyn FnMut(ScreenTri),
) {
    let view = camera.view();
    let mv = view.mul(model);
    let ldir = light.dir.normalized();
    // fov -> focal length in pixels (vertical).
    let half_fov = camera.fov_y * 0.5;
    let focal = crate::math3d::cos(half_fov) / crate::math3d::sin(half_fov) * screen.y * 0.5;
    let center = screen * 0.5;
    // Guard-band frustum half-extents per unit of view depth.
    let lim_x = center.x * GUARD / focal;
    let lim_y = center.y * GUARD / focal;

    let tri_count = mesh.index_count / 3;
    for t in 0..tri_count {
        let (i0, i1, i2) = (
            mesh.index(t * 3) as usize,
            mesh.index(t * 3 + 1) as usize,
            mesh.index(t * 3 + 2) as usize,
        );
        let vs = [
            mv.transform_point(mesh.position(i0)),
            mv.transform_point(mesh.position(i1)),
            mv.transform_point(mesh.position(i2)),
        ];
        // Entirely behind the near plane: gone.
        if vs[0].z <= NEAR && vs[1].z <= NEAR && vs[2].z <= NEAR {
            continue;
        }
        // Gouraud: per-vertex intensity from the world-space normal, with
        // the per-draw tint multiplied in before lighting.
        let shade = |i: usize| {
            let n = model.transform_dir(mesh.normal(i)).normalized();
            let diffuse = -n.dot(ldir);
            let diffuse = if diffuse > 0.0 { diffuse } else { 0.0 };
            let intensity = light.ambient + (1.0 - light.ambient) * diffuse;
            let c = mesh.color(i);
            let mul = |v: u8, t: u8| (v as f32 * (t as f32 / 255.0) * intensity) as u8;
            Color::rgba(
                mul(c.r, tint.r),
                mul(c.g, tint.g),
                mul(c.b, tint.b),
                (c.a as f32 * (tint.a as f32 / 255.0)) as u8,
            )
        };

        let inside = |v: Vec3| {
            v.z > NEAR
                && v.x >= -lim_x * v.z
                && v.x <= lim_x * v.z
                && v.y >= -lim_y * v.z
                && v.y <= lim_y * v.z
        };
        let mut poly_a = [(Vec3::ZERO, Color::WHITE); MAX_POLY];
        let mut poly_b = [(Vec3::ZERO, Color::WHITE); MAX_POLY];
        poly_a[0] = (vs[0], shade(i0));
        poly_a[1] = (vs[1], shade(i1));
        poly_a[2] = (vs[2], shade(i2));
        let mut n_poly = 3;
        if !(inside(vs[0]) && inside(vs[1]) && inside(vs[2])) {
            // Clip against near plane + the 4 guard-band side planes.
            n_poly = clip_plane(&poly_a, n_poly, &mut poly_b, |v| v.z - NEAR);
            n_poly = clip_plane(&poly_b, n_poly, &mut poly_a, |v| lim_x * v.z - v.x);
            n_poly = clip_plane(&poly_a, n_poly, &mut poly_b, |v| lim_x * v.z + v.x);
            n_poly = clip_plane(&poly_b, n_poly, &mut poly_a, |v| lim_y * v.z - v.y);
            n_poly = clip_plane(&poly_a, n_poly, &mut poly_b, |v| lim_y * v.z + v.y);
            poly_a = poly_b;
            if n_poly < 3 {
                continue;
            }
        }

        let project = |v: Vec3| {
            Vec2::new(
                center.x + v.x * focal / v.z,
                center.y - v.y * focal / v.z, // world Y-up -> screen Y-down
            )
        };
        // Fan-triangulate the clipped polygon, then depth-subdivide each
        // fan triangle (DFS with a fixed stack): any edge spanning more
        // than DEPTH_SPLIT of view depth is bisected, recursively, so every
        // emitted triangle is depth-uniform enough for painter sorting.
        for k in 1..n_poly - 1 {
            let mut stack = [[(Vec3::ZERO, Color::WHITE); 3]; SPLIT_STACK];
            stack[0] = [poly_a[0], poly_a[k], poly_a[k + 1]];
            let mut top = 1usize;
            while top > 0 {
                top -= 1;
                let tri = stack[top];
                // Worst depth-spanning edge (cyclic: 0-1, 1-2, 2-0).
                let span = |a: usize, b: usize| {
                    let d = tri[a].0.z - tri[b].0.z;
                    if d < 0.0 { -d } else { d }
                };
                let (s01, s12, s20) = (span(0, 1), span(1, 2), span(2, 0));
                let worst = if s01 >= s12 && s01 >= s20 {
                    (s01, 0)
                } else if s12 >= s20 {
                    (s12, 1)
                } else {
                    (s20, 2)
                };
                if worst.0 > DEPTH_SPLIT && top + 2 <= SPLIT_STACK {
                    // Rotate so the split edge is (a, b); bisect keeping the
                    // CCW winding: (a, m, c) + (m, b, c).
                    let (a, b, c) = match worst.1 {
                        0 => (tri[0], tri[1], tri[2]),
                        1 => (tri[1], tri[2], tri[0]),
                        _ => (tri[2], tri[0], tri[1]),
                    };
                    let mid = ((a.0 + b.0) * 0.5, lerp_color(a.1, b.1, 0.5));
                    stack[top] = [a, mid, c];
                    stack[top + 1] = [mid, b, c];
                    top += 2;
                    continue;
                }
                let [(v0, c0), (v1, c1), (v2, c2)] = tri;
                let pts = [project(v0), project(v1), project(v2)];
                // Backface cull: front faces are CCW in world space, which
                // lands as positive signed area in Y-down screen space.
                let area = (pts[1].x - pts[0].x) * (pts[2].y - pts[0].y)
                    - (pts[2].x - pts[0].x) * (pts[1].y - pts[0].y);
                if area <= 0.0 {
                    continue;
                }
                let depth = v0.z.max(v1.z).max(v2.z);
                emit(ScreenTri {
                    pts,
                    colors: [c0, c1, c2],
                    depth,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit cube TMDL built in memory (24 verts, 36 indices, per-face
    /// normals/colors) — mirrors what the asset pipeline bakes.
    pub fn cube_tmdl() -> std::vec::Vec<u8> {
        // (normal, color) per face; 4 verts per face.
        let faces: [(Vec3, [u8; 4]); 6] = [
            (Vec3::new(0.0, 0.0, -1.0), [230, 80, 80, 255]),
            (Vec3::new(0.0, 0.0, 1.0), [80, 230, 80, 255]),
            (Vec3::new(-1.0, 0.0, 0.0), [80, 80, 230, 255]),
            (Vec3::new(1.0, 0.0, 0.0), [230, 230, 80, 255]),
            (Vec3::new(0.0, -1.0, 0.0), [230, 80, 230, 255]),
            (Vec3::new(0.0, 1.0, 0.0), [80, 230, 230, 255]),
        ];
        let mut positions: std::vec::Vec<f32> = std::vec::Vec::new();
        let mut normals: std::vec::Vec<f32> = std::vec::Vec::new();
        let mut colors: std::vec::Vec<u8> = std::vec::Vec::new();
        let mut indices: std::vec::Vec<u16> = std::vec::Vec::new();
        for (f, (n, c)) in faces.iter().enumerate() {
            // Build the face quad from the normal's basis.
            let u = if n.y.abs() > 0.9 {
                Vec3::new(1.0, 0.0, 0.0)
            } else {
                Vec3::new(0.0, 1.0, 0.0).cross(*n).normalized()
            };
            let v = n.cross(u);
            let base = (f * 4) as u16;
            for (su, sv) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                let p = *n * 0.5 + u * su + v * sv;
                positions.extend_from_slice(&[p.x, p.y, p.z]);
                normals.extend_from_slice(&[n.x, n.y, n.z]);
                colors.extend_from_slice(c);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let mut blob = std::vec::Vec::new();
        blob.extend_from_slice(TMDL_MAGIC);
        blob.extend_from_slice(&(positions.len() as u32 / 3).to_le_bytes());
        blob.extend_from_slice(&(indices.len() as u32).to_le_bytes());
        for p in &positions {
            blob.extend_from_slice(&p.to_le_bytes());
        }
        for n in &normals {
            blob.extend_from_slice(&n.to_le_bytes());
        }
        blob.extend_from_slice(&colors);
        for i in &indices {
            blob.extend_from_slice(&i.to_le_bytes());
        }
        blob
    }

    #[test]
    fn tmdl_roundtrip() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        assert_eq!(mesh.vertex_count, 24);
        assert_eq!(mesh.index_count, 36);
        assert_eq!(mesh.color(0), Color::rgba(230, 80, 80, 255));
        assert!((mesh.normal(4).z - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tmdl_rejects_garbage() {
        assert!(matches!(Mesh::from_tmdl(b"nope"), Err(MeshError::BadMagic)));
        let mut blob = cube_tmdl();
        blob.truncate(40);
        assert!(matches!(Mesh::from_tmdl(&blob), Err(MeshError::Truncated)));
    }

    /// Roda o tessellate coletando os triângulos emitidos.
    fn collect(
        mesh: &Mesh,
        model: &Mat34,
        camera: &Camera3,
        tint: Color,
    ) -> std::vec::Vec<ScreenTri> {
        let mut v = std::vec::Vec::new();
        tessellate(
            mesh,
            model,
            camera,
            &DEFAULT_LIGHT,
            tint,
            Vec2::new(320.0, 240.0),
            &mut |t| v.push(t),
        );
        v
    }

    #[test]
    fn cube_in_front_produces_visible_tris() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let out = collect(&mesh, &Mat34::IDENTITY, &Camera3::default(), Color::WHITE);
        // A cube facing the camera: between 1 and 3 visible faces = 2..6
        // tris (the unit cube's edges never trip the depth subdivision).
        assert!((2..=6).contains(&out.len()), "visible tris: {}", out.len());
        for tri in &out {
            for p in tri.pts {
                assert!((0.0..320.0).contains(&p.x) && (0.0..240.0).contains(&p.y));
            }
        }
    }

    #[test]
    fn ground_crossing_the_near_plane_is_clipped_not_dropped() {
        // A big flat "ground" passing under the camera used to vanish
        // entirely (any vertex behind the near plane dropped the triangle).
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        // 40x1x40 slab whose top face is just below the camera and extends
        // far behind it.
        let model = Mat34::from_rotation_scale_translation(
            Vec3::ZERO,
            Vec3::new(40.0, 1.0, 40.0),
            Vec3::new(0.0, -1.5, 0.0),
        );
        let camera = Camera3 {
            eye: Vec3::new(0.0, 0.0, -5.0),
            target: Vec3::ZERO,
            ..Default::default()
        };
        let out = collect(&mesh, &model, &camera, Color::WHITE);
        assert!(!out.is_empty(), "near-crossing ground vanished");
        // Depth subdivision: the receding floor must arrive as depth-thin
        // strips, not one giant triangle pair.
        assert!(out.len() > 6, "floor not subdivided: {} tris", out.len());
        // Guard band keeps every projected coordinate bounded (the N64 RDP
        // works in fixed point; huge offscreen coordinates overflow it).
        for tri in &out {
            for p in tri.pts {
                assert!(
                    p.x.abs() <= 320.0 * 2.0 && p.y.abs() <= 240.0 * 2.0,
                    "unbounded coord {p:?}"
                );
            }
        }
    }

    #[test]
    fn farther_mesh_sorts_behind_a_nearer_one() {
        // depth = farthest vertex: every triangle of a cube 6 units behind
        // another must carry a strictly larger sort key.
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let camera = Camera3::default();
        let near = collect(&mesh, &Mat34::IDENTITY, &camera, Color::WHITE);
        let far_model =
            Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, Vec3::new(0.0, 0.0, 6.0));
        let far = collect(&mesh, &far_model, &camera, Color::WHITE);
        assert!(!near.is_empty() && !far.is_empty());
        let near_max = near.iter().map(|t| t.depth).fold(0.0f32, f32::max);
        let far_min = far.iter().map(|t| t.depth).fold(f32::MAX, f32::min);
        assert!(
            near_max < far_min,
            "near max {near_max} vs far min {far_min}"
        );
    }

    #[test]
    fn behind_camera_is_dropped() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let camera = Camera3 {
            eye: Vec3::new(0.0, 0.0, -5.0),
            target: Vec3::new(0.0, 0.0, -10.0),
            ..Default::default()
        };
        let out = collect(&mesh, &Mat34::IDENTITY, &camera, Color::WHITE);
        assert!(out.is_empty());
    }

    #[test]
    fn tint_multiplies_vertex_colors() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let camera = Camera3::default();
        let white = collect(&mesh, &Mat34::IDENTITY, &camera, Color::WHITE);
        let dark = collect(&mesh, &Mat34::IDENTITY, &camera, Color::rgb(128, 128, 128));
        assert_eq!(white.len(), dark.len());
        assert!(!white.is_empty());
        // Every emitted channel is (rounding aside) halved by the 50% tint.
        for (w, d) in white.iter().zip(dark.iter()) {
            for (cw, cd) in w.colors.iter().zip(d.colors.iter()) {
                assert!((cd.r as i32 - cw.r as i32 / 2).abs() <= 2);
                assert!((cd.g as i32 - cw.g as i32 / 2).abs() <= 2);
                assert!((cd.b as i32 - cw.b as i32 / 2).abs() <= 2);
                assert_eq!(cd.a, cw.a, "alpha tinted only by tint.a");
            }
        }
    }

    #[test]
    fn lighting_darkens_faces_away_from_the_light() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        // Light straight down: the top face (+Y) is lit, the bottom is
        // ambient-only.
        let light = Light {
            dir: Vec3::new(0.0, -1.0, 0.0),
            ambient: 0.2,
        };
        let model = Mat34::IDENTITY;
        // Top face verts are indices 20..24 (face 5), bottom 16..20.
        let shade_of = |i: usize| {
            let n = model.transform_dir(mesh.normal(i)).normalized();
            let d = (-n.dot(light.dir.normalized())).max(0.0);
            light.ambient + (1.0 - light.ambient) * d
        };
        assert!(shade_of(20) > 0.9);
        assert!(shade_of(16) < 0.3);
    }
}
