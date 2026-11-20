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
//!
//! **Where the frame goes** (hosts that time the game's stages,
//! [`FrameMeter::record_stages`]): the line adds the frame time's
//! percentiles and the spike count (`p50_us= p95_us= p99_us= spikes=`), a
//! `TRINO_SPIKES:` line names the worst frames and the stage that grew in
//! each ([`FrameMeter::spikes_line`]), and `TRINO_FRAME_LOG=<file.csv>`
//! writes one row per frame ([`FrameLog`]) for a spreadsheet or a plot.

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
    /// Frames that drew two eyes and the last frame's stereo (the 3DS
    /// profile's preview, issue #63).
    stereo_frames: u64,
    stereo: trino_core::StereoStats,
    /// Frames the post stack drew (issue #64) and the last one's bloom
    /// levels.
    post_frames: u64,
    bloom_levels: u32,
    /// Every staged frame ([`FrameMeter::record_stages`]), bounded like
    /// `samples`.
    stages: Vec<FrameSample>,
    last_us: u32,
}

/// One frame's stages, µs, as a host measured them: what
/// `TRINO_FRAME_LOG` writes and `TRINO_SPIKES` explains.
///
/// `render_us` is the game's whole `render` call, the renderer's
/// `end_frame` included, so `render_us - tnl_us - batch_us - submit_us` is
/// the game's own drawing code. `gpu_us` is the last GPU frame time the
/// timestamp queries returned (a frame or two late).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameSample {
    pub frame: u64,
    /// Wall clock since the previous frame (what the player feels).
    pub frame_us: u32,
    pub update_us: u32,
    pub render_us: u32,
    /// The renderer's CPU T&L, batch (painter sort and expansion) and
    /// submit (`PcFrameTimes`).
    pub tnl_us: u32,
    pub batch_us: u32,
    pub submit_us: u32,
    pub gpu_us: u32,
    pub draws: u32,
    pub tris: u32,
    /// Draws off the GPU T&L path (every [`PcGpuReject`]).
    pub cpu_draws: u32,
    /// Pipelines compiled inside the frame (hitches).
    pub pipelines: u32,
}

/// A spike's stages, in the order `TRINO_SPIKES` names them.
const STAGES: [&str; 6] = ["update", "render", "tnl", "batch", "submit", "gpu"];

impl FrameSample {
    /// The CSV header `TRINO_FRAME_LOG` starts with.
    pub const CSV_HEADER: &'static str = "frame,frame_us,update_us,render_us,tnl_us,batch_us,\
         submit_us,gpu_us,draws,tris,cpu_draws,pipelines";

    /// The row (no newline).
    pub fn csv_row(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            self.frame,
            self.frame_us,
            self.update_us,
            self.render_us,
            self.tnl_us,
            self.batch_us,
            self.submit_us,
            self.gpu_us,
            self.draws,
            self.tris,
            self.cpu_draws,
            self.pipelines
        )
    }

    /// The stages [`STAGES`] names, µs. `render` is the game's own share
    /// (the renderer's three stages taken out), so the six don't overlap.
    fn stages(&self) -> [u32; 6] {
        let renderer = self.tnl_us + self.batch_us + self.submit_us;
        [
            self.update_us,
            self.render_us.saturating_sub(renderer),
            self.tnl_us,
            self.batch_us,
            self.submit_us,
            self.gpu_us,
        ]
    }
}

/// `TRINO_FRAME_LOG=<file.csv>`: one [`FrameSample`] row per frame,
/// written as the game runs (a crash keeps what came before).
pub struct FrameLog {
    out: std::io::BufWriter<std::fs::File>,
}

impl FrameLog {
    /// The file `TRINO_FRAME_LOG` names, created with its header; `None`
    /// without the variable (an unwritable path says why on stderr).
    pub fn from_env() -> Option<FrameLog> {
        let path = std::env::var_os("TRINO_FRAME_LOG")?;
        match FrameLog::create(std::path::Path::new(&path)) {
            Ok(log) => Some(log),
            Err(e) => {
                eprintln!("TRINO_FRAME_LOG {}: {e}", path.to_string_lossy());
                None
            }
        }
    }

    pub fn create(path: &std::path::Path) -> std::io::Result<FrameLog> {
        use std::io::Write as _;
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(out, "{}", FrameSample::CSV_HEADER)?;
        Ok(FrameLog { out })
    }

    pub fn write(&mut self, s: &FrameSample) {
        use std::io::Write as _;
        let _ = writeln!(self.out, "{}", s.csv_row());
    }

    pub fn flush(&mut self) {
        use std::io::Write as _;
        let _ = self.out.flush();
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        self.flush();
    }
}

/// The value at quantile `q` (0..=1) of sorted `v` (nearest rank).
fn quantile(v: &[u32], q: f32) -> u32 {
    if v.is_empty() {
        return 0;
    }
    let i = ((v.len() as f32 * q).ceil() as usize).clamp(1, v.len()) - 1;
    v[i]
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
            stereo_frames: 0,
            stereo: trino_core::StereoStats::default(),
            post_frames: 0,
            bloom_levels: 0,
            stages: Vec::new(),
            last_us: 0,
        }
    }

    /// One frame's stages ([`FrameSample`]): the line's percentiles and
    /// spikes, `TRINO_SPIKES`.
    pub fn record_stages(&mut self, s: FrameSample) {
        if self.stages.len() >= Self::MAX_SAMPLES {
            self.stages.drain(..Self::MAX_SAMPLES / 2);
        }
        self.stages.push(s);
    }

    /// The staged frames' frame times, sorted.
    fn sorted_frame_us(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.stages.iter().map(|s| s.frame_us).collect();
        v.sort_unstable();
        v
    }

    /// Whether a frame of `us` is a spike against the median `p50`: half
    /// again as long **and** 4 ms over (a 60 fps game's 25 ms frame is one;
    /// a 300 fps game's 5 ms frame is noise, not a stutter).
    pub fn is_spike(us: u32, p50: u32) -> bool {
        us as u64 * 2 > p50 as u64 * 3 && us > p50.saturating_add(4_000)
    }

    /// The staged frames that are spikes ([`Self::is_spike`]), slowest
    /// first, and the median frame they are judged against.
    pub fn spikes(&self) -> (Vec<FrameSample>, u32) {
        let p50 = quantile(&self.sorted_frame_us(), 0.5);
        let mut out: Vec<FrameSample> = self
            .stages
            .iter()
            .filter(|s| Self::is_spike(s.frame_us, p50))
            .copied()
            .collect();
        out.sort_by(|a, b| b.frame_us.cmp(&a.frame_us).then(a.frame.cmp(&b.frame)));
        (out, p50)
    }

    /// `TRINO_SPIKES: count= median_us= worst=` and the `worst` slowest
    /// frames as `frame:us(cause=stage +us; update=… render=… tnl=… batch=…
    /// submit=… gpu=… draws=… cpu_draws=… pipes=…)`: the cause is the stage
    /// that grew most over its own median, so a spike says whether the
    /// game, the CPU T&L, the sort, the submit or the GPU took the time.
    /// `None` without staged frames.
    pub fn spikes_line(&self, worst: usize) -> Option<String> {
        if self.stages.is_empty() {
            return None;
        }
        let (spikes, p50) = self.spikes();
        let medians: Vec<u32> = (0..STAGES.len())
            .map(|i| {
                let mut v: Vec<u32> = self.stages.iter().map(|s| s.stages()[i]).collect();
                v.sort_unstable();
                quantile(&v, 0.5)
            })
            .collect();
        let shown: Vec<String> = spikes
            .iter()
            .take(worst)
            .map(|s| {
                let st = s.stages();
                let (cause, over) = st
                    .iter()
                    .zip(&medians)
                    .enumerate()
                    .map(|(i, (v, m))| (i, v.saturating_sub(*m)))
                    .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                    .unwrap_or((0, 0));
                let parts: Vec<String> = STAGES
                    .iter()
                    .zip(st)
                    .map(|(n, v)| format!("{n}={v}"))
                    .collect();
                format!(
                    "{}:{}(cause={} +{over}; {} draws={} cpu_draws={} pipes={})",
                    s.frame,
                    s.frame_us,
                    STAGES[cause],
                    parts.join(" "),
                    s.draws,
                    s.cpu_draws,
                    s.pipelines
                )
            })
            .collect();
        Some(format!(
            "TRINO_SPIKES: count={} median_us={p50} worst={}",
            spikes.len(),
            if shown.is_empty() {
                "-".to_string()
            } else {
                shown.join(",")
            }
        ))
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

    /// A frame's post stack (`PcFrameTimes::post_passes`,
    /// `bloom_levels`): the line's `post_frames=` and `bloom=`, shown once
    /// a frame used it.
    pub fn record_post(&mut self, passes: u32, bloom_levels: u32) {
        if passes > 0 {
            self.post_frames += 1;
            self.bloom_levels = bloom_levels;
        }
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
        self.last_us = 0;
        if let Some(t) = self.last {
            let us = now.duration_since(t).as_micros() as u64;
            self.last_us = us.min(u32::MAX as u64) as u32;
            self.add_time(us);
        }
        self.last = Some(now);
        self.add(stats, off);
    }

    /// The frame time [`record`](Self::record) last measured, µs (0 for
    /// the first frame).
    pub fn last_frame_us(&self) -> u32 {
        self.last_us
    }

    /// [`record`](Self::record) without the clock (tests).
    pub fn add(&mut self, stats: &FrameStats, off: &[u32; PcGpuReject::COUNT]) {
        self.frames += 1;
        self.models += stats.models as u64;
        self.tris += stats.tris as u64;
        self.pc_gpu_draws += stats.tnl.pc_gpu_draws as u64;
        self.pc_gpu_tris += stats.tnl.pc_gpu_tris as u64;
        self.instanced += stats.tnl.instanced as u64;
        self.stereo = stats.stereo;
        self.stereo_frames += (stats.stereo.eyes == 2) as u64;
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
        // The 3DS profile says how many frames drew two eyes, and why the
        // last one did or didn't (TRINO_STEREO previews them).
        let stereo = match self.profile {
            SimProfile::N3ds => {
                let mut f = String::new();
                let _ = self.stereo.write(&mut f);
                format!(" stereo_frames={} {f}", self.stereo_frames)
            }
            _ => String::new(),
        };
        let post = match self.post_frames {
            0 => String::new(),
            n => format!(" post_frames={n} bloom={}", self.bloom_levels),
        };
        let low1 = self.low1_us();
        let low1_fps = 1_000_000u64.checked_div(low1).unwrap_or(0);
        let percentiles = match self.stages.is_empty() {
            true => String::new(),
            false => {
                let v = self.sorted_frame_us();
                format!(
                    " p50_us={} p95_us={} p99_us={} spikes={}",
                    quantile(&v, 0.5),
                    quantile(&v, 0.95),
                    quantile(&v, 0.99),
                    self.spikes().0.len()
                )
            }
        };
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
             low1_us={low1} low1_fps={low1_fps} pipes={}{percentiles}{quality}{stereo}{post}",
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
    fn the_3ds_profile_reports_its_stereo_frames() {
        let mut m = FrameMeter::new(SimProfile::N3ds);
        let mut stats = FrameStats::default();
        stats.stereo = trino_core::StereoFrame::resolve(
            Some(&trino_core::StereoSettings::new(0.1, 4.0).with_slider(0.5)),
            true,
            0.0,
            true,
        )
        .stats(3);
        m.add(&stats, &[0; PcGpuReject::COUNT]);
        stats.stereo = trino_core::StereoFrame::mono(trino_core::StereoMono::Slider).stats(0);
        m.add(&stats, &[0; PcGpuReject::COUNT]);
        let line = m.line("hub");
        assert!(line.ends_with(" stereo_frames=1 stereo=slider0"), "{line}");
        let native = FrameMeter::new(SimProfile::Native).line("hub");
        assert!(!native.contains("stereo"), "{native}");
    }

    #[test]
    fn the_post_stack_shows_once_a_frame_used_it() {
        let mut m = FrameMeter::new(SimProfile::Native);
        m.record_post(0, 0);
        assert!(!m.line("hub").contains("post"));
        m.record_post(1, 5);
        m.record_post(1, 4);
        assert!(
            m.line("hub").ends_with(" post_frames=2 bloom=4"),
            "{}",
            m.line("hub")
        );
    }

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
        off[PcGpuReject::TextureAlpha.index()] = 1;
        off[PcGpuReject::Offscreen.index()] = 2;
        m.add(&stats, &off);
        off[PcGpuReject::Offscreen.index()] = 0;
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
                 off_pc_gpu=offscreen:1,texture_alpha:1 heap=900/1200"
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

    fn staged(frame: u64, frame_us: u32, tnl_us: u32) -> FrameSample {
        FrameSample {
            frame,
            frame_us,
            update_us: 500,
            render_us: 4_000 + tnl_us,
            tnl_us,
            batch_us: 1_000,
            submit_us: 1_500,
            gpu_us: 6_000,
            draws: 300,
            tris: 90_000,
            cpu_draws: 20,
            pipelines: 0,
        }
    }

    /// Percentiles and spikes come from the staged frames; a spike names
    /// the stage that grew (here the CPU T&L), the slowest first.
    #[test]
    fn spikes_name_the_stage_that_grew() {
        let mut m = FrameMeter::new(SimProfile::Native);
        assert_eq!(m.spikes_line(3), None);
        assert!(!m.line("hub").contains("p50_us"));
        for i in 0..100 {
            let s = match i {
                40 => staged(i, 31_000, 22_000),
                70 => staged(i, 26_000, 17_000),
                // 1.5x but under 4 ms over: not a spike at a fast rate.
                80 => staged(i, 12_000, 3_000),
                _ => staged(i, 8_000, 1_000),
            };
            m.record_stages(s);
        }
        let line = m.line("hub");
        assert!(
            line.contains(" p50_us=8000 p95_us=8000 p99_us=26000 spikes=2"),
            "{line}"
        );
        let spikes = m.spikes_line(1).unwrap();
        assert!(
            spikes.starts_with(
                "TRINO_SPIKES: count=2 median_us=8000 worst=40:31000(cause=tnl +21000; \
                 update=500 render=1500 tnl=22000 batch=1000 submit=1500 gpu=6000"
            ),
            "{spikes}"
        );
        assert!(!spikes.contains("70:"), "{spikes}");
        assert!(
            m.spikes_line(5)
                .unwrap()
                .contains(",70:26000(cause=tnl +16000")
        );
        assert!(FrameMeter::is_spike(25_000, 16_600));
        assert!(!FrameMeter::is_spike(19_000, 16_600));
        assert!(!FrameMeter::is_spike(7_000, 3_000));
    }

    /// `TRINO_FRAME_LOG` writes a header and one row per frame, in the
    /// header's order.
    #[test]
    fn the_frame_log_writes_csv_rows() {
        let dir = std::env::temp_dir().join(format!("trino-frame-log-{}", std::process::id()));
        let path = dir.join("frames.csv");
        {
            let mut log = FrameLog::create(&path).unwrap();
            log.write(&staged(7, 9_000, 1_200));
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let mut lines = text.lines();
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();
        let row: Vec<&str> = lines.next().unwrap().split(',').collect();
        assert_eq!(header.len(), row.len());
        let field = |name: &str| row[header.iter().position(|h| *h == name).unwrap()];
        assert_eq!(field("frame"), "7");
        assert_eq!(field("frame_us"), "9000");
        assert_eq!(field("tnl_us"), "1200");
        assert_eq!(field("render_us"), "5200");
        assert_eq!(field("cpu_draws"), "20");
        assert_eq!(lines.next(), None);
    }
}
