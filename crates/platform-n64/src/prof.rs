//! What an instrumented N64 build measures besides the trace's zones
//! (issue #79, ADR 0038): the backend's counters, the per-command RSP cost
//! (`--prof rsp`) and the CPU sampler (`--prof sample`).
//!
//! Nothing here exists in a plain build: every entry point tests
//! [`trino_core::trace::ENABLED`] (or a `prof-*` feature) and calls an
//! `#[inline(never)]` body only when it is on, so the renderer's hot code
//! and its structs stay exactly as they were (the AGENTS pitfall on code
//! layout). The C shim counts its part only in `shim_prof.o`
//! (`-DTRINO_PROF`).

use trino_core::memo::{self, MemoKey, MemoSlot};
use trino_core::trace;

use crate::ffi;

/// A counter the Rust side keeps.
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Counter {
    /// A draw answered from the draw memo (a remembered RSP command).
    MemoHit,
    /// A draw the memo keyed but didn't hold.
    MemoMiss,
    /// A memo hit that was a remembered whole-mesh cull.
    MemoCulled,
    /// A command (or a cull) stored into the memo.
    MemoStore,
    /// A store that replaced another draw's entry.
    MemoEvict,
    /// A water draw whose command the water memo held.
    WaterHit,
    /// A water draw built anew.
    WaterMiss,
    /// Water memo entries dropped to make room.
    WaterEvict,
    /// An RSP-path draw refused by the eligibility test (CPU T&L instead).
    RspReject,
}

const RUST: usize = 9;
/// The C shim's counters, in its `TP_*` order.
const C: usize = 7;
/// Every counter.
pub const COUNTERS: usize = RUST + C;

/// The counters' names: `TRINO_PERF_SCENE` / `TRINO_FRAME` keys, trace
/// counter tracks and the JSON report's fields.
pub const NAMES: [&str; COUNTERS] = [
    "memo_hit",
    "memo_miss",
    "memo_culled",
    "memo_store",
    "memo_evict",
    "water_hit",
    "water_miss",
    "water_evict",
    "rsp_reject",
    "rspq_words",
    "rsp_cmds",
    "ovl_switch",
    "rsp_flush",
    "tex_upload",
    "tex_resident",
    "cpu_tris",
];

/// This frame's counts (the Rust ones; the shim's are folded in at the
/// frame's end) and the totals since the last [`take`]. Single writer: the
/// main loop (never an interrupt).
static mut FRAME: [u32; COUNTERS] = [0; COUNTERS];
static mut TOTAL: [u32; COUNTERS] = [0; COUNTERS];

/// Count one `c`. Nothing at all in a plain build.
#[inline(always)]
pub fn bump(c: Counter) {
    if trace::ENABLED {
        bump_out(c, 1);
    }
}

/// Count `n` of `c`.
#[inline(always)]
pub fn add(c: Counter, n: u32) {
    if trace::ENABLED {
        bump_out(c, n);
    }
}

#[inline(never)]
fn bump_out(c: Counter, n: u32) {
    // SAFETY: one thread (see FRAME).
    unsafe {
        let a = &mut *core::ptr::addr_of_mut!(FRAME);
        a[c as usize] = a[c as usize].wrapping_add(n);
    }
}

/// A memo store of `key`: counted, and as an eviction when its slot held
/// another draw.
#[inline(always)]
pub fn note_store<const W: usize>(slots: &[MemoSlot<W>], key: &MemoKey) {
    if trace::ENABLED {
        note_store_out(slots, key);
    }
}

#[inline(never)]
fn note_store_out<const W: usize>(slots: &[MemoSlot<W>], key: &MemoKey) {
    bump_out(Counter::MemoStore, 1);
    if memo::evicts(slots, key) {
        bump_out(Counter::MemoEvict, 1);
    }
}

/// The frame ends (the renderer's `end_frame`): fold the shim's counts in,
/// add the frame to the totals and sample each counter on the trace's
/// counter tracks. Only in traced builds.
#[inline(always)]
pub fn end_frame() {
    if trace::ENABLED {
        fold(true);
    }
}

#[inline(never)]
fn fold(sample: bool) {
    // SAFETY: one thread (see FRAME).
    unsafe {
        let f = &mut *core::ptr::addr_of_mut!(FRAME);
        let t = &mut *core::ptr::addr_of_mut!(TOTAL);
        ffi::trino_prof_read(f[RUST..].as_mut_ptr(), C as u32);
        for i in 0..COUNTERS {
            t[i] = t[i].wrapping_add(f[i]);
            if sample {
                trace::counter(NAMES[i], f[i]);
            }
        }
        *f = [0; COUNTERS];
    }
}

/// Every counter since the last call (Rust's, then the shim's), zeroed;
/// zeros in a plain build.
pub fn take() -> [u32; COUNTERS] {
    if !trace::ENABLED {
        return [0; COUNTERS];
    }
    fold(false);
    // SAFETY: one thread (see FRAME).
    unsafe { core::mem::replace(&mut *core::ptr::addr_of_mut!(TOTAL), [0; COUNTERS]) }
}

/// The counters of the last profiled window (a perf scene's measured
/// frames, `start_profile` .. `take_profile`).
static mut WINDOW: [u32; COUNTERS] = [0; COUNTERS];

/// A profiled window starts: the counters restart, and the `--prof`
/// mode's measurement with them.
#[inline(always)]
pub fn start_window() {
    if trace::ENABLED {
        start_window_out();
    }
}

#[inline(never)]
fn start_window_out() {
    take();
    #[cfg(feature = "prof-rsp")]
    rsp::calibrate();
    #[cfg(feature = "prof-sample")]
    sample::start();
}

/// The window ends: its counters are kept for [`window`].
#[inline(always)]
pub fn end_window() {
    if trace::ENABLED {
        end_window_out();
    }
}

#[inline(never)]
fn end_window_out() {
    #[cfg(feature = "prof-sample")]
    sample::stop();
    // SAFETY: one thread (see FRAME).
    unsafe { *core::ptr::addr_of_mut!(WINDOW) = take() };
}

/// The last window's counters.
pub fn window() -> [u32; COUNTERS] {
    // SAFETY: one thread (see FRAME).
    unsafe { *core::ptr::addr_of!(WINDOW) }
}

/// What the `--prof` mode measured over the last window, as
/// `TRINO_PROF_RSP:` / `TRINO_PROF_SAMPLE:` lines of `scene` (empty
/// without a mode).
#[allow(unused_variables, unused_mut)]
pub fn window_report(scene: &str, frames: u32) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    #[cfg(feature = "prof-rsp")]
    s.push_str(&rsp::report(scene));
    #[cfg(feature = "prof-sample")]
    s.push_str(&sample::report(scene, frames));
    s
}

/// `k=v` pairs of `counts / frames` for a `TRINO_PERF_SCENE` or
/// `TRINO_FRAME` line (empty in a plain build).
pub fn fields(counts: &[u32; COUNTERS], frames: u32) -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    if trace::ENABLED {
        let n = frames.max(1);
        for (name, v) in NAMES.iter().zip(counts) {
            let _ = write!(s, " {name}={}", v / n);
        }
    }
    s
}

/// `--prof rsp`: every RSP command timed on its own (issue #79).
///
/// The renderer drains the queue before the batch, then after each
/// command; the time of a drain with nothing queued (calibrated at the
/// scene's start) is subtracted. Each command is recorded with its
/// overlay and the vertex and triangle counts of its segment table, and
/// [`report`](rsp::report) prints one `TRINO_PROF_RSP:` line per distinct
/// (overlay, vertices, triangles) with the count and the summed µs; the
/// host fits µs per command, vertex and triangle per overlay.
#[cfg(feature = "prof-rsp")]
pub mod rsp {
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use core::fmt::Write;

    use crate::runtime::now_us;
    use crate::{ffi, rsp};

    /// Which overlay ran a command.
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
    pub enum Ovl {
        White,
        Color,
        Tex,
        Palette,
        Water,
    }

    impl Ovl {
        pub fn name(self) -> &'static str {
            match self {
                Ovl::White => "white",
                Ovl::Color => "color",
                Ovl::Tex => "tex",
                Ovl::Palette => "palette",
                Ovl::Water => "water",
            }
        }

        /// The overlay of a queued command by its word 0's flags.
        pub fn of(w0: u32) -> Ovl {
            if w0 & rsp::PALETTE != 0 {
                Ovl::Palette
            } else if w0 & rsp::TEXTURED != 0 {
                Ovl::Tex
            } else if w0 & rsp::COLORED != 0 {
                Ovl::Color
            } else {
                Ovl::White
            }
        }
    }

    /// Vertices and triangles a command draws: its segment table (word 1:
    /// the table's physical address; word 2's high half: the segments),
    /// 16 bytes an entry, the counts at bytes 6 and 8.
    pub fn work(w: &[u32; rsp::DRAW_WORDS]) -> (u32, u32) {
        let table = (w[1] | 0xA000_0000) as *const u8;
        let segs = (w[2] >> 16) as usize;
        let (mut v, mut t) = (0, 0);
        for s in 0..segs.min(64) {
            // SAFETY: the table the command points the RSP at (uncached,
            // alive until the drain).
            let e = unsafe { core::slice::from_raw_parts(table.add(s * 16), 16) };
            v += u16::from_be_bytes([e[6], e[7]]) as u32;
            t += u16::from_be_bytes([e[8], e[9]]) as u32;
        }
        (v, t)
    }

    /// (overlay, verts, tris) → (commands, µs) over the window, and the
    /// empty drain's µs subtracted from each. Main loop only.
    static mut BY: BTreeMap<(Ovl, u32, u32), (u32, u32)> = BTreeMap::new();
    static mut EMPTY_US: u32 = 0;

    fn by() -> &'static mut BTreeMap<(Ovl, u32, u32), (u32, u32)> {
        // SAFETY: one thread (the main loop).
        unsafe { &mut *core::ptr::addr_of_mut!(BY) }
    }

    /// Calibrate the empty drain: the median of 16.
    pub fn calibrate() {
        let mut t = [0u32; 16];
        for s in &mut t {
            let t0 = now_us();
            unsafe { ffi::trino_drain() };
            *s = now_us().wrapping_sub(t0);
        }
        t.sort_unstable();
        unsafe { EMPTY_US = t[8] };
        by().clear();
    }

    /// Run `submit` (which writes one command) alone on the RSP and charge
    /// its time to (`ovl`, `work`).
    pub fn time(ovl: Ovl, work: (u32, u32), submit: impl FnOnce()) {
        unsafe { ffi::trino_drain() };
        let t0 = now_us();
        submit();
        unsafe { ffi::trino_drain() };
        let us = now_us()
            .wrapping_sub(t0)
            .saturating_sub(unsafe { EMPTY_US });
        let e = by().entry((ovl, work.0, work.1)).or_insert((0, 0));
        e.0 += 1;
        e.1 += us;
    }

    /// One `TRINO_PROF_RSP:<scene> ...` line per entry, then forget them.
    pub fn report(scene: &str) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "TRINO_PROF_RSP:{scene} empty_drain_us={}", unsafe {
            EMPTY_US
        });
        for ((ovl, v, t), (n, us)) in by().iter() {
            let _ = writeln!(
                s,
                "TRINO_PROF_RSP:{scene} ovl={} verts={v} tris={t} n={n} us={us}",
                ovl.name()
            );
        }
        by().clear();
        s
    }
}

/// `--prof sample`: the CPU's PC sampled by a timer interrupt over the
/// measured frames (issue #79). The host symbolizes the PCs with the
/// ELF's symbol table (`target/n64/<name>.syms`).
#[cfg(feature = "prof-sample")]
pub mod sample {
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::fmt::Write;

    use crate::ffi;
    use crate::runtime::now_us;

    /// One sample every this many µs (~400 over a 40 ms frame). What the
    /// interrupt costs is measured at the first start and reported with
    /// the samples (`handler_us`, `overhead_pct`): ~5.3 µs in ares, so 5 %
    /// at 100 µs (10.6 % at 50). #69's sampler at 4–8 µs inflated the frame
    /// ~4× and skewed the histogram toward loops. Time spent with
    /// interrupts off is charged to the instruction that turns them back
    /// on (`enable_interrupts`).
    pub const PERIOD_US: u32 = 100;
    /// Samples kept per scene (128 KB of heap while it runs).
    pub const CAP: usize = 32 * 1024;

    /// The samples' buffer while sampling; then the samples taken and lost.
    static mut BUF: Vec<u32> = Vec::new();
    static mut TAKEN: (usize, u32) = (0, 0);
    static mut ON: bool = false;
    /// The handler's cost per sample, ns (0 until calibrated).
    static mut HANDLER_NS: u32 = 0;

    /// A fixed amount of CPU work.
    #[inline(never)]
    fn busy(n: u32) -> u32 {
        let mut x = 0u32;
        for i in 0..n {
            x = core::hint::black_box(x.wrapping_mul(31).wrapping_add(i));
        }
        x
    }

    /// Measure what one sample costs: the same work with and without the
    /// sampler, divided by the samples it took (once per boot).
    fn calibrate() {
        const N: u32 = 400_000;
        let t0 = now_us();
        busy(N);
        let plain = now_us().wrapping_sub(t0);
        let mut buf = alloc::vec![0u32; 4096];
        let mut lost = 0u32;
        let t0 = now_us();
        unsafe { ffi::trino_prof_sample_start(buf.as_mut_ptr(), 4096, PERIOD_US) };
        busy(N);
        let k = unsafe { ffi::trino_prof_sample_stop(&mut lost) } + lost;
        let sampled = now_us().wrapping_sub(t0);
        let ns = (sampled.saturating_sub(plain) as u64 * 1000) / k.max(1) as u64;
        unsafe { HANDLER_NS = (ns as u32).max(1) };
        drop(buf);
    }

    pub fn start() {
        // SAFETY: the main loop only; the interrupt writes into BUF's
        // storage, which stays put until `stop`.
        unsafe {
            if ON {
                return;
            }
            if HANDLER_NS == 0 {
                calibrate();
            }
            let buf = &mut *core::ptr::addr_of_mut!(BUF);
            buf.clear();
            buf.resize(CAP, 0);
            ON = true;
            ffi::trino_prof_sample_start(buf.as_mut_ptr(), CAP as u32, PERIOD_US);
        }
    }

    pub fn stop() {
        unsafe {
            if !ON {
                return;
            }
            ON = false;
            let mut lost = 0u32;
            let n = ffi::trino_prof_sample_stop(&mut lost) as usize;
            TAKEN = (n.min(CAP), lost);
        }
    }

    /// `TRINO_PROF_SAMPLE:<scene>` lines: the totals, then the PCs with
    /// their counts (hex address = samples, 8 per line). Frees the buffer.
    pub fn report(scene: &str, frames: u32) -> String {
        let mut s = String::new();
        // SAFETY: not sampling (stopped at the window's end).
        let (buf, (n, lost)) = unsafe {
            (
                core::mem::take(&mut *core::ptr::addr_of_mut!(BUF)),
                *core::ptr::addr_of!(TAKEN),
            )
        };
        if buf.is_empty() {
            return s;
        }
        let mut by: BTreeMap<u32, u32> = BTreeMap::new();
        for &pc in &buf[..n] {
            *by.entry(pc).or_insert(0) += 1;
        }
        let ns = unsafe { HANDLER_NS };
        let _ = writeln!(
            s,
            "TRINO_PROF_SAMPLE:{scene} samples={n} lost={lost} period_us={PERIOD_US} \
             frames={frames} pcs={} handler_us={}.{:03} overhead_pct={}.{}",
            by.len(),
            ns / 1000,
            ns % 1000,
            ns / (PERIOD_US * 10),
            (ns / PERIOD_US) % 10
        );
        let mut line = String::new();
        for (i, (pc, c)) in by.iter().enumerate() {
            let _ = write!(line, " {pc:08x}={c}");
            if i % 8 == 7 {
                let _ = writeln!(s, "TRINO_PROF_SAMPLE:{scene}{line}");
                line.clear();
            }
        }
        if !line.is_empty() {
            let _ = writeln!(s, "TRINO_PROF_SAMPLE:{scene}{line}");
        }
        s
    }
}
