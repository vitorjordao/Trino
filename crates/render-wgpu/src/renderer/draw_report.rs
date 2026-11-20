//! Which models cost the frame (`TRINO_DRAW_REPORT`): per model, how many
//! draws and triangles went to the GPU T&L path, how many stayed on the
//! CPU path and why ([`PcGpuReject`]), and the CPU path's T&L time each
//! one took. `TRINO_FRAME` says *that* draws left the GPU path; this says
//! *which* model, so a slow scene points at the mesh to fix (a fence with
//! an alpha texture, a floor every lamp reaches, a translucent glow).
//!
//! Off by default: the renderer only counts while
//! [`PcRenderer::set_draw_report`](super::PcRenderer::set_draw_report) is
//! on, and the names come from the hosts' uploads
//! ([`PcRenderer::name_model`](super::PcRenderer::name_model)).

use std::collections::HashMap;

use trino_core::render3d::PcGpuReject;

/// What one model cost over the frames counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelCost {
    /// Draw calls (`draw_model*`, one per `draw_model_instances*` call).
    pub draws: u64,
    /// Copies drawn (an instanced call's instances; 1 per plain draw).
    pub copies: u64,
    pub gpu_copies: u64,
    pub gpu_tris: u64,
    pub cpu_copies: u64,
    pub cpu_tris: u64,
    /// The CPU path's T&L time of this model's copies, ns.
    pub cpu_ns: u64,
    /// CPU copies per [`PcGpuReject`].
    pub reasons: [u64; PcGpuReject::COUNT],
}

impl ModelCost {
    /// The reason most of its CPU copies had (`None`: all on the GPU).
    pub fn main_reason(&self) -> Option<PcGpuReject> {
        PcGpuReject::ALL
            .iter()
            .copied()
            .filter(|r| self.reasons[r.index()] > 0)
            .max_by(|a, b| {
                self.reasons[a.index()]
                    .cmp(&self.reasons[b.index()])
                    .then(b.cmp(a))
            })
    }
}

/// One row of the report, per frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRow {
    pub id: u32,
    /// The logical path (`models/fence`), or the id in hex.
    pub name: String,
    pub draws: f32,
    pub copies: f32,
    pub gpu_tris: f32,
    pub cpu_copies: f32,
    pub cpu_tris: f32,
    pub cpu_us: f32,
    pub reason: Option<PcGpuReject>,
}

#[derive(Default)]
pub struct DrawReport {
    on: bool,
    frames: u64,
    models: HashMap<u32, ModelCost>,
    names: HashMap<u32, String>,
}

impl DrawReport {
    pub fn set_on(&mut self, on: bool) {
        self.on = on;
    }

    pub fn name(&mut self, id: u32, name: &str) {
        self.names.insert(id, name.to_string());
    }

    /// The end of a counted frame.
    pub fn frame(&mut self) {
        if self.on {
            self.frames += 1;
        }
    }

    fn entry(&mut self, model: u32) -> &mut ModelCost {
        self.models.entry(model).or_default()
    }

    /// A draw call of `copies` copies on the GPU path.
    pub fn gpu(&mut self, model: u32, copies: u32, tris: u32) {
        if !self.on {
            return;
        }
        let e = self.entry(model);
        e.draws += 1;
        e.copies += copies as u64;
        e.gpu_copies += copies as u64;
        e.gpu_tris += copies as u64 * tris as u64;
    }

    /// A draw call of `copies` copies left on the CPU path for `reason`
    /// (`draws` 0 for the rest of an instanced call already counted).
    pub fn cpu(&mut self, model: u32, draws: u32, copies: u32, tris: u32, reason: PcGpuReject) {
        if !self.on {
            return;
        }
        let e = self.entry(model);
        e.draws += draws as u64;
        e.copies += copies as u64;
        e.cpu_copies += copies as u64;
        e.cpu_tris += copies as u64 * tris as u64;
        e.reasons[reason.index()] += copies as u64;
    }

    /// The CPU path's T&L time of one copy of `model`.
    pub fn cpu_time(&mut self, model: u32, ns: u64) {
        if !self.on {
            return;
        }
        self.entry(model).cpu_ns += ns;
    }

    /// Every model per frame, the costliest first: CPU T&L time, then CPU
    /// triangles, then GPU triangles.
    pub fn rows(&self) -> Vec<ModelRow> {
        let n = self.frames.max(1) as f32;
        let mut rows: Vec<ModelRow> = self
            .models
            .iter()
            .map(|(id, c)| ModelRow {
                id: *id,
                name: self
                    .names
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| format!("{id:08x}")),
                draws: c.draws as f32 / n,
                copies: c.copies as f32 / n,
                gpu_tris: c.gpu_tris as f32 / n,
                cpu_copies: c.cpu_copies as f32 / n,
                cpu_tris: c.cpu_tris as f32 / n,
                cpu_us: c.cpu_ns as f32 / 1000.0 / n,
                reason: c.main_reason(),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.cpu_us
                .total_cmp(&a.cpu_us)
                .then(b.cpu_tris.total_cmp(&a.cpu_tris))
                .then(b.gpu_tris.total_cmp(&a.gpu_tris))
                .then(a.name.cmp(&b.name))
        });
        rows
    }

    /// `TRINO_DRAWS:` lines, the `top` costliest models per frame:
    /// `name draws= copies= gpu_tris= cpu_copies= cpu_tris= cpu_us= why=`.
    pub fn lines(&self, top: usize) -> Vec<String> {
        let rows = self.rows();
        let mut out = vec![format!(
            "TRINO_DRAWS: frames={} models={} (per frame, costliest first)",
            self.frames,
            rows.len()
        )];
        for r in rows.iter().take(top) {
            out.push(format!(
                "TRINO_DRAWS: {} draws={:.1} copies={:.1} gpu_tris={:.0} cpu_copies={:.1} \
                 cpu_tris={:.0} cpu_us={:.1} why={}",
                r.name,
                r.draws,
                r.copies,
                r.gpu_tris,
                r.cpu_copies,
                r.cpu_tris,
                r.cpu_us,
                r.reason.map_or("-", |r| r.key())
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off, nothing is counted; on, the rows average per frame, the CPU
    /// path's costliest model comes first and names its main reason.
    #[test]
    fn the_report_ranks_the_cpu_path_first() {
        let mut r = DrawReport::default();
        r.gpu(1, 10, 100);
        r.frame();
        assert!(r.rows().is_empty());
        r.set_on(true);
        r.name(1, "models/tree");
        r.name(2, "models/fence");
        for _ in 0..2 {
            r.gpu(1, 10, 100);
            r.cpu(2, 1, 1, 40, PcGpuReject::TextureAlpha);
            r.cpu(2, 1, 1, 40, PcGpuReject::TextureAlpha);
            r.cpu(2, 1, 1, 40, PcGpuReject::Offscreen);
            r.cpu_time(2, 30_000);
            r.cpu(3, 1, 1, 8, PcGpuReject::Translucent);
            r.frame();
        }
        let rows = r.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "models/fence");
        assert_eq!(rows[0].cpu_us, 30.0);
        assert_eq!(rows[0].cpu_copies, 3.0);
        assert_eq!(rows[0].cpu_tris, 120.0);
        assert_eq!(rows[0].reason, Some(PcGpuReject::TextureAlpha));
        // No time measured: CPU triangles rank it over a GPU-only model.
        assert_eq!(rows[1].name, "00000003");
        assert_eq!(rows[2].name, "models/tree");
        assert_eq!(rows[2].gpu_tris, 1000.0);
        assert_eq!(rows[2].reason, None);
        let lines = r.lines(1);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("TRINO_DRAWS: frames=2 models=3"));
        assert!(
            lines[1].ends_with("cpu_tris=120 cpu_us=30.0 why=texture_alpha"),
            "{}",
            lines[1]
        );
    }
}
