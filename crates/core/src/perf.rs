//! Console render profiling: where a frame's time goes, per stage and per
//! model, on the console's **own clock**.
//!
//! The core has no clock, so backends own the timing: the N64 and 3DS
//! renderers wrap each stage (frame begin, T&L, batch sort, rasterizer
//! submit, frame end) and each `draw_model` with their microsecond timer
//! and accumulate here while profiling is on. The T&L work counters
//! ([`TnlStats`]) come along, so a report says both *how long* (console
//! time — emulated N64 time under ares) and *how much work* (deterministic,
//! identical on PC, which is what host tests pin).
//!
//! [`RenderProfile::write_report`] prints the `TRINO_PERF_SCENE:` /
//! `TRINO_PERF_MODEL:` lines that `cargo xtask test n64|3ds` parse and
//! compare with the committed baselines in `tests/perf/`. Fixed capacity,
//! no allocator.

use crate::render::ModelId;
use crate::render3d::TnlStats;

/// A linear model of what 3D costs on a console, fitted to measured
/// frames: lets tools that only see the deterministic [`TnlStats`] (PC
/// tests, `model inspect`, the editor's budget HUD) say how long the
/// console would take, without an emulator.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CostModel {
    /// T&L per transformed vertex (vertex-cache miss).
    pub us_per_vert: f32,
    /// T&L per source triangle (fetch, clip test, project, cull).
    pub us_per_tri: f32,
    /// Fixed cost of one `draw_model*` call (setup, culling, pose).
    pub us_per_draw: f32,
    /// One depth-split bisection.
    pub us_per_split: f32,
    /// Depth sort + rasterizer submission per emitted triangle.
    pub us_per_emitted: f32,
}

/// The N64 (emulated time under ares), recalibrated 2026-10-13 after the
/// throughput plan's Phases 1–3c (`docs/plans/n64-throughput.md`: per-frame
/// T&L constants, backend-owned scratch, meshes parsed once, chunked vertex
/// fetch, split T&L code, opaque draws unsplit, one reciprocal per vertex,
/// model-space lighting), from the castle64 `TRINO_PERF_MODEL` /
/// `TRINO_PERF_SCENE` lines (default white light, no fog). castle64 no
/// longer splits, so the split term keeps its Phase 1 value, and vertices
/// and triangles are too correlated in real meshes (~2:1) for a free fit:
/// the per-vertex term is the micro scenes' measured 7.1 µs and the rest is
/// least squares over 39 per-model points (mean error 11%, scene totals
/// -4…+9%). Sort + submit come to 34–40 µs per emitted triangle. Code layout
/// alone moves real frames by ~5–10%, so read results as estimates.
/// Recalibrate when the T&L or the backends change (the n64-performance
/// skill says how); `cargo xtask test n64` prints estimate vs measurement
/// for every scene.
pub const N64_COST: CostModel = CostModel {
    us_per_vert: 7.1,
    us_per_tri: 17.3,
    us_per_draw: 183.8,
    us_per_split: 25.5,
    us_per_emitted: 37.0,
};

impl CostModel {
    /// Estimated microseconds of `draws` draw calls doing `tnl` work (T&L
    /// plus sort and submit).
    pub fn estimate_us(&self, tnl: &TnlStats, draws: u32) -> u32 {
        (self.us_per_vert * tnl.verts as f32
            + self.us_per_tri * tnl.tris_in as f32
            + self.us_per_draw * draws as f32
            + self.us_per_split * tnl.splits as f32
            + self.us_per_emitted * tnl.emitted as f32) as u32
    }

    /// Estimated T&L-only microseconds (no sort/submit) — comparable with
    /// the `tnl` stage of a console report.
    pub fn tnl_us(&self, tnl: &TnlStats, draws: u32) -> u32 {
        self.estimate_us(&TnlStats { emitted: 0, ..*tnl }, draws)
    }

    /// Estimated microseconds of one draw of a closed mesh with
    /// `vertices` / `triangles`, about half its faces toward the camera
    /// and no clipping or splits — a per-asset figure for tools.
    pub fn mesh_draw_us(&self, vertices: u32, triangles: u32) -> u32 {
        self.estimate_us(
            &TnlStats {
                verts: vertices,
                tris_in: triangles,
                emitted: triangles / 2,
                ..TnlStats::default()
            },
            1,
        )
    }
}

/// One frame at 30 fps, in microseconds.
pub const FRAME_30FPS_US: u32 = 33_333;

/// Distinct models a profile tracks individually (the rest pool into the
/// last slot's totals via [`RenderProfile::other`]).
pub const MAX_PROFILED_MODELS: usize = 24;

/// Accumulated cost of one model across the profiled frames.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ModelCost {
    pub model: ModelId,
    /// `draw_model*` calls (culled ones included).
    pub calls: u32,
    /// Console microseconds spent in the T&L for this model.
    pub us: u32,
    pub tnl: TnlStats,
}

impl Default for ModelCost {
    fn default() -> Self {
        ModelCost {
            model: ModelId(0),
            calls: 0,
            us: 0,
            tnl: TnlStats::default(),
        }
    }
}

/// Per-stage microseconds of a backend frame, summed over `frames`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StageTimes {
    /// `begin_frame`: framebuffer acquire + clears (may wait for vsync).
    pub begin: u32,
    /// Software T&L (`tessellate`) of every model.
    pub tnl: u32,
    /// Batch depth sort.
    pub sort: u32,
    /// Handing triangles to the rasterizer (RDP / citro / wgpu).
    pub submit: u32,
    /// Sprites and text (2D draws, including their batch flushes).
    pub sprites: u32,
    /// `end_frame`: final flush + present.
    pub end: u32,
}

impl StageTimes {
    pub fn total(&self) -> u32 {
        self.begin + self.tnl + self.sort + self.submit + self.sprites + self.end
    }
}

/// What a backend measured over a run of profiled frames.
#[derive(Clone, Copy, Debug)]
pub struct RenderProfile {
    pub frames: u32,
    pub stages: StageTimes,
    pub tnl: TnlStats,
    pub models: [ModelCost; MAX_PROFILED_MODELS],
    pub model_count: usize,
    /// Draws of models beyond [`MAX_PROFILED_MODELS`].
    pub other: ModelCost,
}

impl Default for RenderProfile {
    fn default() -> Self {
        RenderProfile {
            frames: 0,
            stages: StageTimes::default(),
            tnl: TnlStats::default(),
            models: [ModelCost::default(); MAX_PROFILED_MODELS],
            model_count: 0,
            other: ModelCost::default(),
        }
    }
}

/// Timing of the code around the renderer, measured by the caller.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FrameTimes {
    /// `Game::update` microseconds, summed over the frames.
    pub update: u32,
    /// `Game::render` microseconds (renderer stages + the game's own
    /// render-side logic), summed over the frames.
    pub render: u32,
    /// Display vblanks that elapsed (independent clock check: at 60 Hz,
    /// `render + update ≈ vblanks * 16 667 us`). 0 = not measured.
    pub vblanks: u32,
}

impl RenderProfile {
    /// Account one `draw_model*` call.
    pub fn record_model(&mut self, model: ModelId, us: u32, tnl: &TnlStats) {
        self.tnl.add(tnl);
        self.stages.tnl += us;
        let slot = match self.models[..self.model_count]
            .iter()
            .position(|m| m.model == model)
        {
            Some(i) => &mut self.models[i],
            None if self.model_count < MAX_PROFILED_MODELS => {
                self.model_count += 1;
                let m = &mut self.models[self.model_count - 1];
                m.model = model;
                m
            }
            None => &mut self.other,
        };
        slot.calls += 1;
        slot.us += us;
        slot.tnl.add(tnl);
    }

    /// `draw_model*` calls over the profiled frames.
    pub fn draws(&self) -> u32 {
        self.models[..self.model_count]
            .iter()
            .map(|m| m.calls)
            .sum::<u32>()
            + self.other.calls
    }

    /// The tracked models, most expensive first.
    pub fn models_by_cost(&self) -> [ModelCost; MAX_PROFILED_MODELS] {
        let mut out = self.models;
        out[..self.model_count].sort_unstable_by_key(|m| core::cmp::Reverse(m.us));
        out
    }

    /// Print the report: one `TRINO_PERF_SCENE:` line, then one
    /// `TRINO_PERF_MODEL:` line per model, every figure **per frame**
    /// (averaged over `frames`). `scene` must not contain spaces.
    pub fn write_report(
        &self,
        scene: &str,
        times: &FrameTimes,
        out: &mut dyn core::fmt::Write,
    ) -> core::fmt::Result {
        let n = self.frames.max(1);
        let s = &self.stages;
        let t = &self.tnl;
        writeln!(
            out,
            "TRINO_PERF_SCENE:{scene} frames={} update={} render={} begin={} tnl={} sort={} \
             submit={} sprites={} end={} game={} vblanks={} verts={} tris_in={} clipped={} \
             splits={} backfaced={} emitted={} culled={} draws={} rsp_draws={} rsp_tris={}",
            self.frames,
            times.update / n,
            times.render / n,
            s.begin / n,
            s.tnl / n,
            s.sort / n,
            s.submit / n,
            s.sprites / n,
            s.end / n,
            times.render.saturating_sub(s.total()) / n,
            times.vblanks,
            t.verts / n,
            t.tris_in / n,
            t.clipped / n,
            t.splits / n,
            t.backfaced / n,
            t.emitted / n,
            t.culled / n,
            self.draws() / n,
            t.rsp_draws / n,
            t.rsp_tris / n,
        )?;
        let sorted = self.models_by_cost();
        let other = [self.other];
        let extra = if self.other.calls > 0 {
            &other[..]
        } else {
            &[]
        };
        for m in sorted[..self.model_count].iter().chain(extra) {
            writeln!(
                out,
                "TRINO_PERF_MODEL:{scene} model={:08x} calls={} us={} verts={} tris_in={} \
                 emitted={} splits={}",
                m.model.0,
                m.calls / n,
                m.us / n,
                m.tnl.verts / n,
                m.tnl.tris_in / n,
                m.tnl.emitted / n,
                m.tnl.splits / n,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tnl(verts: u32, emitted: u32) -> TnlStats {
        TnlStats {
            verts,
            emitted,
            ..TnlStats::default()
        }
    }

    #[test]
    fn n64_cost_model_reproduces_its_calibration() {
        // (verts, tris_in, draws, splits, measured T&L us) from castle64.
        let points = [
            (159, 75, 1, 0, 2_845),     // castle doorway
            (140, 70, 1, 0, 2_483),     // boar
            (1229, 605, 26, 0, 24_908), // the whole hub scene
            (450, 220, 12, 0, 9_592),   // the whole throne scene
        ];
        for (verts, tris_in, draws, splits, us) in points {
            let t = TnlStats {
                verts,
                tris_in,
                splits,
                ..TnlStats::default()
            };
            let est = N64_COST.tnl_us(&t, draws) as f32;
            let err = (est - us as f32).abs() / us as f32;
            // Mean error 11%; small meshes drawn many times sit furthest
            // off (their per-draw cost moves with code layout).
            assert!(
                err < 0.15,
                "{verts}v {tris_in}t: est {est} vs {us} ({err:.2})"
            );
        }
    }

    #[test]
    fn models_accumulate_and_sort_by_cost() {
        let mut p = RenderProfile {
            frames: 2,
            ..RenderProfile::default()
        };
        let a = ModelId(0xA);
        let b = ModelId(0xB);
        p.record_model(a, 100, &tnl(10, 4));
        p.record_model(b, 900, &tnl(50, 20));
        p.record_model(a, 100, &tnl(10, 4));
        assert_eq!(p.model_count, 2);
        assert_eq!(p.stages.tnl, 1100);
        assert_eq!(p.tnl.verts, 70);
        let sorted = p.models_by_cost();
        assert_eq!(sorted[0].model, b);
        assert_eq!(sorted[1].calls, 2);
    }

    #[test]
    fn overflow_models_pool_into_other() {
        let mut p = RenderProfile::default();
        for i in 0..(MAX_PROFILED_MODELS as u32 + 3) {
            p.record_model(ModelId(i), 1, &TnlStats::default());
        }
        assert_eq!(p.model_count, MAX_PROFILED_MODELS);
        assert_eq!(p.other.calls, 3);
    }

    #[test]
    fn report_is_per_frame_and_parseable() {
        let mut p = RenderProfile {
            frames: 4,
            ..RenderProfile::default()
        };
        p.stages.begin = 400;
        p.record_model(ModelId(0x1234), 4000, &tnl(400, 80));
        let times = FrameTimes {
            update: 40,
            render: 6000,
            vblanks: 2,
        };
        let mut s = std::string::String::new();
        p.write_report("hub", &times, &mut s).unwrap();
        let mut lines = s.lines();
        let scene = lines.next().unwrap();
        assert!(
            scene.starts_with(
                "TRINO_PERF_SCENE:hub frames=4 update=10 render=1500 begin=100 tnl=1000"
            )
        );
        // game = render - renderer stages = (6000 - 4400) / 4.
        assert!(scene.contains(" game=400 "), "{scene}");
        assert!(scene.contains(" verts=100 "), "{scene}");
        assert!(
            scene.ends_with(" draws=0 rsp_draws=0 rsp_tris=0"),
            "{scene}"
        );
        let model = lines.next().unwrap();
        assert_eq!(
            model,
            "TRINO_PERF_MODEL:hub model=00001234 calls=0 us=1000 verts=100 tris_in=0 emitted=20 splits=0"
        );
    }
}
