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
//! Limits (v1, on purpose): no z-buffer (per-mesh painter's sort), no
//! textures on 3D geometry (vertex colors only), no near-plane clipping
//! (triangles crossing the near plane are dropped).

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
    /// View-space depth of the triangle center (larger = farther).
    pub depth: f32,
}

const NEAR: f32 = 0.05;

/// Transform, light, project, cull and depth-sort `mesh` into `out`.
/// Returns how many triangles were written (front-to-back callers draw the
/// slice in order: it is sorted far-to-near for painter's rendering).
/// Triangles beyond `out.len()` are dropped — size the scratch for the mesh.
pub fn tessellate(
    mesh: &Mesh,
    model: &Mat34,
    camera: &Camera3,
    light: &Light,
    screen: Vec2,
    out: &mut [ScreenTri],
) -> usize {
    let view = camera.view();
    let mv = view.mul(model);
    let ldir = light.dir.normalized();
    // fov -> focal length in pixels (vertical).
    let half_fov = camera.fov_y * 0.5;
    let focal = crate::math3d::cos(half_fov) / crate::math3d::sin(half_fov) * screen.y * 0.5;
    let center = screen * 0.5;

    let mut count = 0usize;
    let tri_count = mesh.index_count / 3;
    for t in 0..tri_count {
        if count >= out.len() {
            break;
        }
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
        // No near-plane clipping in v1: drop the whole triangle.
        if vs[0].z <= NEAR || vs[1].z <= NEAR || vs[2].z <= NEAR {
            continue;
        }
        let project = |v: Vec3| {
            Vec2::new(
                center.x + v.x * focal / v.z,
                center.y - v.y * focal / v.z, // world Y-up -> screen Y-down
            )
        };
        let pts = [project(vs[0]), project(vs[1]), project(vs[2])];
        // Backface cull: front faces are CCW in world space, which lands as
        // positive signed area in Y-down screen space.
        let area = (pts[1].x - pts[0].x) * (pts[2].y - pts[0].y)
            - (pts[2].x - pts[0].x) * (pts[1].y - pts[0].y);
        if area <= 0.0 {
            continue;
        }
        // Gouraud: per-vertex intensity from the world-space normal.
        let shade = |i: usize| {
            let n = model.transform_dir(mesh.normal(i)).normalized();
            let diffuse = -n.dot(ldir);
            let diffuse = if diffuse > 0.0 { diffuse } else { 0.0 };
            let intensity = light.ambient + (1.0 - light.ambient) * diffuse;
            let c = mesh.color(i);
            let mul = |v: u8| (v as f32 * intensity) as u8;
            Color::rgba(mul(c.r), mul(c.g), mul(c.b), c.a)
        };
        out[count] = ScreenTri {
            pts,
            colors: [shade(i0), shade(i1), shade(i2)],
            depth: (vs[0].z + vs[1].z + vs[2].z) * (1.0 / 3.0),
        };
        count += 1;
    }

    // Painter's sort: farthest first. Insertion sort — no alloc, and the
    // counts are small by design (Caps budgets).
    for i in 1..count {
        let key = out[i];
        let mut j = i;
        while j > 0 && out[j - 1].depth < key.depth {
            out[j] = out[j - 1];
            j -= 1;
        }
        out[j] = key;
    }
    count
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

    #[test]
    fn cube_in_front_produces_sorted_visible_tris() {
        let blob = cube_tmdl();
        let mesh = Mesh::from_tmdl(&blob).unwrap();
        let camera = Camera3::default();
        let mut out = [ScreenTri {
            pts: [Vec2::ZERO; 3],
            colors: [Color::WHITE; 3],
            depth: 0.0,
        }; 64];
        let n = tessellate(
            &mesh,
            &Mat34::IDENTITY,
            &camera,
            &DEFAULT_LIGHT,
            Vec2::new(320.0, 240.0),
            &mut out,
        );
        // A cube facing the camera: between 1 and 3 visible faces = 2..6 tris.
        assert!((2..=6).contains(&n), "visible tris: {n}");
        // Painter's order: depth non-increasing.
        for w in out[..n].windows(2) {
            assert!(w[0].depth >= w[1].depth);
        }
        // Everything projects on-screen for this setup.
        for tri in &out[..n] {
            for p in tri.pts {
                assert!((0.0..320.0).contains(&p.x) && (0.0..240.0).contains(&p.y));
            }
        }
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
        let mut out = [ScreenTri {
            pts: [Vec2::ZERO; 3],
            colors: [Color::WHITE; 3],
            depth: 0.0,
        }; 64];
        let n = tessellate(
            &mesh,
            &Mat34::IDENTITY,
            &camera,
            &DEFAULT_LIGHT,
            Vec2::new(320.0, 240.0),
            &mut out,
        );
        assert_eq!(n, 0);
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
