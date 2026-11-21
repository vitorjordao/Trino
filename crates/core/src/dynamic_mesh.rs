//! Dynamic meshes: geometry the game writes every frame
//! ([`Renderer::draw_mesh`](crate::Renderer::draw_mesh)).
//!
//! A baked model is fixed at bake time; a surface that follows the
//! simulation (a sheet of water running up a road, a wet patch drying, a
//! floor opening tile by tile) changes shape every frame. Drawn as many
//! model pieces it shows seams where the pieces meet and steps where their
//! colors differ. A [`DynamicMesh`] is one continuous mesh instead:
//! vertices (position, normal, color with alpha, optional UV) and
//! triangles over them, borrowed from the game for one call.
//!
//! The backends draw it as a model: the mesh is written as a TMDL blob
//! ([`DynamicMesh::write_tmdl`], the format [`crate::render3d::Mesh`]
//! reads) and goes through the very path a baked model with the same
//! vertices takes, so it gets the same T&L, lighting, translucency, water
//! and fog, and the same pixels. Nothing here allocates: the backend owns
//! the buffer it writes into.
//!
//! Targets: the PC and the Web draw them ([`crate::Caps::max_dynamic_vertices`]
//! is 65 536 there); the consoles' tier is 0 for now and the call draws
//! nothing there (the trait's default), as on the PC's console profiles
//! (counted in [`crate::FrameStats::dynamic_rejected`]).

use crate::math::{Color, Vec2, Vec3};
use crate::math3d::sqrt;
use crate::render::SpriteId;
use crate::render3d::TMDL_MAGIC;

/// Most vertices a dynamic mesh can have: TMDL indices are `u16`.
pub const MAX_DYNAMIC_VERTICES: usize = 65_536;

/// Most indices a dynamic mesh can have (65 536 triangles).
pub const MAX_DYNAMIC_INDICES: usize = 3 * 65_536;

/// One vertex of a [`DynamicMesh`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DynVertex {
    /// Model space (the draw's matrix places it).
    pub position: Vec3,
    /// Unit length for the lighting the baked models get (a zero normal
    /// lights like a surface facing away from the light).
    /// [`smooth_normals`] computes them from the triangles.
    pub normal: Vec3,
    /// Multiplied by the draw's tint, before lighting; alpha below 255
    /// makes the draw translucent (blended, sorted with the frame's other
    /// translucent draws).
    pub color: Color,
    /// Texture coordinates, read only when [`DynamicMesh::uvs`] is set.
    pub uv: Vec2,
}

impl DynVertex {
    /// A vertex with no texture coordinates.
    pub const fn new(position: Vec3, normal: Vec3, color: Color) -> DynVertex {
        DynVertex {
            position,
            normal,
            color,
            uv: Vec2::ZERO,
        }
    }
}

/// A mesh the game wrote this frame: [`Renderer::draw_mesh`](crate::Renderer::draw_mesh).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DynamicMesh<'a> {
    pub vertices: &'a [DynVertex],
    /// Three per triangle, counter-clockwise from the front (the baked
    /// models' winding).
    pub indices: &'a [u16],
    /// The vertices carry UVs (a TMDL `TXUV` section): the draw's
    /// [`ModelParams::texture`](crate::ModelParams::texture), or
    /// [`texture`](Self::texture), maps over them.
    pub uvs: bool,
    /// The default texture (TMDL `TXID`), when [`uvs`](Self::uvs) is set.
    pub texture: Option<SpriteId>,
}

/// Why a [`DynamicMesh`] can't be drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DynamicMeshError {
    /// More vertices than the target takes
    /// ([`crate::Caps::max_dynamic_vertices`]; 0 = none on this target).
    TooManyVertices { count: usize, max: usize },
    /// More than [`MAX_DYNAMIC_INDICES`] indices.
    TooManyIndices { count: usize },
    /// The index count is not a multiple of three.
    NotTriangles { count: usize },
    /// Index `at` names a vertex past the end.
    IndexOutOfRange { at: usize },
    /// Vertex `vertex` has a NaN or infinite position, normal or UV.
    NonFinite { vertex: usize },
    /// The output buffer is shorter than [`DynamicMesh::tmdl_len`].
    BufferTooSmall { need: usize },
}

impl<'a> DynamicMesh<'a> {
    /// An untextured mesh.
    pub const fn new(vertices: &'a [DynVertex], indices: &'a [u16]) -> DynamicMesh<'a> {
        DynamicMesh {
            vertices,
            indices,
            uvs: false,
            texture: None,
        }
    }

    /// The same mesh with UVs and a default texture.
    pub const fn textured(self, texture: SpriteId) -> DynamicMesh<'a> {
        DynamicMesh {
            uvs: true,
            texture: Some(texture),
            ..self
        }
    }

    /// No triangles: nothing to draw (not an error).
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty() || self.vertices.is_empty()
    }

    /// Whether the mesh can be drawn by a target that takes up to
    /// `max_vertices` vertices: sizes, triangles, indices in range, every
    /// number finite. The first problem found.
    pub fn check(&self, max_vertices: usize) -> Result<(), DynamicMeshError> {
        let max = max_vertices.min(MAX_DYNAMIC_VERTICES);
        let v = self.vertices.len();
        if v > max {
            return Err(DynamicMeshError::TooManyVertices { count: v, max });
        }
        let i = self.indices.len();
        if i > MAX_DYNAMIC_INDICES {
            return Err(DynamicMeshError::TooManyIndices { count: i });
        }
        if !i.is_multiple_of(3) {
            return Err(DynamicMeshError::NotTriangles { count: i });
        }
        if let Some(at) = self.indices.iter().position(|&k| k as usize >= v) {
            return Err(DynamicMeshError::IndexOutOfRange { at });
        }
        let finite = |p: Vec3| p.x.is_finite() && p.y.is_finite() && p.z.is_finite();
        let uvs = self.uvs;
        if let Some(vertex) = self.vertices.iter().position(|d| {
            !finite(d.position)
                || !finite(d.normal)
                || (uvs && !(d.uv.x.is_finite() && d.uv.y.is_finite()))
        }) {
            return Err(DynamicMeshError::NonFinite { vertex });
        }
        Ok(())
    }

    /// The bounding sphere the blob carries (`BNDS`): the center of the
    /// box around the vertices and the farthest vertex from it.
    pub fn bounds(&self) -> (Vec3, f32) {
        let Some(first) = self.vertices.first() else {
            return (Vec3::ZERO, 0.0);
        };
        let (mut lo, mut hi) = (first.position, first.position);
        for d in self.vertices {
            let p = d.position;
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        let c = (lo + hi) * 0.5;
        let r2 = self
            .vertices
            .iter()
            .map(|d| {
                let o = d.position - c;
                o.dot(o)
            })
            .fold(0.0f32, f32::max);
        // A hair over, as the model bake does: the T&L culls a sphere that
        // only touches the view.
        (c, sqrt(r2) * 1.001 + 1e-4)
    }

    /// Bytes of the TMDL blob [`write_tmdl`](Self::write_tmdl) writes
    /// (header, vertices, indices, `BNDS`, and `TXUV`/`TXID` when set).
    /// Checked arithmetic: the consoles' `usize` is 32 bits.
    pub fn tmdl_len(&self) -> Option<usize> {
        let v = self.vertices.len();
        let mut n = v.checked_mul(28)?.checked_add(12)?;
        n = n.checked_add(self.indices.len().checked_mul(2)?)?;
        n = n.checked_add(8 + 16)?;
        if self.uvs {
            n = n.checked_add(v.checked_mul(8)?.checked_add(8)?)?;
            if self.texture.is_some() {
                n = n.checked_add(8 + 4)?;
            }
        }
        Some(n)
    }

    /// Write the mesh as a TMDL blob into `out` (at least
    /// [`tmdl_len`](Self::tmdl_len) bytes) after [`check`](Self::check)ing
    /// it against the format's limits; returns the bytes written.
    /// [`crate::render3d::Mesh::from_tmdl`] reads it back as the mesh a
    /// baked model with these vertices, indices and bounds is.
    pub fn write_tmdl(&self, out: &mut [u8]) -> Result<usize, DynamicMeshError> {
        self.check(MAX_DYNAMIC_VERTICES)?;
        let need = self.tmdl_len().ok_or(DynamicMeshError::TooManyIndices {
            count: self.indices.len(),
        })?;
        if out.len() < need {
            return Err(DynamicMeshError::BufferTooSmall { need });
        }
        let mut w = Writer { out, at: 0 };
        w.bytes(TMDL_MAGIC);
        w.u32(self.vertices.len() as u32);
        w.u32(self.indices.len() as u32);
        for d in self.vertices {
            w.vec3(d.position);
        }
        for d in self.vertices {
            w.vec3(d.normal);
        }
        for d in self.vertices {
            let c = d.color;
            w.bytes(&[c.r, c.g, c.b, c.a]);
        }
        for &k in self.indices {
            w.bytes(&k.to_le_bytes());
        }
        let (c, r) = self.bounds();
        w.bytes(b"BNDS");
        w.u32(16);
        w.vec3(c);
        w.f32(r);
        if self.uvs {
            w.bytes(b"TXUV");
            w.u32(self.vertices.len() as u32 * 8);
            for d in self.vertices {
                w.f32(d.uv.x);
                w.f32(d.uv.y);
            }
            if let Some(t) = self.texture {
                w.bytes(b"TXID");
                w.u32(4);
                w.u32(t.0);
            }
        }
        Ok(w.at)
    }
}

struct Writer<'b> {
    out: &'b mut [u8],
    at: usize,
}

impl Writer<'_> {
    fn bytes(&mut self, b: &[u8]) {
        self.out[self.at..self.at + b.len()].copy_from_slice(b);
        self.at += b.len();
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.bytes(&v.to_le_bytes());
    }
    fn vec3(&mut self, v: Vec3) {
        self.f32(v.x);
        self.f32(v.y);
        self.f32(v.z);
    }
}

/// Smooth normals from the triangles: each vertex gets the sum of its
/// triangles' face normals weighted by their area, normalized (a vertex
/// shared by two strips gets one normal, so the light has no seam there).
/// A vertex in no triangle, or only in degenerate ones, gets +Y. Indices
/// past the vertices are skipped.
pub fn smooth_normals(vertices: &mut [DynVertex], indices: &[u16]) {
    for v in vertices.iter_mut() {
        v.normal = Vec3::ZERO;
    }
    let n = vertices.len();
    for t in indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(usize::from);
        if a >= n || b >= n || c >= n {
            continue;
        }
        let (pa, pb, pc) = (
            vertices[a].position,
            vertices[b].position,
            vertices[c].position,
        );
        // The cross product's length is twice the area: area-weighted.
        let f = (pb - pa).cross(pc - pa);
        if !(f.x.is_finite() && f.y.is_finite() && f.z.is_finite()) {
            continue;
        }
        for k in [a, b, c] {
            vertices[k].normal += f;
        }
    }
    for v in vertices.iter_mut() {
        let u = v.normal.normalized();
        v.normal = if u == Vec3::ZERO {
            Vec3::new(0.0, 1.0, 0.0)
        } else {
            u
        };
    }
}
