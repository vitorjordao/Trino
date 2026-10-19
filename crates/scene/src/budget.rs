//! What a scene costs each platform from a camera: the draws, the software
//! T&L work behind them (what the PC renderer counts), on the N64 the split
//! between the RSP fast path and the CPU, on the 3DS between the PICA200's
//! vertex T&L and the CPU, on the PC native profile what its GPU T&L path
//! takes (issue #51) — with the rule each CPU draw failed — on the Web
//! (issue #47) the same split plus the GPU calls and pipelines a browser
//! pays for, the impostors' billboards, the LOD picks, and the draws sent
//! for nothing (a whole mesh outside the view, still paid for). A console's
//! expanded hardware tier (issue #54: `n64_expanded`, `n3ds_new`) is
//! estimated on its console's paths with its own variants, and reports the
//! memory its scene's models take against [`trino_core::Caps::ram_bytes`].
//!
//! With `environment.shadows` on, each platform's shadows too (ADR 0025):
//! its tier, the casters drawn as blobs, projected or in the map, the
//! N64's planar casters past its budget, and the blobs' N64 CPU time.
//!
//! The editor's Game view HUD and `cargo xtask scene budget` call
//! [`estimate`] on the same resolved scene, so a person and an agent read
//! the same numbers.

use trino_core::perf::{FRAME_30FPS_US, N64_COST, RspWork};
use trino_core::render3d::{
    Camera3, GpuReject, Light, Mesh, PcGpuReject, RspReject, TnlContext, TnlScratch, TnlStats,
    gpu_check, gpu_mesh_check, pc_gpu_check, pc_gpu_mesh_check, rsp_plan, tessellate_with,
};
use trino_core::shadow::{ShadowFrame, ShadowPlan, ShadowTier};
use trino_core::{LodPick, ModelId, SpriteId, Vec2};

use crate::{Resolved, Target};

/// What one platform's frame costs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameBudget {
    pub target: Target,
    /// Mesh draws sent to the renderer.
    pub draws: u32,
    /// The software T&L work of every mesh draw (the PC renderer's counters).
    pub tnl: TnlStats,
    /// N64 and 3DS: the draws (and their work) left on the CPU path (PC:
    /// every draw).
    pub cpu_draws: u32,
    pub cpu_tnl: TnlStats,
    /// N64: draws on the RSP fast path and their source triangles.
    pub rsp_draws: u32,
    pub rsp_tris: u32,
    /// N64: of those, the water surfaces (the water overlay's costs).
    pub rsp_water_draws: u32,
    pub rsp_water_tris: u32,
    /// N64: why the other draws stay on the CPU, most common first.
    pub off_rsp: Vec<(RspReject, u32)>,
    /// 3DS: draws on the GPU T&L path and their source triangles.
    pub gpu_draws: u32,
    pub gpu_tris: u32,
    /// 3DS: why the other draws stay on the CPU, most common first
    /// (`Offscreen` draws are culled there, not drawn).
    pub off_gpu: Vec<(GpuReject, u32)>,
    /// PC: draws the native profile's GPU T&L path takes
    /// ([`pc_gpu_check`], issue #51) and their source triangles; the
    /// others are `cpu_draws` and `cpu_tnl`, like the consoles' splits.
    pub pc_gpu_draws: u32,
    pub pc_gpu_tris: u32,
    /// PC: why the other draws stay off it, most common first
    /// (`Offscreen` draws are culled, not drawn).
    pub off_pc_gpu: Vec<(PcGpuReject, u32)>,
    /// Impostor billboards.
    pub billboards: u32,
    /// What entities with LOD levels drew: full, light, impostor, culled.
    pub lod: [u32; 4],
    /// Draws whose whole mesh was outside the view: sent and paid for,
    /// nothing drawn. Their names.
    pub wasted: Vec<String>,
    /// The water surfaces drawn (ADR 0015).
    pub water: Vec<WaterDraw>,
    /// Each drawn entity's path on this platform, in draw order: the
    /// editor's Inspector "Render path" row and the reasons' entity lists.
    pub paths: Vec<DrawPath>,
    /// Web: what the browser's GPU API is asked for ([`WebCalls`]).
    pub web: WebCalls,
    /// The baked bytes of every model the scene can draw from here on
    /// (each distinct model once, LOD levels included, wherever the camera
    /// is): what the scene's models take of the console's RAM.
    pub model_bytes: u32,
    /// The frame's shadows on this platform.
    pub shadows: ShadowBudget,
}

/// A platform's shadows ([`trino_core::shadow`]): its tier and what each
/// caster comes out as there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShadowBudget {
    /// The tier drawn: the platform's, capped by the scene's `tier_max`
    /// (`None` without `environment.shadows`).
    pub tier: ShadowTier,
    /// Draws that cast (`shadow` with a `cast`).
    pub casters: u32,
    pub blobs: u32,
    /// Projected onto their receiver (the 3DS's GPU, the N64's CPU T&L).
    pub planar: u32,
    /// In the shadow map (the PC, the Web).
    pub mapped: u32,
    /// `Mesh` casters past the N64's planar budget: they cast nothing.
    pub over_budget: u32,
    /// Surfaces blob and planar shadows land on.
    pub receivers: u32,
}

impl Default for ShadowBudget {
    fn default() -> Self {
        ShadowBudget {
            tier: ShadowTier::None,
            casters: 0,
            blobs: 0,
            planar: 0,
            mapped: 0,
            over_budget: 0,
            receivers: 0,
        }
    }
}

/// `shadows: blob — 3 casters: 3 blobs, 0 planar, 0 in the map; 1
/// receiver` (`None` when the scene has no shadows and nothing casts).
pub fn shadow_line(b: &FrameBudget) -> Option<String> {
    let s = &b.shadows;
    if s.tier == ShadowTier::None && s.casters == 0 {
        return None;
    }
    let mut line = format!(
        "shadows: {} — {} casters: {} blobs, {} planar, {} in the map",
        s.tier.key(),
        s.casters,
        s.blobs,
        s.planar,
        s.mapped
    );
    if s.over_budget > 0 {
        line += &format!(" ({} past the planar budget cast nothing)", s.over_budget);
    }
    line += &format!(
        "; {} receiver{}",
        s.receivers,
        if s.receivers == 1 { "" } else { "s" }
    );
    if b.target.base() == Target::N64 && s.blobs > 0 {
        line += &format!(
            "; blobs ≈{:.1} ms of N64 CPU",
            N64_COST.blobs_us(s.blobs) as f32 / 1000.0
        );
    }
    Some(line)
}

/// The Web's frame as WebGPU / WebGL2 calls (issue #47), estimated like
/// the renderer batches it: GPU T&L draws sharing a model and a texture
/// merge into one instanced call, CPU-path triangles into one call per
/// texture, each water surface its own. A browser pays per call and per
/// pipeline change in JavaScript, which on a phone binds long before the
/// triangles do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebCalls {
    /// GPU draw calls.
    pub calls: u32,
    /// Pipelines (shaders) the frame binds: GPU T&L, CPU triangles,
    /// per-pixel lit, water.
    pub pipelines: u32,
    /// Pipeline and texture changes between those calls (one per call
    /// after the first: no two calls share both).
    pub switches: u32,
}

/// Above this many calls a frame is CPU-bound in a phone's browser (WebGL2
/// validates every call in JavaScript): the HUD warns.
pub const WEB_CALLS_WARN: u32 = 300;
/// Above this many triangles a mid-range phone's GPU drops below 60 fps at
/// its native resolution (fillrate and vertex work): the HUD warns.
pub const WEB_TRIS_WARN: u32 = 250_000;

/// The HUD's and the CLI's Web line: `Web 12 calls · 3 pipelines · 11
/// switches`.
pub fn web_line(b: &FrameBudget) -> String {
    format!(
        "Web {} calls · {} pipelines · {} {}",
        b.web.calls,
        b.web.pipelines,
        b.web.switches,
        if b.web.switches == 1 {
            "switch"
        } else {
            "switches"
        }
    )
}

/// The Web's warnings for a frame: too many calls, or too many triangles,
/// for a phone's browser (empty when it fits).
pub fn web_warnings(b: &FrameBudget) -> Vec<String> {
    let mut out = Vec::new();
    if b.web.calls > WEB_CALLS_WARN {
        out.push(format!(
            "{} calls > {WEB_CALLS_WARN}: CPU-bound on mobile browsers",
            b.web.calls
        ));
    }
    let tris = b.tris() + b.pc_gpu_tris;
    if tris > WEB_TRIS_WARN {
        out.push(format!(
            "{tris} tris > {WEB_TRIS_WARN}: over a phone's fillrate"
        ));
    }
    out
}

/// How one entity's draw runs on a platform ([`FrameBudget::paths`]).
#[derive(Clone, Debug, PartialEq)]
pub struct DrawPath {
    pub name: String,
    pub path: RenderPath,
}

/// The hardware path (N64 RSP, 3DS PICA200, PC GPU T&L), the CPU path with
/// the rule the draw failed, or an impostor billboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderPath {
    Hardware,
    Cpu(&'static str),
    Impostor,
}

impl RenderPath {
    /// The hardware path's name on `target`.
    pub fn hardware_name(target: Target) -> &'static str {
        match target.base() {
            Target::N64 => "RSP",
            _ => "GPU",
        }
    }

    /// `GPU`, `CPU: fog`, `impostor` (the Inspector's wording).
    pub fn text(&self, target: Target) -> String {
        match self {
            RenderPath::Hardware => Self::hardware_name(target).to_string(),
            RenderPath::Cpu(r) => format!("CPU: {r}"),
            RenderPath::Impostor => "impostor".to_string(),
        }
    }

    /// One word for `--json`: `rsp`, `gpu`, `cpu` or `impostor`.
    pub fn key(&self, target: Target) -> &'static str {
        match (self, target.base()) {
            (RenderPath::Hardware, Target::N64) => "rsp",
            (RenderPath::Hardware, _) => "gpu",
            (RenderPath::Cpu(_), _) => "cpu",
            (RenderPath::Impostor, _) => "impostor",
        }
    }
}

impl FrameBudget {
    /// The path of the entity named `name` (its first draw), if drawn.
    pub fn path_of(&self, name: &str) -> Option<RenderPath> {
        self.paths.iter().find(|p| p.name == name).map(|p| p.path)
    }

    /// The entities off the hardware path for `reason` (a label), in draw
    /// order.
    pub fn off_path(&self, reason: &str) -> Vec<&str> {
        self.paths
            .iter()
            .filter(|p| matches!(p.path, RenderPath::Cpu(r) if r == reason))
            .map(|p| p.name.as_str())
            .collect()
    }
}

/// One water surface's cost: what the N64 tier spends on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaterDraw {
    pub name: String,
    /// Triangles that reach the rasterizer (ripple rings not counted: the
    /// estimate has no ripples).
    pub tris: u32,
    /// The N64 combiner: 2 cycles to blend both flowing layers, 1 for one.
    pub cycles: u8,
    /// The two layers (TMEM: [`tmem_bytes`] of each, sharing 4096 bytes).
    pub texture: Option<SpriteId>,
    pub detail: Option<SpriteId>,
}

/// TMEM bytes of a `width`×`height` texture in N64 `format` (rows padded to
/// 8 bytes, as rdpq loads them), or `None` for a palette format (CI4/CI8:
/// its TLUT takes TMEM's upper half, so it can't share TMEM with a second
/// layer) or an unknown one.
pub fn tmem_bytes(width: u32, height: u32, format: &str) -> Option<u32> {
    let bits = match format {
        "RGBA32" => 32,
        "RGBA16" | "RGBA5551" | "IA16" => 16,
        "I8" | "IA8" => 8,
        "I4" | "IA4" => 4,
        _ => return None,
    };
    Some((width * bits / 8).next_multiple_of(8) * height)
}

/// The HUD's and the CLI's water lines, two per surface: its triangles and
/// combiner cycles, then the layers' TMEM (`tmem` gives a sprite's N64 TMEM
/// bytes; `None` = a palette, unknown or missing) out of 4096. Short: the
/// HUD is 210 px wide.
pub fn water_lines(b: &FrameBudget, tmem: &dyn Fn(SpriteId) -> Option<u32>) -> Vec<String> {
    let mut out = Vec::new();
    for w in &b.water {
        out.push(format!(
            "Water {} · {} tris · {}-cycle",
            w.name, w.tris, w.cycles
        ));
        let layers: Vec<Option<u32>> = [w.texture, w.detail]
            .into_iter()
            .flatten()
            .map(tmem)
            .collect();
        out.push(if layers.is_empty() {
            "  no texture".to_string()
        } else if layers.iter().all(Option::is_some) {
            let sum: u32 = layers.iter().flatten().sum();
            let fit = if sum <= 4096 { "" } else { " over TMEM" };
            format!("  TMEM {sum}/4096 B{fit}")
        } else {
            "  TMEM: palette layer (detail dropped)".to_string()
        });
    }
    out
}

/// The HUD's and the CLI's PC GPU line: `PC GPU 12/14 draws · 1840 tris`,
/// then the most common reason the rest stay on the CPU path. Short: the
/// HUD is 210 px wide.
pub fn pc_gpu_line(b: &FrameBudget) -> String {
    let mut line = format!(
        "PC GPU {}/{} draws · {} tris",
        b.pc_gpu_draws, b.draws, b.pc_gpu_tris
    );
    if let Some((r, k)) = b.off_pc_gpu.first() {
        line.push_str(&format!(" · {k} {}", r.label()));
    }
    line
}

impl FrameBudget {
    /// Triangles that reach the rasterizer (+2 per billboard).
    pub fn tris(&self) -> u32 {
        self.tnl.emitted + self.billboards * 2
    }

    /// N64 CPU time: the CPU-path draws' T&L, sort and submit, and the
    /// billboards (µs, calibrated model).
    pub fn n64_cpu_us(&self) -> u32 {
        N64_COST
            .split_us(&self.cpu_tnl, self.cpu_draws, &self.rsp_work())
            .0
            + N64_COST.billboards_us(self.billboards)
            + N64_COST.blobs_us(self.shadows.blobs)
    }

    /// N64 RSP time of the fast-path draws (µs).
    pub fn n64_rsp_us(&self) -> u32 {
        N64_COST.rsp_us(&self.rsp_work())
    }

    /// The frame's RSP draws and triangles, the water surfaces' apart.
    pub fn rsp_work(&self) -> RspWork {
        RspWork {
            draws: self.rsp_draws,
            tris: self.rsp_tris,
            water_draws: self.rsp_water_draws,
            water_tris: self.rsp_water_tris,
        }
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

/// The screen of a platform's top display (PC: the given size; a tier:
/// its console's).
pub fn screen_of(target: Target, pc: Vec2) -> Vec2 {
    match target.base() {
        Target::N64 => Vec2::new(320.0, 240.0),
        Target::N3ds => Vec2::new(400.0, 240.0),
        _ => pc,
    }
}

/// The hardware budget a target's estimate is held to: its `Caps` table
/// (the tiers' own, ADR 0020).
pub fn caps_of(target: Target) -> trino_core::Caps {
    use trino_core::Caps;
    match target {
        Target::Pc => Caps::PC,
        Target::Web => Caps::WEB,
        Target::N64 => Caps::N64,
        Target::N3ds => Caps::N3DS,
        Target::N64Expanded => Caps::N64_EXPANDED,
        Target::N3dsNew => Caps::N3DS_NEW,
    }
}

/// The HUD's and the CLI's memory line: `RAM 412 KB of 4096 KB models`
/// (the scene's models against the target's [`trino_core::Caps::ram_bytes`];
/// `None` where there is no budget: PC, Web).
pub fn ram_line(b: &FrameBudget) -> Option<String> {
    let ram = caps_of(b.target).ram_bytes;
    (ram != u32::MAX).then(|| {
        format!(
            "{} RAM {} KB models of {} KB",
            b.target.label(),
            b.model_bytes.div_ceil(1024),
            ram / 1024
        )
    })
}

/// The hardware a target's estimate assumes, in one line: `New 3DS: 2
/// cores at 804 MHz, 64 MB RAM, crowds x2, particles x2` (`None` on the PC
/// and the Web, which have no table to hold a scene to).
pub fn hardware_line(target: Target) -> Option<String> {
    let caps = caps_of(target);
    if caps.ram_bytes == u32::MAX {
        return None;
    }
    let cores = if caps.cpu_cores == 1 {
        "1 core".to_string()
    } else {
        format!("{} cores", caps.cpu_cores)
    };
    let mut line = format!(
        "{}: {cores} at {} MHz, {} MB RAM",
        target.label(),
        caps.cpu_mhz,
        caps.ram_bytes / (1024 * 1024)
    );
    if caps.crowd_percent != 100 || caps.particle_percent != 100 {
        line.push_str(&format!(
            ", crowds x{}, particles x{}",
            caps.crowd_scale(),
            caps.particle_scale()
        ));
    }
    Some(line)
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
        model_bytes: model_bytes(resolved, meshes),
        ..FrameBudget::default()
    };
    // A tier runs on its console's paths (same GPU, same RSP).
    let console = target.base();
    // The shadows as the platform's renderer plans them (its tier, the
    // N64's planar budget), at the best quality.
    let mut shadows = Box::new(ShadowFrame::OFF);
    shadows.set(resolved.shadows.as_ref(), &caps_of(target), ShadowTier::Map);
    b.shadows.tier = shadows.tier();
    b.shadows.receivers = resolved.receivers().len() as u32;
    let visible = resolved.visible_cells(camera, screen);
    let mut reasons: Vec<(RspReject, u32)> = Vec::new();
    let mut gpu_reasons: Vec<(GpuReject, u32)> = Vec::new();
    let mut pc_reasons: Vec<(PcGpuReject, u32)> = Vec::new();
    // The Web's calls: (pipeline, model, texture) groups, and the
    // pipelines used as bits.
    let mut web_groups: Vec<(u8, u32, u32)> = Vec::new();
    let mut web_pipes = 0u8;
    let mut web_call = |pipe: u8, model: u32, texture: u32| {
        web_pipes |= 1 << pipe;
        if pipe == WEB_WATER || !web_groups.contains(&(pipe, model, texture)) {
            web_groups.push((pipe, model, texture));
        }
    };
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
            LodPick::Impostor(imp) => {
                b.billboards += 1;
                web_call(WEB_TRIS, 0, imp.sprite.0);
                b.paths.push(DrawPath {
                    name: item.name.clone(),
                    path: RenderPath::Impostor,
                });
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
        if item.params.shadow.casts() {
            b.shadows.casters += 1;
            match shadows.plan(item.params.shadow) {
                ShadowPlan::Blob { .. } => b.shadows.blobs += 1,
                ShadowPlan::Map => b.shadows.mapped += 1,
                ShadowPlan::Planar => {
                    b.shadows.planar += 1;
                    // The N64 flattens it through the CPU T&L: about the
                    // caster's own T&L again.
                    if console == Target::N64 {
                        b.cpu_tnl.add(&stats);
                    }
                }
                ShadowPlan::None => {}
            }
        }
        if item.params.water.is_some() {
            let texture = mesh.texture_for(&item.params);
            let detail = texture.and(item.params.detail.map(|d| d.sprite));
            b.water.push(WaterDraw {
                name: item.name.clone(),
                tris: stats.emitted,
                cycles: if detail.is_some() { 2 } else { 1 },
                texture,
                detail,
            });
        }
        if console == Target::N3ds {
            match gpu_mesh_check(&mesh)
                .and_then(|_| gpu_check(&ctx, &mesh, &item.matrix, &item.params))
            {
                Ok(_) => {
                    b.gpu_draws += 1;
                    b.gpu_tris += (mesh.index_count / 3) as u32;
                    b.paths.push(path(item, RenderPath::Hardware));
                }
                Err(r) => {
                    b.cpu_draws += 1;
                    b.cpu_tnl.add(&stats);
                    count(&mut gpu_reasons, r);
                    b.paths.push(path(item, RenderPath::Cpu(r.label())));
                }
            }
            continue;
        }
        if matches!(console, Target::Pc | Target::Web) {
            // A glossy draw is shaded per pixel on the PC (the estimate has
            // no local lights or wetness, the other two reasons).
            let per_pixel =
                item.params.gloss > 0 && item.params.shading != trino_core::Shading::Unlit;
            match pc_gpu_mesh_check(&mesh)
                .and_then(|_| pc_gpu_check(&ctx, &mesh, &item.matrix, &item.params))
                .and(match per_pixel {
                    true => Err(PcGpuReject::PerPixelLit),
                    false => Ok(()),
                }) {
                Ok(()) => {
                    let texture = mesh.texture_for(&item.params).map_or(0, |t| t.0);
                    web_call(WEB_GPU, id.0, texture);
                    b.pc_gpu_draws += 1;
                    b.pc_gpu_tris += (mesh.index_count / 3) as u32;
                    b.paths.push(path(item, RenderPath::Hardware));
                }
                Err(r) => {
                    let texture = mesh.texture_for(&item.params).map_or(0, |t| t.0);
                    let pipe = match r {
                        PcGpuReject::Water => WEB_WATER,
                        PcGpuReject::PerPixelLit => WEB_LIT,
                        _ => WEB_TRIS,
                    };
                    web_call(pipe, 0, texture);
                    b.cpu_draws += 1;
                    b.cpu_tnl.add(&stats);
                    count(&mut pc_reasons, r);
                    b.paths.push(path(item, RenderPath::Cpu(r.label())));
                }
            }
            continue;
        }
        if console != Target::N64 {
            continue;
        }
        match rsp_plan(&ctx, &mesh, &item.matrix, &item.params) {
            // One draw however many commands (a rigid mesh sends one per
            // joint on screen): `rsp_draws + cpu_draws` stays `draws`.
            Ok((_, tris)) => {
                b.rsp_draws += 1;
                b.rsp_tris += tris;
                if item.params.water.is_some() {
                    b.rsp_water_draws += 1;
                    b.rsp_water_tris += tris;
                }
                b.paths.push(path(item, RenderPath::Hardware));
            }
            Err(r) => {
                b.cpu_draws += 1;
                b.cpu_tnl.add(&stats);
                count(&mut reasons, r);
                b.paths.push(path(item, RenderPath::Cpu(r.label())));
            }
        }
    }
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    gpu_reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    pc_reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    b.off_rsp = reasons;
    b.off_gpu = gpu_reasons;
    b.off_pc_gpu = pc_reasons;
    b.shadows.over_budget = shadows.stats.over_budget as u32;
    if target == Target::Web {
        let calls = web_groups.len() as u32;
        b.web = WebCalls {
            calls,
            pipelines: web_pipes.count_ones(),
            switches: calls.saturating_sub(1),
        };
    }
    b
}

/// The baked bytes of the distinct models `resolved` can draw (each
/// entity's model and its LOD levels' models).
fn model_bytes<'m>(resolved: &Resolved, meshes: &dyn Fn(ModelId) -> Option<&'m [u8]>) -> u32 {
    let mut seen: Vec<ModelId> = Vec::new();
    let mut add = |id: ModelId| {
        if !seen.contains(&id) {
            seen.push(id);
        }
    };
    for item in &resolved.items {
        if let Some(m) = &item.model {
            add(m.0);
        }
        for l in &item.lod {
            if let crate::LodDraw::Model(id, _) = &l.draw {
                add(*id);
            }
        }
    }
    seen.iter()
        .filter_map(|id| meshes(*id))
        .map(|blob| blob.len() as u32)
        .sum()
}

/// The Web's pipelines ([`WebCalls::pipelines`]), as bit numbers.
const WEB_GPU: u8 = 0;
const WEB_TRIS: u8 = 1;
const WEB_LIT: u8 = 2;
/// Water: one call per surface (drawn last, blended).
const WEB_WATER: u8 = 3;

fn path(item: &crate::DrawItem, path: RenderPath) -> DrawPath {
    DrawPath {
        name: item.name.clone(),
        path,
    }
}

/// One more draw for `reason`.
fn count<R: PartialEq>(reasons: &mut Vec<(R, u32)>, reason: R) {
    match reasons.iter_mut().find(|(k, _)| *k == reason) {
        Some((_, n)) => *n += 1,
        None => reasons.push((reason, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmem_counts_padded_rows_and_refuses_palettes() {
        assert_eq!(tmem_bytes(32, 32, "I8"), Some(1024));
        assert_eq!(tmem_bytes(32, 32, "I4"), Some(512));
        assert_eq!(tmem_bytes(32, 32, "RGBA5551"), Some(2048));
        assert_eq!(tmem_bytes(4, 2, "I4"), Some(16), "rows pad to 8 bytes");
        assert_eq!(tmem_bytes(32, 32, "CI4"), None);
        let b = FrameBudget {
            water: vec![WaterDraw {
                name: "river".into(),
                tris: 128,
                cycles: 2,
                texture: Some(SpriteId(1)),
                detail: Some(SpriteId(2)),
            }],
            ..FrameBudget::default()
        };
        let lines = water_lines(&b, &|s| Some(if s.0 == 1 { 1024 } else { 512 }));
        assert_eq!(
            lines,
            ["Water river · 128 tris · 2-cycle", "  TMEM 1536/4096 B"]
        );
        let over = water_lines(&b, &|_| Some(4000));
        assert!(over[1].contains("over TMEM"), "{over:?}");
        assert!(water_lines(&b, &|_| None)[1].contains("palette"));
    }
}
