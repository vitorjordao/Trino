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
//! **Phase 1:** the PC backend still draws every mesh through the CPU path;
//! these rules only decide and report ([`TnlStats::pc_gpu_draws`]), so the
//! picture and the CPU work counters are those of the CPU path. The GPU path
//! itself is Phase 2. The rules are permissive: a desktop GPU can do what the
//! CPU path does, so a reject names only what the GPU path won't port yet
//! (skeletons until Phase 3, water, the per-pixel lit surfaces) or what
//! needs the CPU path's per-triangle order (blending).
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
    /// A skeleton (rigid joints): the joint matrices reach WGSL in Phase 3.
    Skinned,
    /// Smooth skinning weights (`SKW2`): Phase 3 too.
    Blended,
    /// No vertices or no triangles.
    Empty,
    // The draw.
    /// A water surface: the core displaces it ([`crate::water`]).
    Water,
    /// A tint with alpha below 255, or vertex alpha below 255: blending
    /// follows the CPU path's per-triangle painter order.
    Translucent,
    /// Wholly outside the guard band or behind the near plane: the CPU
    /// path culls it (invisible work).
    Offscreen,
    /// Shaded per pixel by a local light, gloss or wetness: the lit
    /// pipeline's surface varyings come from the CPU path
    /// ([`tessellate_surface`](super::tessellate_surface)). Reported by the
    /// renderer, never by [`pc_gpu_check`] (which doesn't see the lights).
    PerPixelLit,
}

// `ALL` ends with the last variant: a new reason must join it.
const _: () = assert!(PcGpuReject::PerPixelLit as usize + 1 == PcGpuReject::COUNT);

impl PcGpuReject {
    /// Every reason, in [`PcGpuReject::index`] order.
    pub const ALL: [PcGpuReject; PcGpuReject::COUNT] = [
        PcGpuReject::Skinned,
        PcGpuReject::Blended,
        PcGpuReject::Empty,
        PcGpuReject::Water,
        PcGpuReject::Translucent,
        PcGpuReject::Offscreen,
        PcGpuReject::PerPixelLit,
    ];

    /// Reasons in all ([`PcGpuReject::ALL`]'s length).
    pub const COUNT: usize = 7;

    /// The reason's slot in a per-reason histogram (`[u32; COUNT]`).
    pub const fn index(self) -> usize {
        self as usize
    }

    /// A few words for tools (budget reports).
    pub const fn label(self) -> &'static str {
        match self {
            PcGpuReject::Skinned => "skinned mesh",
            PcGpuReject::Blended => "smooth skinning",
            PcGpuReject::Empty => "empty mesh",
            PcGpuReject::Water => "water surface",
            PcGpuReject::Translucent => "translucent",
            PcGpuReject::Offscreen => "off screen",
            PcGpuReject::PerPixelLit => "per-pixel lit",
        }
    }

    /// One word for `TRINO_FRAME` lines (`off_pc_gpu=translucent:3,...`).
    pub const fn key(self) -> &'static str {
        match self {
            PcGpuReject::Skinned => "skinned",
            PcGpuReject::Blended => "blended",
            PcGpuReject::Empty => "empty",
            PcGpuReject::Water => "water",
            PcGpuReject::Translucent => "translucent",
            PcGpuReject::Offscreen => "offscreen",
            PcGpuReject::PerPixelLit => "per_pixel_lit",
        }
    }
}

/// Whether a mesh can ever take the PC's GPU T&L path (checked once, at
/// register).
pub fn pc_gpu_mesh_check(mesh: &Mesh) -> Result<(), PcGpuReject> {
    if mesh.joint_count() != 0 {
        return Err(PcGpuReject::Skinned);
    }
    if mesh.has_blend() {
        return Err(PcGpuReject::Blended);
    }
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
/// In the core so a host tool (`scene budget`) predicts the split exactly.
pub fn pc_gpu_check(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
) -> Result<(), PcGpuReject> {
    if params.water.is_some() {
        return Err(PcGpuReject::Water);
    }
    if params.tint.a < 255 || !mesh.opaque {
        return Err(PcGpuReject::Translucent);
    }
    // The CPU path culls a draw wholly outside its guard band or behind the
    // near plane (`tessellate_with`'s sphere test, the same arithmetic): do
    // the same, so both paths draw (and count) the same set of meshes.
    if let Some((c, r)) = mesh.bounds() {
        let c = ctx.view.mul(model).transform_point(c);
        let s = model.axis_scales_sq();
        let r2 = r * r * s.x.max(s.y).max(s.z);
        let outside = |d: f32, lim: f32| d < 0.0 && d * d > r2 * (lim * lim + 1.0);
        let behind = c.z <= NEAR && (NEAR - c.z) * (NEAR - c.z) >= r2;
        let (lim_x, lim_y) = (ctx.lim_x, ctx.lim_y);
        if behind
            || outside(lim_x * c.z - c.x, lim_x)
            || outside(lim_x * c.z + c.x, lim_x)
            || outside(lim_y * c.z - c.y, lim_y)
            || outside(lim_y * c.z + c.y, lim_y)
        {
            return Err(PcGpuReject::Offscreen);
        }
    }
    Ok(())
}
