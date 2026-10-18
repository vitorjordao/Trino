//! `TRINO_FRAME:` lines on the PC, like the consoles' hosts print them
//! (`N64Host::report_frames`, `N3dsHost::report_frames`): frame time and,
//! per frame, the 3D draws and how the native profile's GPU T&L path splits
//! them (issue #51: `pc_gpu_draws`, `pc_gpu_tris`, instanced copies and why
//! the rest stay on the CPU path, most common first).
//!
//! `apps/pc` and `trino-runner-pc` record every frame and print the line
//! when a `TRINO_SMOKE_FRAMES` run ends.

use std::time::Instant;

use trino_core::FrameStats;
use trino_core::render3d::PcGpuReject;

use crate::sim::SimProfile;

/// Frames recorded so far ([`FrameMeter::record`]).
#[derive(Clone, Debug)]
pub struct FrameMeter {
    profile: SimProfile,
    last: Option<Instant>,
    frames: u64,
    timed: u64,
    total_us: u64,
    max_us: u64,
    models: u64,
    tris: u64,
    pc_gpu_draws: u64,
    pc_gpu_tris: u64,
    instanced: u64,
    off: [u64; PcGpuReject::COUNT],
}

impl FrameMeter {
    pub fn new(profile: SimProfile) -> Self {
        FrameMeter {
            profile,
            last: None,
            frames: 0,
            timed: 0,
            total_us: 0,
            max_us: 0,
            models: 0,
            tris: 0,
            pc_gpu_draws: 0,
            pc_gpu_tris: 0,
            instanced: 0,
            off: [0; PcGpuReject::COUNT],
        }
    }

    /// One finished frame: its counters (`Renderer::stats`) and the draws
    /// off the GPU path (`PcRenderer::pc_gpu_rejects`). The frame time is
    /// the wall clock since the previous call.
    pub fn record(&mut self, stats: &FrameStats, off: &[u32; PcGpuReject::COUNT]) {
        let now = Instant::now();
        if let Some(t) = self.last {
            let us = now.duration_since(t).as_micros() as u64;
            self.total_us += us;
            self.max_us = self.max_us.max(us);
            self.timed += 1;
        }
        self.last = Some(now);
        self.add(stats, off);
    }

    /// [`record`](Self::record) without the clock (tests).
    pub fn add(&mut self, stats: &FrameStats, off: &[u32; PcGpuReject::COUNT]) {
        self.frames += 1;
        self.models += stats.models as u64;
        self.tris += stats.tris as u64;
        self.pc_gpu_draws += stats.tnl.pc_gpu_draws as u64;
        self.pc_gpu_tris += stats.tnl.pc_gpu_tris as u64;
        self.instanced += stats.tnl.instanced as u64;
        for (o, n) in self.off.iter_mut().zip(off) {
            *o += *n as u64;
        }
    }

    /// The `TRINO_FRAME:<scene> ...` line (no newline): per-frame averages,
    /// `off_pc_gpu=` as `reason:draws` pairs, most common first (rounded
    /// up, so a reason seen at all shows), `-` when none.
    pub fn line(&self, scene: &str) -> String {
        let n = self.frames.max(1);
        let per = |v: u64| v / n;
        let avg = self.total_us.checked_div(self.timed).unwrap_or(0);
        let fps = 1_000_000u64.checked_div(avg).unwrap_or(0);
        let mut off: Vec<(PcGpuReject, u64)> = PcGpuReject::ALL
            .iter()
            .map(|r| (*r, self.off[r.index()].div_ceil(n)))
            .filter(|(_, k)| *k > 0)
            .collect();
        off.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let off = match off.is_empty() {
            true => "-".to_string(),
            false => off
                .iter()
                .map(|(r, k)| format!("{}:{k}", r.key()))
                .collect::<Vec<_>>()
                .join(","),
        };
        let profile = match self.profile {
            SimProfile::Native => "native",
            SimProfile::N64 => "n64",
            SimProfile::N3ds => "3ds",
        };
        format!(
            "TRINO_FRAME:{scene} profile={profile} avg_us={avg} max_us={} frames={} fps={fps} \
             draws={} tris={} pc_gpu_draws={} pc_gpu_tris={} instanced={} off_pc_gpu={off}",
            self.max_us,
            self.frames,
            per(self.models),
            per(self.tris),
            per(self.pc_gpu_draws),
            per(self.pc_gpu_tris),
            per(self.instanced),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_averages_per_frame_and_sorts_the_reasons() {
        let mut m = FrameMeter::new(SimProfile::Native);
        let mut stats = FrameStats {
            models: 10,
            tris: 300,
            ..FrameStats::default()
        };
        stats.tnl.pc_gpu_draws = 7;
        stats.tnl.pc_gpu_tris = 120;
        stats.tnl.instanced = 4;
        let mut off = [0; PcGpuReject::COUNT];
        off[PcGpuReject::Water.index()] = 1;
        off[PcGpuReject::Translucent.index()] = 2;
        m.add(&stats, &off);
        off[PcGpuReject::Translucent.index()] = 0;
        m.add(&stats, &off);
        let line = m.line("hub");
        assert!(
            line.starts_with("TRINO_FRAME:hub profile=native "),
            "{line}"
        );
        assert!(
            line.contains(
                "frames=2 fps=0 draws=10 tris=300 pc_gpu_draws=7 pc_gpu_tris=120 instanced=4 \
                 off_pc_gpu=water:1,translucent:1"
            ),
            "{line}"
        );
        let empty = FrameMeter::new(SimProfile::N64).line("x");
        assert!(
            empty.contains("profile=n64") && empty.ends_with("off_pc_gpu=-"),
            "{empty}"
        );
    }
}
