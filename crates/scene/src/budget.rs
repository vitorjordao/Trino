//! What a scene costs each platform from a camera: the draws, the software
//! T&L work behind them (what the PC renderer counts), on the N64 the split
//! between the RSP fast path and the CPU — with the rule each CPU draw
//! failed — the impostors' billboards, the LOD picks, and the draws sent
//! for nothing (a whole mesh outside the view, still paid for).
//!
//! The editor's Game view HUD and `cargo xtask scene budget` call
//! [`estimate`] on the same resolved scene, so a person and an agent read
//! the same numbers.

use trino_core::perf::{FRAME_30FPS_US, N64_COST};
use trino_core::render3d::{
    Camera3, Light, Mesh, RspReject, TnlContext, TnlScratch, TnlStats, rsp_check, rsp_mesh_check,
    tessellate_with,
};
use trino_core::{LodPick, ModelId, Vec2};

use crate::{Resolved, Target};

/// What one platform's frame costs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameBudget {
    pub target: Target,
    /// Mesh draws sent to the renderer.
    pub draws: u32,
    /// The software T&L work of every mesh draw (the PC renderer's counters).
    pub tnl: TnlStats,
    /// N64: the draws (and their work) left on the CPU path.
    pub cpu_draws: u32,
    pub cpu_tnl: TnlStats,
    /// N64: draws on the RSP fast path and their source triangles.
    pub rsp_draws: u32,
    pub rsp_tris: u32,
    /// N64: why the other draws stay on the CPU, most common first.
    pub off_rsp: Vec<(RspReject, u32)>,
    /// Impostor billboards.
    pub billboards: u32,
    /// What entities with LOD levels drew: full, light, impostor, culled.
    pub lod: [u32; 4],
    /// Draws whose whole mesh was outside the view: sent and paid for,
    /// nothing drawn. Their names.
    pub wasted: Vec<String>,
}

impl FrameBudget {
    /// Triangles that reach the rasterizer (+2 per billboard).
    pub fn tris(&self) -> u32 {
        self.tnl.emitted + self.billboards * 2
    }

    /// N64 CPU time: the CPU-path draws' T&L, sort and submit, and the
    /// billboards (µs, calibrated model).
    pub fn n64_cpu_us(&self) -> u32 {
        N64_COST.estimate_us(&self.cpu_tnl, self.cpu_draws)
            + N64_COST.billboards_us(self.billboards)
    }

    /// N64 RSP time of the fast-path draws (µs).
    pub fn n64_rsp_us(&self) -> u32 {
        N64_COST.rsp_us(self.rsp_tris)
    }

    /// The N64 frame: CPU and RSP work in parallel, the slower one bounds.
    pub fn n64_frame_us(&self) -> u32 {
        self.n64_cpu_us().max(self.n64_rsp_us())
    }

    /// The share of a 30 fps frame (100 = the whole frame).
    pub fn n64_frame_percent(&self) -> f32 {
        self.n64_frame_us() as f32 / FRAME_30FPS_US as f32 * 100.0
    }
}

/// The screen of a platform's top display (PC: the given size).
pub fn screen_of(target: Target, pc: Vec2) -> Vec2 {
    match target {
        Target::Pc => pc,
        Target::N64 => Vec2::new(320.0, 240.0),
        Target::N3ds => Vec2::new(400.0, 240.0),
    }
}

/// Estimate the frame of `resolved` (resolved for `target`) from `camera`,
/// with `meshes` giving each model's baked TMDL. Interior cells are culled
/// like the game does; animated models are posed at their resolve time.
pub fn estimate<'m>(
    resolved: &Resolved,
    meshes: &dyn Fn(ModelId) -> Option<&'m [u8]>,
    camera: &Camera3,
    light: &Light,
    screen: Vec2,
    target: Target,
) -> FrameBudget {
    let ctx = TnlContext::new(camera, light, screen);
    let mut scratch = Box::new(TnlScratch::new());
    let mut b = FrameBudget {
        target,
        ..FrameBudget::default()
    };
    let visible = resolved.visible_cells(camera, screen);
    let mut reasons: Vec<(RspReject, u32)> = Vec::new();
    for item in &resolved.items {
        if !trino_core::vis::in_view(item.cell, visible) {
            continue;
        }
        let pick = item.lod_from(camera.eye);
        if !item.lod.is_empty() {
            let k = match pick {
                LodPick::Model(id) if item.model.as_ref().map(|m| m.0) == Some(id) => 0,
                LodPick::Model(_) => 1,
                LodPick::Impostor(_) => 2,
                LodPick::None => 3,
            };
            b.lod[k] += 1;
        }
        let id = match pick {
            LodPick::Model(id) => id,
            LodPick::Impostor(_) => {
                b.billboards += 1;
                continue;
            }
            LodPick::None => continue,
        };
        let Some(blob) = meshes(id) else { continue };
        let Ok(mesh) = Mesh::from_tmdl(blob) else {
            continue;
        };
        b.draws += 1;
        let stats = tessellate_with(
            &ctx,
            &mut scratch,
            &mesh,
            &item.matrix,
            &item.params,
            &mut |_| {},
        );
        b.tnl.add(&stats);
        if stats.culled > 0 {
            b.wasted.push(item.name.clone());
        }
        if target != Target::N64 {
            continue;
        }
        match rsp_mesh_check(&mesh).and_then(|_| rsp_check(&ctx, &mesh, &item.matrix, &item.params))
        {
            Ok(_) => {
                b.rsp_draws += 1;
                b.rsp_tris += (mesh.index_count / 3) as u32;
            }
            Err(r) => {
                b.cpu_draws += 1;
                b.cpu_tnl.add(&stats);
                match reasons.iter_mut().find(|(k, _)| *k == r) {
                    Some((_, n)) => *n += 1,
                    None => reasons.push((r, 1)),
                }
            }
        }
    }
    if target != Target::N64 {
        b.cpu_draws = b.draws;
        b.cpu_tnl = b.tnl;
    }
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    b.off_rsp = reasons;
    b
}
