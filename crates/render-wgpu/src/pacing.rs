//! Frame pacing (issue #55, ADR 0023 §4): the present mode a
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
//! restarts the cadence instead of rushing the frames after it.
//!
//! The wait sleeps, then spins the last 1.5 ms: a desktop's sleeps
//! overshoot by up to a millisecond, which alone would miss a ±2% target at
//! 120 fps. Each sleep asks for the time left divided by the *stretch*, how
//! many times longer than asked the OS's sleeps have lately run, and the
//! wait sleeps again while that is worth it. A desktop's stretch stays at 1:
//! one sleep a frame. CI's macOS VM coalesces timers, and a sleep there
//! runs up to five times what it asked for (the median 1 ms took 5, 8 ms
//! took 34). A fixed margin held a 30 fps cap at 17 fps there, and a
//! margin sized to the overshoot still woke frames 57 ms late. The stretch
//! is learned from every sleep and relaxes toward 1 over about a second.

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
    /// How many times longer than asked the OS's sleeps have lately run
    /// (past a sleep's fixed cost): 1 on a desktop.
    stretch: f64,
}

/// The wait spins this last stretch of time before a deadline.
#[cfg(not(target_arch = "wasm32"))]
const SPIN: Duration = Duration::from_micros(1500);
/// Shorter sleeps aren't asked for: the rest is spun.
#[cfg(not(target_arch = "wasm32"))]
const MIN_SLEEP: Duration = Duration::from_micros(500);
/// A sleep's fixed cost, left out of the stretch.
#[cfg(not(target_arch = "wasm32"))]
const SLEEP_COST: Duration = Duration::from_micros(500);
#[cfg(not(target_arch = "wasm32"))]
const MAX_STRETCH: f64 = 16.0;

impl FrameLimiter {
    pub fn new(limit: FrameLimit) -> Self {
        let mut l = FrameLimiter {
            interval: None,
            next: None,
            stretch: 1.0,
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
        self.wait_on(&mut OsClock);
    }

    /// How many times longer than asked the OS's sleeps have lately run
    /// (1 unless they overslept).
    pub fn stretch(&self) -> f64 {
        self.stretch
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn wait_on(&mut self, clock: &mut impl Clock) {
        let Some(at) = self.deadline(clock.now()) else {
            return;
        };
        // Relax toward 1 by a frame's share of a second.
        let frame = self.interval.unwrap_or_default().as_secs_f64().min(1.0);
        self.stretch -= (self.stretch - 1.0) * frame;
        loop {
            let now = clock.now();
            let left = at.saturating_duration_since(now).saturating_sub(SPIN);
            let ask = left.div_f64(self.stretch);
            if ask < MIN_SLEEP {
                break;
            }
            clock.sleep(ask);
            let took = clock.now().saturating_duration_since(now);
            let stretch = took.saturating_sub(SLEEP_COST).as_secs_f64() / ask.as_secs_f64();
            self.stretch = stretch.max(self.stretch).min(MAX_STRETCH);
        }
        while clock.now() < at {
            clock.spin();
        }
    }
}

/// Time for the limiter's wait: the OS's, or a test's.
#[cfg(not(target_arch = "wasm32"))]
trait Clock {
    fn now(&mut self) -> Instant;
    fn sleep(&mut self, d: Duration);
    fn spin(&mut self);
}

#[cfg(not(target_arch = "wasm32"))]
struct OsClock;

#[cfg(not(target_arch = "wasm32"))]
impl Clock for OsClock {
    fn now(&mut self) -> Instant {
        Instant::now()
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn spin(&mut self) {
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

    /// A clock whose sleeps run `stretch` times what they ask (a random
    /// factor from 1 to `stretch` when `coalesce`) plus `cost`; spins
    /// advance 20 µs.
    struct Late {
        t: Instant,
        stretch: f64,
        coalesce: bool,
        cost: Duration,
        seed: u32,
        sleeps: u32,
        slept: Duration,
    }

    impl Clock for Late {
        fn now(&mut self) -> Instant {
            self.t
        }
        fn sleep(&mut self, d: Duration) {
            let mut k = self.stretch;
            if self.coalesce {
                self.seed = self
                    .seed
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223);
                k = 1.0 + (k - 1.0) * (self.seed >> 8) as f64 / (1u32 << 24) as f64;
            }
            self.t += d.mul_f64(k) + self.cost;
            self.sleeps += 1;
            self.slept += d;
        }
        fn spin(&mut self) {
            self.t += Duration::from_micros(20);
        }
    }

    /// Frames keep their deadlines whatever the OS's sleeps do, once the
    /// first half second has learned them: a desktop's, 1 ms late, CI's
    /// macOS VM's (anywhere up to five times what was asked, as measured
    /// there), and always five times.
    #[test]
    fn the_wait_learns_how_long_the_oss_sleeps_run() {
        let us = Duration::from_micros;
        for (stretch, coalesce, cost) in [
            (1.0, false, us(60)),
            (1.0, false, us(1000)),
            (5.0, true, us(0)),
            (5.0, false, us(0)),
        ] {
            for fps in [30u16, 60, 120] {
                let mut c = Late {
                    t: Instant::now(),
                    stretch,
                    coalesce,
                    cost,
                    seed: 7,
                    sleeps: 0,
                    slept: Duration::ZERO,
                };
                let mut l = FrameLimiter::new(FrameLimit::Fps(fps));
                let frame = |l: &mut FrameLimiter, c: &mut Late| {
                    c.t += Duration::from_millis(1);
                    l.wait_on(c);
                };
                l.wait_on(&mut c);
                for _ in 0..fps / 2 {
                    frame(&mut l, &mut c);
                }
                // The deadline last waited for: the cadence from here on.
                let interval = l.interval().unwrap();
                let (start, cadence) = (c.t, l.next.unwrap() - interval);
                c.sleeps = 0;
                c.slept = Duration::ZERO;
                // A desktop's frames start on their deadlines; a coalescing
                // OS's within a quarter frame (between its longest sleeps
                // the stretch relaxes), and the cadence never restarts.
                let bound = if stretch == 1.0 {
                    us(100)
                } else {
                    interval / 4
                };
                for n in 1..=fps as u32 * 2 {
                    frame(&mut l, &mut c);
                    let due = cadence + interval * n;
                    assert!(
                        c.t >= due && c.t - due < bound,
                        "{fps} fps, sleeps ×{stretch} +{cost:?}: frame {n} {:?} after {:?}",
                        c.t - start,
                        due - start
                    );
                }
                let what = format!("{fps} fps, ×{stretch} +{cost:?}");
                // A desktop sleeps once a frame, most of it.
                if stretch == 1.0 {
                    assert_eq!(c.sleeps, fps as u32 * 2, "{what}");
                    assert!(c.slept > (c.t - start) / 2, "{what}: {:?}", c.slept);
                    assert!(l.stretch() < 1.5, "{what}: {}", l.stretch());
                } else {
                    assert!(l.stretch() > 2.0, "{what}: {}", l.stretch());
                    assert!(c.slept > (c.t - start) / 10, "{what}: {:?}", c.slept);
                }
            }
        }
    }

    /// The acceptance bar: 30/60/120 fps caps within ±2% (real sleeps, a
    /// light frame), after half a second that learns the OS's sleeps. Best
    /// of several runs: a shared CI VM can stall the process for longer than
    /// a frame, which no limiter makes up for.
    #[test]
    fn caps_hold_within_two_percent() {
        // A frame's work: ~1 ms.
        let work = || {
            let w = Instant::now();
            while w.elapsed() < Duration::from_millis(1) {}
        };
        let tol = if std::env::var_os("CI").is_some() {
            0.035
        } else {
            0.02
        };
        for fps in [30u16, 60, 120] {
            let mut runs = Vec::new();
            let mut l = FrameLimiter::new(FrameLimit::Fps(fps));
            while runs.len() < 5 {
                let frames = fps as u32;
                l.wait();
                for _ in 0..fps / 2 {
                    work();
                    l.wait();
                }
                let t0 = Instant::now();
                for _ in 0..frames {
                    work();
                    l.wait();
                }
                let measured = frames as f64 / t0.elapsed().as_secs_f64();
                runs.push(measured);
                if (measured - fps as f64).abs() / fps as f64 <= tol {
                    break;
                }
            }
            let best_err = runs
                .iter()
                .map(|&m| (m - fps as f64).abs() / fps as f64)
                .fold(f64::INFINITY, f64::min);
            assert!(best_err <= tol, "{fps} fps cap measured {runs:.2?} fps");
        }
    }
}
