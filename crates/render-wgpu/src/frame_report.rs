//! `TRINO_FRAME:` lines on the PC, like the consoles' hosts print them
//! (`N64Host::report_frames`, `N3dsHost::report_frames`): frame time and,
//! per frame, the 3D draws and how the native profile's GPU T&L path splits
//! them (issue #51: `pc_gpu_draws`, `pc_gpu_tris`, instanced copies and why
//! the rest stay on the CPU path, most common first).
//!
//! `apps/pc` and `trino-runner-pc` record every frame and print the line
//! when a `TRINO_SMOKE_FRAMES` run ends. After `off_pc_gpu=` comes
//! `heap=used/peak` (issue #56: the host's counting allocator,
//! `Platform::memory`), as on the consoles. Since issue #55 the line also
//! carries the 1% lows (`low1_us`: the mean of the slowest 1% of frames,
//! `low1_fps` its rate) and, where the host set one, the quality in effect
//! (`quality= scale= quality_from= detected= gpu_us=`).

use web_time::Instant;

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
    /// The last [`FrameMeter::record_memory`]: heap used and peak.
    heap: (u32, u32),
    /// Every timed frame's µs (the 1% lows).
    samples: Vec<u32>,
    quality: Option<QualityNote>,
    /// Pipelines compiled inside frames (issue #101): hitches.
    pipes: u64,
}

/// The quality a host reports on the line ([`FrameMeter::set_quality`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QualityNote {
    pub preset: trino_core::QualityPreset,
    /// The 3D's render scale at the end of the run.
    pub scale: f32,
    pub source: trino_core::QualitySource,
    /// The calibration's pick, when it ran.
    pub detected: Option<trino_core::QualityPreset>,
    /// The last GPU frame time, µs.
    pub gpu_us: u32,
}

impl FrameMeter {
    /// Frame times kept for the 1% lows (about half an hour at 60 fps).
    const MAX_SAMPLES: usize = 1 << 17;

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
            heap: (0, 0),
            samples: Vec::new(),
            quality: None,
            pipes: 0,
        }
    }

    /// What the line says about the quality (the host's latest).
    pub fn set_quality(&mut self, note: QualityNote) {
        self.quality = Some(note);
    }

    /// One timed frame's µs, without the counters (tests, hosts that time
    /// frames themselves).
    pub fn add_time(&mut self, us: u64) {
        self.total_us += us;
        self.max_us = self.max_us.max(us);
        self.timed += 1;
        if self.samples.len() >= Self::MAX_SAMPLES {
            // Long sessions: the 1% lows of the latest frames.
            self.samples.drain(..Self::MAX_SAMPLES / 2);
        }
        self.samples.push(us.min(u32::MAX as u64) as u32);
    }

    /// The mean of the slowest 1% of the timed frames (at least one), µs;
    /// 0 before any.
    pub fn low1_us(&self) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let mut s = self.samples.clone();
        s.sort_unstable_by(|a, b| b.cmp(a));
        let n = s.len().div_ceil(100);
        s[..n].iter().map(|v| *v as u64).sum::<u64>() / n as u64
    }

    /// The pipelines a frame compiled on the render thread
    /// (`PcFrameTimes::pipelines`): the line's `pipes=`, a total.
    pub fn record_pipes(&mut self, n: u32) {
        self.pipes += n as u64;
    }

    /// The heap after a frame (`Platform::memory`): the line's `heap=`.
    pub fn record_memory(&mut self, memory: &trino_core::MemoryReport) {
        self.heap = (memory.heap_used, memory.heap_peak.max(self.heap.1));
    }

    /// One finished frame: its counters (`Renderer::stats`) and the draws
    /// off the GPU path (`PcRenderer::pc_gpu_rejects`). The frame time is
    /// the wall clock since the previous call.
    pub fn record(&mut self, stats: &FrameStats, off: &[u32; PcGpuReject::COUNT]) {
        let now = Instant::now();
        if let Some(t) = self.last {
            self.add_time(now.duration_since(t).as_micros() as u64);
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
            SimProfile::Web => "web",
        };
        let low1 = self.low1_us();
        let low1_fps = 1_000_000u64.checked_div(low1).unwrap_or(0);
        let quality = match self.quality {
            None => String::new(),
            Some(q) => format!(
                " quality={} scale={:.2} quality_from={} detected={} gpu_us={}",
                q.preset.key(),
                q.scale,
                q.source.key(),
                q.detected.map_or("-", |p| p.key()),
                q.gpu_us
            ),
        };
        format!(
            "TRINO_FRAME:{scene} profile={profile} avg_us={avg} max_us={} frames={} fps={fps} \
             draws={} tris={} pc_gpu_draws={} pc_gpu_tris={} instanced={} off_pc_gpu={off} {} \
             low1_us={low1} low1_fps={low1_fps} pipes={}{quality}",
            self.max_us,
            self.frames,
            per(self.models),
            per(self.tris),
            per(self.pc_gpu_draws),
            per(self.pc_gpu_tris),
            per(self.instanced),
            trino_core::group::HeapField(self.heap.0, self.heap.1),
            self.pipes,
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
        m.record_memory(&trino_core::MemoryReport {
            heap_used: 900,
            heap_peak: 1200,
            ..Default::default()
        });
        let line = m.line("hub");
        assert!(
            line.starts_with("TRINO_FRAME:hub profile=native "),
            "{line}"
        );
        assert!(
            line.contains(
                "frames=2 fps=0 draws=10 tris=300 pc_gpu_draws=7 pc_gpu_tris=120 instanced=4 \
                 off_pc_gpu=water:1,translucent:1 heap=900/1200"
            ),
            "{line}"
        );
        let empty = FrameMeter::new(SimProfile::N64).line("x");
        assert!(
            empty.contains("profile=n64")
                && empty.ends_with("off_pc_gpu=- heap=0/0 low1_us=0 low1_fps=0 pipes=0"),
            "{empty}"
        );
    }

    #[test]
    fn one_percent_lows_and_the_quality_note() {
        let mut m = FrameMeter::new(SimProfile::Native);
        // 198 frames at 10 ms and two hitches: the slowest 1% is those two.
        for i in 0..200 {
            m.add_time(if i == 50 || i == 150 { 40_000 } else { 10_000 });
        }
        assert_eq!(m.low1_us(), 40_000);
        m.add_time(30_000); // 201 frames: the slowest 3.
        assert_eq!(m.low1_us(), (40_000 + 40_000 + 30_000) / 3);
        m.set_quality(QualityNote {
            preset: trino_core::QualityPreset::Medium,
            scale: 0.8,
            source: trino_core::QualitySource::Detected,
            detected: Some(trino_core::QualityPreset::Medium),
            gpu_us: 12_345,
        });
        let line = m.line("hub");
        assert!(
            line.contains("low1_us=36666 low1_fps=27 pipes=0 quality=medium scale=0.80 quality_from=detected detected=medium gpu_us=12345"),
            "{line}"
        );
    }
}
