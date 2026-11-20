//! Vertex T&L on the PC's GPU (issue #51, ADR 0016): the rules that decide,
//! per draw, whether the PC native profile (and later the Web target, #47)
//! may hand a mesh to a WGSL vertex stage instead of
//! [`tessellate`](super::tessellate). They sit beside the N64's
//! ([`rsp_check`](super::rsp_check)) and the 3DS's
//! ([`gpu_check`](super::gpu_check)) and follow their pattern: a mesh check
//! once at register, a draw check per draw, and a [`PcGpuReject`] naming the
//! first rule a draw fails, for `TRINO_FRAME`, the editor's budget HUD and
//! `cargo xtask scene budget`.
//!
//! **Since Phase 2** the PC backend draws the accepted meshes on the GPU
//! (WGSL vertex stage, resident vertex buffers, instanced draws), and the
//! CPU path stays the oracle for everything else. The rules are permissive:
//! a desktop GPU can do what the CPU path does (skeletons and smooth
//! skinning included, since Phase 3; per-pixel light since ADR 0016 §12;
//! translucent draws, sorted back to front per copy, and water surfaces,
//! displaced in the vertex stage, since §13), so a reject names only what
//! the GPU path doesn't port (a flat-shaded skeleton) or what needs the CPU
//! path's per-triangle painter order (a texture whose texels blend).
//!
//! The console simulation profiles (`SimProfile::N64` / `N3ds`) never take
//! this path: they keep the CPU path and report the RSP / PICA200 splits.
//!
//! [`TnlStats::pc_gpu_draws`]: super::TnlStats::pc_gpu_draws

use super::{Mesh, NEAR, TnlContext};
use crate::math3d::Mat34;
use crate::render::ModelParams;

/// Why a draw stays off the PC's GPU T&L path ([`pc_gpu_check`]): the first
/// rule it fails, in the order the rules are tested.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum PcGpuReject {
    // The mesh (decided once per mesh, [`pc_gpu_mesh_check`]).
    /// No vertices or no triangles.
    Empty,
    // The draw. Water surfaces and translucent draws (a tint or vertex
    // alpha below 255) were reasons until ADR 0016 §13: the GPU path
    // displaces water in its vertex stage and sorts translucent copies back
    // to front, between the CPU path's blended triangles.
    /// [`Shading::Flat`](crate::Shading::Flat) on a skeleton: a face
    /// normal needs the three corners' posed positions, which a vertex
    /// stage doesn't see (a static mesh carries its face normal instead).
    FlatSkinned,
    /// Wholly outside the guard band or behind the near plane: the CPU
    /// path culls it (invisible work).
    Offscreen,
    // Reported by the renderer, never by [`pc_gpu_check`] (it knows the
    // textures and its own settings). A draw shaded per pixel (a local
    // light, gloss, wetness) is no reason: the GPU path has lit pipelines.
    /// A texture with texels below full alpha: they blend, so the draw
    /// keeps the CPU path's per-triangle painter order (crossed cards of
    /// foliage blend within one copy; the budget can't see textures and
    /// doesn't predict it).
    TextureAlpha,
    /// The GPU path is turned off (`TRINO_PC_TNL=cpu`,
    /// `PcRenderer::set_hw_tnl(false)`): a draw it would have taken.
    Disabled,
}

// `ALL` ends with the last variant: a new reason must join it.
const _: () = assert!(PcGpuReject::Disabled as usize + 1 == PcGpuReject::COUNT);

impl PcGpuReject {
    /// Every reason, in [`PcGpuReject::index`] order.
    pub const ALL: [PcGpuReject; PcGpuReject::COUNT] = [
        PcGpuReject::Empty,
        PcGpuReject::FlatSkinned,
        PcGpuReject::Offscreen,
        PcGpuReject::TextureAlpha,
        PcGpuReject::Disabled,
    ];

    /// Reasons in all ([`PcGpuReject::ALL`]'s length).
    pub const COUNT: usize = 5;

    /// The reason's slot in a per-reason histogram (`[u32; COUNT]`).
    pub const fn index(self) -> usize {
        self as usize
    }

    /// A few words for tools (budget reports).
    pub const fn label(self) -> &'static str {
        match self {
            PcGpuReject::Empty => "empty mesh",
            PcGpuReject::FlatSkinned => "flat-shaded skeleton",
            PcGpuReject::Offscreen => "off screen",
            PcGpuReject::TextureAlpha => "texture alpha",
            PcGpuReject::Disabled => "GPU T&L off",
        }
    }

    /// One word for `TRINO_FRAME` lines (`off_pc_gpu=offscreen:3,...`).
    pub const fn key(self) -> &'static str {
        match self {
            PcGpuReject::Empty => "empty",
            PcGpuReject::FlatSkinned => "flat_skinned",
            PcGpuReject::Offscreen => "offscreen",
            PcGpuReject::TextureAlpha => "texture_alpha",
            PcGpuReject::Disabled => "disabled",
        }
    }
}

/// Whether a mesh can ever take the PC's GPU T&L path (checked once, at
/// register). Skeletons (rigid joints, `SKW2` smooth skinning) are fine
/// since Phase 3: the joint matrices go up per draw.
pub fn pc_gpu_mesh_check(mesh: &Mesh) -> Result<(), PcGpuReject> {
    if mesh.vertex_count == 0 || mesh.index_count < 3 {
        return Err(PcGpuReject::Empty);
    }
    Ok(())
}

/// Whether the PC native profile may send this draw to its GPU T&L path.
/// Only for meshes [`pc_gpu_mesh_check`] accepted. The GPU clips in
/// hardware and lights in floats, so there is no near-plane, band, scale,
/// light-color, fog, texture, palette or shading rule (unlike
/// [`rsp_check`](super::rsp_check) and [`gpu_check`](super::gpu_check)).
/// Translucent draws ([`pc_translucent`]) and water surfaces pass: the GPU
/// path blends the first back to front and displaces the second.
/// In the core so a host tool (`scene budget`) predicts the split exactly.
pub fn pc_gpu_check(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
) -> Result<(), PcGpuReject> {
    // Water is never faceted (its normal is the displaced surface's).
    if params.shading == crate::Shading::Flat && mesh.joint_count() != 0 && params.water.is_none() {
        return Err(PcGpuReject::FlatSkinned);
    }
    // The CPU path culls a draw wholly outside its guard band or behind the
    // near plane (`tessellate_with`'s sphere test, the same arithmetic): do
    // the same, so both paths draw (and count) the same set of meshes.
    if pc_offscreen(ctx, mesh, model) {
        return Err(PcGpuReject::Offscreen);
    }
    Ok(())
}

/// Whether a draw blends (a tint or vertex alpha below 255): the CPU path
/// sorts its triangles after the batch's opaque ones, back to front, and
/// the GPU path its copies, by the view depth of their bounding sphere's
/// center ([`pc_sort_depth`]). Water blends too, after both.
pub fn pc_translucent(mesh: &Mesh, params: &ModelParams) -> bool {
    params.water.is_none() && (params.tint.a < 255 || !mesh.opaque)
}

/// The view depth a translucent copy sorts by on the GPU path: its bounding
/// sphere's center (the mesh origin without bounds), never nearer than the
/// near plane. The CPU path sorts each triangle by its farthest corner;
/// a copy is one item, so convex shapes and copies apart from each other
/// come out the same.
pub fn pc_sort_depth(mesh: &Mesh, model_view: &Mat34) -> f32 {
    let c = mesh.bounds().map_or(crate::math::Vec3::ZERO, |(c, _)| c);
    let z = model_view.transform_point(c).z;
    if z > NEAR { z } else { NEAR }
}

/// Whether the draw is wholly outside the view: its bounding sphere past
/// one of the guard band's side planes or behind the near plane
/// (`tessellate_with`'s sphere test, the same arithmetic). Such a draw shows
/// nothing on any path; the PC drops it before choosing one (a CPU T&L job
/// of a culled mesh still walks its vertices).
pub fn pc_offscreen(ctx: &TnlContext, mesh: &Mesh, model: &Mat34) -> bool {
    let Some((c, r)) = mesh.bounds() else {
        return false;
    };
    let c = ctx.view.mul(model).transform_point(c);
    let s = model.axis_scales_sq();
    let r2 = r * r * s.x.max(s.y).max(s.z);
    let outside = |d: f32, lim: f32| d < 0.0 && d * d > r2 * (lim * lim + 1.0);
    let behind = c.z <= NEAR && (NEAR - c.z) * (NEAR - c.z) >= r2;
    let (lim_x, lim_y) = (ctx.lim_x, ctx.lim_y);
    behind
        || outside(lim_x * c.z - c.x, lim_x)
        || outside(lim_x * c.z + c.x, lim_x)
        || outside(lim_y * c.z - c.y, lim_y)
        || outside(lim_y * c.z + c.y, lim_y)
}
