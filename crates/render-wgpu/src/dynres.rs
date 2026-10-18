//! Dynamic resolution (issue #55, ADR 0023 §3): a PI controller that moves
//! the 3D's render scale so the GPU's frame time holds a target.
//!
//! It works in the log of the rendered area (`scale²`): a frame's GPU time
//! is roughly a fixed part plus a part proportional to the pixels drawn, so
//! a ratio of times maps onto a ratio of areas. Every [`DynResController::PERIOD`]
//! frames it averages the measured times, takes the error
//! `e = ln(target / measured)` and steps the log-area by
//! `KI·e + KP·(e − e_prev)` (the velocity form: no integral to wind up when
//! the scale sits at a limit). A dead band of ±3% keeps it from hunting
//! around a target it already holds.
//!
//! The measurement is the GPU time of the frame where the adapter has
//! timestamp queries, else the host's frame interval
//! ([`crate::PcRenderer::report_frame_time`]). Under VSync an interval
//! can't show headroom (it never drops below the refresh), so on that
//! fallback a frame at the target probes 2% up per period and a missed
//! frame brings it back down.

/// The render scale's controller. Pure: the renderer feeds it times and
/// reads [`scale`](Self::scale).
#[derive(Clone, Debug)]
pub struct DynResController {
    target_us: f32,
    min: f32,
    max: f32,
    /// ln(scale²).
    log_area: f32,
    sum_us: f64,
    frames: u32,
    last_error: f32,
    /// Measurements are frame intervals, not GPU time (see the module doc).
    intervals: bool,
}

impl DynResController {
    /// Frames averaged per update.
    pub const PERIOD: u32 = 8;
    pub const KI: f32 = 0.55;
    pub const KP: f32 = 0.25;
    /// Errors smaller than this (3%) change nothing.
    pub const DEAD_BAND: f32 = 0.03;
    /// The largest step of the log-area per update (×1.35 / ÷1.35 area).
    pub const MAX_STEP: f32 = 0.3;

    /// A controller holding `target_us`, the scale within `min..=max`,
    /// starting at `start`.
    pub fn new(target_us: u32, min: f32, max: f32, start: f32) -> Self {
        let (min, max) = (min.max(0.05), max.max(min.max(0.05)));
        let s = start.clamp(min, max);
        DynResController {
            target_us: target_us.max(1) as f32,
            min,
            max,
            log_area: (s * s).ln(),
            sum_us: 0.0,
            frames: 0,
            last_error: 0.0,
            intervals: false,
        }
    }

    /// The measurements are frame intervals (no GPU timer): see the module
    /// doc.
    pub fn with_intervals(mut self, on: bool) -> Self {
        self.intervals = on;
        self
    }

    /// [`with_intervals`](Self::with_intervals) in place.
    pub fn set_intervals(&mut self, on: bool) {
        self.intervals = on;
    }

    pub fn target_us(&self) -> u32 {
        self.target_us as u32
    }

    /// Change the target (a new frame limit) without losing the scale.
    pub fn set_target(&mut self, target_us: u32) {
        self.target_us = target_us.max(1) as f32;
    }

    /// Change the range; the scale is clamped into it.
    pub fn set_range(&mut self, min: f32, max: f32) {
        let s = self.scale();
        self.min = min.max(0.05);
        self.max = max.max(self.min);
        let s = s.clamp(self.min, self.max);
        self.log_area = (s * s).ln();
    }

    pub fn range(&self) -> (f32, f32) {
        (self.min, self.max)
    }

    /// The render scale now.
    pub fn scale(&self) -> f32 {
        (self.log_area * 0.5).exp().clamp(self.min, self.max)
    }

    /// One frame's measured time. Returns `true` when this frame closed a
    /// period and the scale may have moved.
    pub fn record(&mut self, frame_us: u32) -> bool {
        self.sum_us += frame_us as f64;
        self.frames += 1;
        if self.frames < Self::PERIOD {
            return false;
        }
        let mean = (self.sum_us / self.frames as f64).max(1.0) as f32;
        self.sum_us = 0.0;
        self.frames = 0;
        self.update(mean);
        true
    }

    fn update(&mut self, mean_us: f32) {
        let mut e = (self.target_us / mean_us).ln();
        if self.intervals && e > -Self::DEAD_BAND {
            // An interval at (or under) the target can't show headroom:
            // probe up slowly.
            e = 0.02 / Self::KI;
        } else if e.abs() < Self::DEAD_BAND {
            self.last_error = 0.0;
            return;
        }
        let step = (Self::KI * e + Self::KP * (e - self.last_error))
            .clamp(-Self::MAX_STEP, Self::MAX_STEP);
        self.last_error = e;
        let (lo, hi) = ((self.min * self.min).ln(), (self.max * self.max).ln());
        self.log_area = (self.log_area + step).clamp(lo, hi);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GPU-bound frame: a fixed part plus a part per pixel, with ±3%
    /// deterministic noise.
    fn model(scale: f32, fixed: f32, per_area: f32, i: u32) -> u32 {
        let noise = 1.0 + 0.03 * ((i as f32 * 1.7).sin());
        ((fixed + per_area * scale * scale) * noise) as u32
    }

    /// Run the controller on the model; returns the scales and times of
    /// the last 240 frames.
    fn run(
        c: &mut DynResController,
        fixed: f32,
        per_area: f32,
        frames: u32,
    ) -> (Vec<f32>, Vec<u32>) {
        let (mut scales, mut times) = (Vec::new(), Vec::new());
        for i in 0..frames {
            let t = model(c.scale(), fixed, per_area, i);
            c.record(t);
            if i + 240 >= frames {
                scales.push(c.scale());
                times.push(t);
            }
        }
        (scales, times)
    }

    #[test]
    fn converges_on_the_target_and_never_oscillates_beyond_5_percent() {
        // 16.6 ms target; the frame costs 2 ms + 20 ms × area: the answer is
        // scale √(14.667/20) = 0.856.
        let target = 16_667;
        for start in [0.5, 1.0, 2.0] {
            let mut c = DynResController::new(target, 0.25, 2.0, start);
            let (scales, times) = run(&mut c, 2_000.0, 20_000.0, 900);
            let want = ((target as f32 - 2_000.0) / 20_000.0).sqrt();
            let (lo, hi) = scales
                .iter()
                .fold((f32::MAX, f32::MIN), |(a, b), s| (a.min(*s), b.max(*s)));
            assert!(
                (lo - want).abs() / want <= 0.05 && (hi - want).abs() / want <= 0.05,
                "start {start}: scale {lo:.3}..{hi:.3}, want {want:.3}"
            );
            let mean = times.iter().map(|t| *t as f64).sum::<f64>() / times.len() as f64;
            assert!(
                (mean - target as f64).abs() <= 1_000.0,
                "start {start}: mean frame {mean:.0} µs"
            );
            // Steady: at most ±5% around its mean scale.
            let m = scales.iter().sum::<f32>() / scales.len() as f32;
            assert!(scales.iter().all(|s| (s - m).abs() / m <= 0.05));
        }
    }

    #[test]
    fn it_follows_a_heavier_scene_and_stops_at_its_limits() {
        let mut c = DynResController::new(16_667, 0.5, 1.0, 1.0);
        // Light: stays at the top.
        run(&mut c, 1_000.0, 5_000.0, 200);
        assert_eq!(c.scale(), 1.0);
        // Far too heavy even at the bottom: stays at the floor.
        run(&mut c, 30_000.0, 40_000.0, 200);
        assert_eq!(c.scale(), 0.5);
        // Back to something it can hold.
        let (scales, _) = run(&mut c, 2_000.0, 25_000.0, 600);
        let want = (14_667.0f32 / 25_000.0).sqrt();
        assert!((scales.last().unwrap() - want).abs() / want < 0.05);
        // Ranges and targets change without jumps.
        c.set_range(0.9, 1.0);
        assert_eq!(c.scale(), 0.9);
        c.set_target(33_333);
        assert_eq!(c.target_us(), 33_333);
    }

    #[test]
    fn interval_measurements_probe_up_and_fall_back_on_a_miss() {
        // VSync at 60 Hz: an interval is 16.7 ms whatever the GPU does, or
        // 33.3 ms once the GPU misses (the area past 0.8 doesn't fit).
        let interval = |s: f32| if s * s > 0.64 { 33_333 } else { 16_667 };
        let mut c = DynResController::new(16_667, 0.5, 1.0, 0.5).with_intervals(true);
        let mut seen = Vec::new();
        for _ in 0..2_000 {
            c.record(interval(c.scale()));
            seen.push(c.scale());
        }
        let tail = &seen[seen.len() - 400..];
        assert!(
            tail.iter().all(|s| *s <= 0.9),
            "it backs off a missed frame"
        );
        assert!(tail.iter().any(|s| *s > 0.7), "it climbs toward the limit");
    }

    #[test]
    fn the_start_scale_is_clamped() {
        assert_eq!(DynResController::new(10_000, 0.5, 1.0, 3.0).scale(), 1.0);
        assert_eq!(DynResController::new(10_000, 0.5, 1.0, 0.1).scale(), 0.5);
        let c = DynResController::new(10_000, 1.2, 0.8, 1.0);
        assert!(c.range().0 <= c.range().1);
    }
}
