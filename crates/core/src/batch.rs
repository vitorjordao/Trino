//! Static batching: many static, opaque, untextured meshes merged into one
//! TMDL blob, so a scene of props costs a few draws instead of hundreds.
//!
//! On the N64 every draw carries a fixed cost that dwarfs its triangles
//! (measured in ares on a 157-prop scene: ~160 µs per draw, and props drawn
//! as 8-triangle stand-ins saved only ~15 µs each). Merging props that never
//! move into one mesh per area of the level removes most of that cost on
//! every target. Backends expose it as [`crate::Renderer::build_batch`].
//!
//! [`merge_tmdl`] bakes each part's matrix into its vertices: positions
//! transformed, normals by the inverse transpose (non-uniform scale and
//! quarter turns stay correctly lit) and renormalized, colors multiplied by
//! the part's tint. The result is a plain static TMDL with a `BNDS` section,
//! so it draws, culls and (when small enough) takes the N64's RSP path
//! exactly like an authored model.

use crate::math::{Color, Vec3};
use crate::math3d::Mat34;
use crate::render3d::{Mesh, MeshError, TMDL_MAGIC};

/// One part of a batch: a static mesh, where it goes, and its tint.
#[derive(Clone, Copy)]
pub struct BatchSource<'a> {
    pub mesh: &'a Mesh<'a>,
    /// Model matrix (the part's placement in the batch's space).
    pub matrix: Mat34,
    /// Multiplies the part's vertex colors (white = unchanged).
    pub tint: Color,
}

/// Bytes [`merge_tmdl`] writes for `parts` (`None`: more vertices than a
/// TMDL's `u16` indices can address).
pub fn merged_size(parts: &[BatchSource]) -> Option<usize> {
    let (mut v, mut i) = (0usize, 0usize);
    for p in parts {
        v += p.mesh.vertex_count;
        i += p.mesh.index_count;
    }
    if v > u16::MAX as usize + 1 {
        return None;
    }
    Some(12 + v * (12 + 12 + 4) + i * 2 + BNDS_BYTES)
}

const BNDS_BYTES: usize = 8 + 16;

/// Can this mesh join a batch? Static (no skeleton), untextured, opaque.
pub fn batchable(mesh: &Mesh) -> bool {
    mesh.joint_count() == 0 && !mesh.has_uvs() && !mesh.has_blend()
}

/// Merge `parts` into one static TMDL blob in `out`; returns its length.
/// Errors: a part that is not [`batchable`] (`BadSection`), too many
/// vertices or a short `out` (`Truncated`).
pub fn merge_tmdl(parts: &[BatchSource], out: &mut [u8]) -> Result<usize, MeshError> {
    merge_with(parts.len(), out, |k| parts[k])
}

/// [`merge_tmdl`] over `n` parts produced by `part(k)` (no slice of
/// sources needed: console stacks are small).
pub fn merge_with<'a>(
    n: usize,
    out: &mut [u8],
    part: impl Fn(usize) -> BatchSource<'a>,
) -> Result<usize, MeshError> {
    let (mut v, mut i) = (0usize, 0usize);
    for k in 0..n {
        let p = part(k);
        if !batchable(p.mesh) {
            return Err(MeshError::BadSection);
        }
        v += p.mesh.vertex_count;
        i += p.mesh.index_count;
    }
    if v > u16::MAX as usize + 1 {
        return Err(MeshError::Truncated);
    }
    let size = 12 + v * 28 + i * 2 + BNDS_BYTES;
    if out.len() < size {
        return Err(MeshError::Truncated);
    }
    out[0..4].copy_from_slice(TMDL_MAGIC);
    out[4..8].copy_from_slice(&(v as u32).to_le_bytes());
    out[8..12].copy_from_slice(&(i as u32).to_le_bytes());
    let pos_at = 12;
    let nrm_at = pos_at + v * 12;
    let col_at = nrm_at + v * 12;
    let idx_at = col_at + v * 4;
    let mut lo = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut hi = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
    let (mut vbase, mut ibase) = (0usize, 0usize);
    for k in 0..n {
        let p = part(k);
        let m = &p.matrix.m;
        let col = |c: usize| Vec3::new(m[0][c], m[1][c], m[2][c]);
        let (a, b, c) = (col(0), col(1), col(2));
        // Inverse transpose up to a positive factor: n' = n.x (b×c) +
        // n.y (c×a) + n.z (a×b), sign-corrected for mirrored matrices.
        let (bc, ca, ab) = (b.cross(c), c.cross(a), a.cross(b));
        let flip = if a.dot(bc) < 0.0 { -1.0 } else { 1.0 };
        let tint = p.tint;
        for k in 0..p.mesh.vertex_count {
            let q = p.matrix.transform_point(p.mesh.position(k));
            lo = Vec3::new(lo.x.min(q.x), lo.y.min(q.y), lo.z.min(q.z));
            hi = Vec3::new(hi.x.max(q.x), hi.y.max(q.y), hi.z.max(q.z));
            let n = p.mesh.normal(k);
            let t = Vec3::new(
                (bc.x * n.x + ca.x * n.y + ab.x * n.z) * flip,
                (bc.y * n.x + ca.y * n.y + ab.y * n.z) * flip,
                (bc.z * n.x + ca.z * n.y + ab.z * n.z) * flip,
            )
            .normalized();
            let at = vbase + k;
            put_vec3(out, pos_at + at * 12, q);
            put_vec3(out, nrm_at + at * 12, t);
            let cl = p.mesh.color(k);
            let mul = |x: u8, y: u8| ((x as u16 * y as u16 + 127) / 255) as u8;
            out[col_at + at * 4..col_at + at * 4 + 4].copy_from_slice(&[
                mul(cl.r, tint.r),
                mul(cl.g, tint.g),
                mul(cl.b, tint.b),
                mul(cl.a, tint.a),
            ]);
        }
        for k in 0..p.mesh.index_count {
            let idx = (vbase + p.mesh.index(k) as usize) as u16;
            let at = idx_at + (ibase + k) * 2;
            out[at..at + 2].copy_from_slice(&idx.to_le_bytes());
        }
        vbase += p.mesh.vertex_count;
        ibase += p.mesh.index_count;
    }
    // BNDS: a sphere around the box (generous, cheap).
    let at = idx_at + i * 2;
    out[at..at + 4].copy_from_slice(b"BNDS");
    out[at + 4..at + 8].copy_from_slice(&16u32.to_le_bytes());
    let (center, radius) = if v == 0 {
        (Vec3::ZERO, 0.0)
    } else {
        let c = Vec3::new(
            (lo.x + hi.x) * 0.5,
            (lo.y + hi.y) * 0.5,
            (lo.z + hi.z) * 0.5,
        );
        let d = Vec3::new(hi.x - c.x, hi.y - c.y, hi.z - c.z);
        (c, crate::math3d::sqrt(d.dot(d)))
    };
    put_vec3(out, at + 8, center);
    out[at + 20..at + 24].copy_from_slice(&radius.to_le_bytes());
    Ok(size)
}

fn put_vec3(out: &mut [u8], at: usize, v: Vec3) {
    out[at..at + 4].copy_from_slice(&v.x.to_le_bytes());
    out[at + 4..at + 8].copy_from_slice(&v.y.to_le_bytes());
    out[at + 8..at + 12].copy_from_slice(&v.z.to_le_bytes());
}

/// [`merge_tmdl`] for [`crate::BatchPart`]s whose meshes the backend looked
/// up (`meshes[k]` is `parts[k].model`): what every backend's
/// `build_batch` does. `out` must hold [`merged_size_of`] bytes.
pub fn merge_parts(
    meshes: &[Mesh],
    parts: &[crate::BatchPart],
    out: &mut [u8],
) -> Result<usize, MeshError> {
    if meshes.len() != parts.len() {
        return Err(MeshError::BadSection);
    }
    merge_with(parts.len(), out, |k| BatchSource {
        mesh: &meshes[k],
        matrix: parts[k].matrix,
        tint: parts[k].tint,
    })
}

/// Bytes [`merge_parts`] needs for these meshes (`None`: too many vertices
/// for u16 indices).
pub fn merged_size_of(meshes: &[Mesh]) -> Option<usize> {
    let (mut v, mut i) = (0usize, 0usize);
    for m in meshes {
        v += m.vertex_count;
        i += m.index_count;
    }
    (v <= u16::MAX as usize + 1).then_some(12 + v * 28 + i * 2 + BNDS_BYTES)
}
