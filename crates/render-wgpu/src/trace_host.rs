//! The PC's and the Web's side of the timeline trace (issue #62, ADR 0030):
//! one struct both hosts hold, so they record and export alike.
//!
//! [`TraceHost::start`] installs the global ring of [`trino_core::trace`]
//! (the engine's zones: the game loop, the renderer's stages, the GPU passes
//! from [`crate::renderer`]) when the build has `trino-core/trace`; each
//! [`TraceHost::frame`] hands what the ring holds to the host's sink as
//! `TRINO_TRACE:` lines (a file on the PC, the console on the Web) before it
//! fills, and [`TraceHost::finish`] the rest. `cargo xtask trace collect`
//! reads them like any console's.
//!
//! Other threads (the T&L workers, the audio stream, the group loader's
//! decoders) record on tracks of their own through [`thread_hooks`]: a
//! thread that calls [`trace::bind_thread`] gets a lane of its own, kept in
//! a `thread_local!` here (issue #130).

use std::cell::Cell;
use trino_core::trace::{self, GLOBAL_EVENTS, GlobalRing, Lane, ThreadHooks, TracePlatform};
use web_time::Instant;

/// Microseconds since the first call: the ring's clock on the PC and the
/// Web (`performance.now()` there, through `web_time`).
pub fn clock() -> u32 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u32
}

thread_local! {
    /// The calling thread's lane ([`trace::bind_thread`]).
    static LANE: Cell<Option<&'static Lane>> = const { Cell::new(None) };
}

/// The PC's and the Web's [`ThreadHooks`]: a `thread_local!` per thread,
/// a lane leaked for the process when every registered one is taken
/// (16 KB each, at most [`trace::MAX_LANES`]).
pub fn thread_hooks() -> ThreadHooks {
    ThreadHooks {
        // `try_with`: a thread's zones may end while it tears down.
        get: || LANE.try_with(Cell::get).ok().flatten(),
        set: |l| {
            let _ = LANE.try_with(|c| c.set(l));
        },
        grow: || Some(Box::leak(Box::new(Lane::new()))),
    }
}

/// The global ring's owner on a PC or Web host.
pub struct TraceHost {
    session: String,
    seq: u32,
    out: Box<dyn FnMut(&str)>,
    frames: u32,
    /// Flush at least every this many frames.
    every: u32,
}

impl TraceHost {
    /// Record from now on into a ring leaked for the process (48 KB), as
    /// `session` (`pc-castle64`), each line handed to `out`. `None` without
    /// the `trace` feature, or when a ring is already installed.
    pub fn start(
        platform: TracePlatform,
        session: &str,
        out: Box<dyn FnMut(&str)>,
    ) -> Option<TraceHost> {
        if !trace::ENABLED || trace::is_on() {
            return None;
        }
        let ring: &'static mut GlobalRing = Box::leak(Box::new(GlobalRing::new()));
        trace::set_thread_hooks(Some(thread_hooks()));
        trace::install(ring, clock, platform).then(|| TraceHost {
            session: session.chars().filter(|c| !c.is_whitespace()).collect(),
            seq: 0,
            out,
            frames: 0,
            every: 60,
        })
    }

    /// Flush every `n` frames at least (the Web: fewer, longer console
    /// bursts).
    pub fn flush_every(mut self, n: u32) -> Self {
        self.every = n.max(1);
        self
    }

    /// After a frame: flush when the ring is half full or `every` frames
    /// went by, so a long run loses nothing.
    pub fn frame(&mut self) {
        self.frames += 1;
        let half = trace::with_ring(|r, _| r.len() >= GLOBAL_EVENTS / 2).unwrap_or(false);
        if half || self.frames.is_multiple_of(self.every) {
            self.flush();
        }
    }

    /// Hand what the ring holds to the sink now.
    pub fn flush(&mut self) {
        let out = &mut self.out;
        let t0 = trace::now();
        trace::flush(&self.session, &mut self.seq, &mut |l| out(l));
        // Its own zone (in the next packet): a frame's spike from writing
        // the trace reads as the trace's.
        let t1 = trace::now();
        trace::record_zone("trace flush", trace::track::CPU, 0, t0, t1.wrapping_sub(t0));
    }

    /// The last lines; recording stops.
    pub fn finish(mut self) {
        self.flush();
        trace::uninstall();
    }

    /// Lines written so far.
    pub fn lines(&self) -> u32 {
        self.seq
    }
}
