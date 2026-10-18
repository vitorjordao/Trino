//! Frame pacing (issue #55, ADR 0022 §4): the present mode a
//! [`FrameLimit`] asks for, and a sleep-based limiter for fixed caps.
//!
//! - `VSync` presents FIFO (the display's rate, no tearing).
//! - `Fps(n)` at or under the display's rate presents FIFO and sleeps to
//!   the cap (30 on a 60 Hz display shows each frame for two refreshes);
//!   over it (144 on a 60 Hz display) presents without waiting (mailbox,
//!   else immediate) and the limiter alone holds the cap.
//! - `Unlimited` presents mailbox, else immediate, else FIFO.
//!
//! The limiter keeps a cadence of deadlines `interval` apart and waits for
//! the next one after each frame; a frame later than a whole interval
//! restarts the cadence instead of rushing the frames after it. The wait
//! sleeps until 1.5 ms before the deadline and spins the rest: OS sleeps
//! overshoot by up to a millisecond (more on Windows), which alone would
//! miss a ±2% target at 120 fps.

use trino_core::FrameLimit;
use web_time::{Duration, Instant};

/// The present mode for `limit` on a display at `refresh_hz`, among the
/// surface's `supported` modes (FIFO is always among them).
pub fn present_mode(
    limit: FrameLimit,
    refresh_hz: u32,
    supported: &[wgpu::PresentMode],
) -> wgpu::PresentMode {
    use wgpu::PresentMode as M;
    let first = |modes: &[M]| {
        modes
            .iter()
            .copied()
            .find(|m| supported.contains(m))
            .unwrap_or(M::Fifo)
    };
    match limit {
        FrameLimit::VSync => M::Fifo,
        FrameLimit::Fps(n) if (n as u32) <= refresh_hz.max(1) => M::Fifo,
        FrameLimit::Fps(_) | FrameLimit::Unlimited => first(&[M::Mailbox, M::Immediate]),
    }
}

/// The sleep-based cap ([`FrameLimit::Fps`]); no-op for the other limits.
#[derive(Clone, Debug)]
pub struct FrameLimiter {
    interval: Option<Duration>,
    next: Option<Instant>,
}

impl FrameLimiter {
    pub fn new(limit: FrameLimit) -> Self {
        let mut l = FrameLimiter {
            interval: None,
            next: None,
        };
        l.set(limit);
        l
    }

    /// A new limit (the cadence restarts).
    pub fn set(&mut self, limit: FrameLimit) {
        let interval = match limit {
            FrameLimit::Fps(n) if n > 0 => Some(Duration::from_secs_f64(1.0 / n as f64)),
            _ => None,
        };
        if interval != self.interval {
            self.interval = interval;
            self.next = None;
        }
    }

    pub fn interval(&self) -> Option<Duration> {
        self.interval
    }

    /// A frame ended at `now`: the deadline to wait for before the next one
    /// starts (`None`: go on now).
    pub fn deadline(&mut self, now: Instant) -> Option<Instant> {
        let interval = self.interval?;
        let Some(next) = self.next else {
            self.next = Some(now + interval);
            return None;
        };
        if now >= next + interval {
            // A whole interval late: restart the cadence.
            self.next = Some(now + interval);
            return None;
        }
        self.next = Some(next + interval);
        (now < next).then_some(next)
    }

    /// [`deadline`](Self::deadline), then wait for it (native only: a
    /// browser can't block, its host skips animation frames instead).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn wait(&mut self) {
        if let Some(at) = self.deadline(Instant::now()) {
            sleep_until(at);
        }
    }
}

/// Sleep until 1.5 ms before `deadline`, then spin (yielding) to it.
#[cfg(not(target_arch = "wasm32"))]
pub fn sleep_until(deadline: Instant) {
    let margin = Duration::from_micros(1500);
    let now = Instant::now();
    if deadline > now + margin {
        std::thread::sleep(deadline - now - margin);
    }
    while Instant::now() < deadline {
        std::thread::yield_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::PresentMode as M;

    #[test]
    fn present_modes_follow_the_limit() {
        let all = [M::Fifo, M::Mailbox, M::Immediate];
        assert_eq!(present_mode(FrameLimit::VSync, 60, &all), M::Fifo);
        assert_eq!(present_mode(FrameLimit::Fps(30), 60, &all), M::Fifo);
        assert_eq!(present_mode(FrameLimit::Fps(60), 60, &all), M::Fifo);
        assert_eq!(present_mode(FrameLimit::Fps(144), 60, &all), M::Mailbox);
        assert_eq!(present_mode(FrameLimit::Unlimited, 144, &all), M::Mailbox);
        assert_eq!(
            present_mode(FrameLimit::Unlimited, 60, &[M::Fifo, M::Immediate]),
            M::Immediate
        );
        assert_eq!(present_mode(FrameLimit::Unlimited, 60, &[M::Fifo]), M::Fifo);
    }

    #[test]
    fn deadlines_keep_a_cadence_and_restart_when_far_behind() {
        let mut l = FrameLimiter::new(FrameLimit::Fps(50));
        let ms = Duration::from_millis;
        let t0 = Instant::now();
        assert_eq!(l.deadline(t0), None, "the first frame starts the cadence");
        // Fast frames wait for the cadence.
        assert_eq!(l.deadline(t0 + ms(5)), Some(t0 + ms(20)));
        assert_eq!(l.deadline(t0 + ms(25)), Some(t0 + ms(40)));
        // A frame 3 ms late (40 → 63): no wait, the cadence holds.
        assert_eq!(l.deadline(t0 + ms(63)), None);
        assert_eq!(l.deadline(t0 + ms(70)), Some(t0 + ms(80)));
        // A whole interval late: a new cadence from now.
        assert_eq!(l.deadline(t0 + ms(130)), None);
        assert_eq!(l.deadline(t0 + ms(135)), Some(t0 + ms(150)));
        // Other limits never wait.
        let mut v = FrameLimiter::new(FrameLimit::VSync);
        assert_eq!(v.deadline(t0), None);
        assert_eq!(v.deadline(t0 + ms(1)), None);
        l.set(FrameLimit::Unlimited);
        assert_eq!(l.interval(), None);
    }

    /// The acceptance bar: 30/60/120 fps caps within ±2% (real sleeps, a
    /// light frame).
    #[test]
    fn caps_hold_within_two_percent() {
        for fps in [30u16, 60, 120] {
            let mut l = FrameLimiter::new(FrameLimit::Fps(fps));
            let frames = (fps as u32 / 2).max(15);
            l.wait();
            let t0 = Instant::now();
            for _ in 0..frames {
                // A frame's work: ~1 ms.
                let w = Instant::now();
                while w.elapsed() < Duration::from_millis(1) {}
                l.wait();
            }
            let measured = frames as f64 / t0.elapsed().as_secs_f64();
            let err = (measured - fps as f64).abs() / fps as f64;
            assert!(err <= 0.02, "{fps} fps cap measured {measured:.2} fps");
        }
    }
}
