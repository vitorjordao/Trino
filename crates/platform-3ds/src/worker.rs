//! The worker core (issue #54 Phase 3, ADR 0020): game jobs on the 3DS's
//! second CPU core, for the work a frame can hand off — AI (#56), a
//! crowd's animation. The shim's audio thread (#53, ADR 0021) runs on the
//! same core, granted by the same policy, one priority above the jobs: ndsp
//! wakes it every audio frame to refill the streams, so a long job never
//! holds a refill back (`N3dsAudio::stream_core` says which core).
//!
//! [`start`] asks for the New 3DS's core 2 (the app's whole) and falls back
//! to the old 3DS's core 1, of which the app gets 30%
//! (`APT_SetAppCpuTimeLimit`; the system keeps the rest). A job is a plain
//! `extern "C"` function and one word — an index, or the address of data the
//! caller keeps alive until [`wait`]. [`submit`] queues it, or runs it right
//! there when there is no worker or its 32-job ring is full, so a game that
//! never checks [`core`] behaves the same everywhere, only slower without
//! the second core. Jobs must not touch the renderer, the audio or the
//! platform (those belong to the main thread); they compute into memory
//! the main thread reads after [`wait`].
//!
//! ```ignore
//! extern "C" fn think(i: u32) { /* agent i's next move into MOVES[i] */ }
//! worker::start();
//! for i in 0..agents { worker::submit(think, i); }
//! // ... the main thread draws meanwhile ...
//! worker::wait();
//! ```

use core::sync::atomic::{AtomicI32, Ordering};

/// A job: a function and the one word it takes.
pub type Job = extern "C" fn(arg: u32);

/// Not started yet.
const UNSTARTED: i32 = -2;
/// Started, no core granted: jobs run inline.
const INLINE: i32 = -1;

static CORE: AtomicI32 = AtomicI32::new(UNSTARTED);

/// Which core [`start`] asks for first: core 2 on a New 3DS (whatever its
/// clock: the extra cores do not need the speedup), else core 1.
pub fn preferred_core(new3ds: bool) -> u8 {
    if new3ds { 2 } else { 1 }
}

/// The shim's answer (`trino_worker_start`): the core, or none.
pub fn decode(started: i32) -> Option<u8> {
    u8::try_from(started).ok().filter(|c| (1..=3).contains(c))
}

/// Start the worker once (later calls return the same answer): the core
/// the jobs run on, or `None` (jobs run inline on the main thread).
pub fn start() -> Option<u8> {
    let now = CORE.load(Ordering::Acquire);
    if now != UNSTARTED {
        return decode(now);
    }
    let started = spawn();
    CORE.store(
        if decode(started).is_some() {
            started
        } else {
            INLINE
        },
        Ordering::Release,
    );
    decode(started)
}

/// The worker's core, if [`start`] got one.
pub fn core() -> Option<u8> {
    decode(CORE.load(Ordering::Acquire))
}

/// Queue `job(arg)` for the worker, or run it now (no worker, or the ring
/// is full). Returns true when it was queued.
pub fn submit(job: Job, arg: u32) -> bool {
    if core().is_some() && queue(job, arg) {
        return true;
    }
    job(arg);
    false
}

/// Block until every queued job has run.
pub fn wait() {
    if core().is_some() {
        drain();
    }
}

#[cfg(target_os = "horizon")]
fn spawn() -> i32 {
    let new3ds = crate::runtime::hw().new3ds;
    unsafe { crate::ffi::trino_worker_start(new3ds as u32) }
}

#[cfg(target_os = "horizon")]
fn queue(job: Job, arg: u32) -> bool {
    unsafe { crate::ffi::trino_worker_submit(job, arg) != 0 }
}

#[cfg(target_os = "horizon")]
fn drain() {
    unsafe { crate::ffi::trino_worker_wait() }
}

// Host builds (tests): no second core, every job runs inline.
#[cfg(not(target_os = "horizon"))]
fn spawn() -> i32 {
    INLINE
}

#[cfg(not(target_os = "horizon"))]
fn queue(_job: Job, _arg: u32) -> bool {
    false
}

#[cfg(not(target_os = "horizon"))]
fn drain() {}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicU32;

    static SUMS: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];

    extern "C" fn add(i: u32) {
        SUMS[i as usize].fetch_add(i + 1, Ordering::Relaxed);
    }

    #[test]
    fn the_shims_answer_names_a_core_or_none() {
        assert_eq!((preferred_core(true), preferred_core(false)), (2, 1));
        assert_eq!((decode(2), decode(1)), (Some(2), Some(1)));
        assert_eq!(
            (decode(-1), decode(-2), decode(0), decode(9)),
            (None, None, None, None)
        );
    }

    #[test]
    fn without_a_worker_every_job_runs_inline() {
        assert_eq!(start(), None);
        assert_eq!((start(), core()), (None, None));
        for i in 0..4 {
            assert!(!submit(add, i), "inline, not queued");
        }
        wait();
        for (i, sum) in SUMS.iter().enumerate() {
            assert_eq!(sum.load(Ordering::Relaxed), i as u32 + 1);
        }
    }
}
