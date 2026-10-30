//! Boot, logging, time, panic handler and global allocator — the no_std
//! runtime glue that every N64 binary needs exactly once.

use alloc::ffi::CString;
use core::alloc::{GlobalAlloc, Layout};

use crate::ffi;

/// Initialize libdragon subsystems (display, rdpq, joypad, dfs, audio,
/// ISViewer debugging) and print the hardware tier (`TRINO_HW n64 ram=8M
/// tier=expanded`, ADR 0020). Call first, once.
pub fn init() {
    unsafe { ffi::trino_init() }
    let ram = memory_size();
    log(&alloc::format!(
        "TRINO_HW n64 ram={}M tier={}\n",
        crate::hw::megabytes(ram),
        crate::hw::tier_of_memory(ram).key()
    ));
}

/// RDRAM in bytes: 4 MiB, 8 MiB with the Expansion Pak.
pub fn memory_size() -> u32 {
    unsafe { ffi::trino_memory_size() }
}

/// The hardware tier: `Expanded` with an Expansion Pak (issue #54).
pub fn hw_tier() -> trino_core::HwTier {
    crate::hw::tier_of_memory(memory_size())
}

/// Self-test only: report a stall (`TRINO_STALL`) if no frame begins and
/// nothing is logged for 4 s of emulated time (see the shim).
pub fn watchdog_start() {
    unsafe { ffi::trino_watchdog_start() }
}

pub fn log(msg: &str) {
    if let Ok(c) = CString::new(msg) {
        unsafe { ffi::trino_log(c.as_ptr()) }
    }
}

/// Deepest main-stack use since boot (bytes; the stack is 64 KB).
pub fn stack_used() -> u32 {
    unsafe { ffi::trino_stack_used() }
}

/// Heap bytes in use and in total (RDRAM left after code, data, stack).
pub fn heap() -> (u32, u32) {
    unsafe { (ffi::trino_heap_used(), ffi::trino_heap_total()) }
}

/// The heap's top free block, where big allocations come from (newlib's
/// top chunk + the unclaimed sbrk room; allocates nothing). On the base
/// N64 the free bytes fragment and this is what a big `Vec` finds
/// (GitHub #110).
pub fn heap_top_free() -> u32 {
    unsafe { ffi::trino_heap_top_free() }
}

/// Monotonic microseconds (wraps every ~71 minutes; diff arithmetic only).
/// The FPU's control word (FCR31): how this CPU treats subnormals and
/// NaNs (issue #67, logged by the `replay` self-test step).
pub fn fpu_control() -> u32 {
    unsafe { crate::ffi::trino_fpu_control() }
}

pub fn now_us() -> u32 {
    unsafe { ffi::trino_ticks_us() }
}

/// Start recording the engine's zones into a static ring, timed by
/// [`now_us`] (issue #62). `false` (and no ring linked) unless the build has
/// `trino-core/trace` (`cargo xtask test n64 --trace`).
pub fn trace_start() -> bool {
    use trino_core::trace::{self, GlobalRing, TracePlatform};
    // A sampled build (`--prof sample`, issue #79) records no zones: the
    // recorder would be in its samples.
    if !trace::ENABLED || cfg!(feature = "prof-sample") {
        return false;
    }
    if trace::is_on() {
        return true;
    }
    // A static, never the stack (48 KB; the stack is 64 KB) or the heap.
    static mut RING: GlobalRing = GlobalRing::new();
    // SAFETY: the only reference: handed over once (`is_on` above), and
    // `uninstall` is never called on the N64.
    trace::install(
        unsafe { &mut *core::ptr::addr_of_mut!(RING) },
        now_us,
        TracePlatform::N64,
    )
}

/// Print what the ring holds as `TRINO_TRACE:` lines of `session` over
/// ISViewer (and the USB log on a flashcart), numbered from `*seq`.
pub fn trace_flush(session: &str, seq: &mut u32) -> bool {
    trino_core::trace::flush(session, seq, &mut |l| log(l))
}

/// Print what the ring holds like [`trace_flush`] and time it: the
/// lines printed and the µs they took (ISViewer's cost per line, issue
/// #79 / #105 item 2).
pub fn trace_flush_timed(session: &str, seq: &mut u32) -> (u32, u32) {
    let mut lines = 0u32;
    let t0 = now_us();
    trino_core::trace::flush(session, seq, &mut |l| {
        lines += 1;
        log(l)
    });
    (lines, now_us().wrapping_sub(t0))
}

/// The play loop's trace (`run n64 --trace`, issue #62): the engine's
/// zones as `session`, encoded every 60 frames and printed a few lines per
/// frame over the next 60. No code at all without `trino-core/trace`.
///
/// Printing all of a ring's lines at once cost ~115 ms in ares (~40 lines
/// over ISViewer): a 7× spike every second (#105 item 2). Spread, each
/// frame pays its share (a `trace flush` zone of a line or so) and no
/// frame stands out.
pub struct TraceLoop {
    on: bool,
    seq: u32,
    frames: u32,
    session: &'static str,
    /// Encoded lines waiting to be printed, and how many go per frame.
    pending: alloc::collections::VecDeque<alloc::string::String>,
    per_frame: usize,
}

/// Frames between two encodes of the ring (and over which its lines are
/// printed).
const TRACE_PERIOD: u32 = 60;

impl TraceLoop {
    pub fn start(session: &'static str) -> TraceLoop {
        TraceLoop {
            on: trino_core::trace::ENABLED && trace_start(),
            seq: 0,
            frames: 0,
            session,
            pending: alloc::collections::VecDeque::new(),
            per_frame: 1,
        }
    }

    /// After each frame.
    pub fn frame(&mut self) {
        if !trino_core::trace::ENABLED || !self.on {
            return;
        }
        self.frame_out();
    }

    #[inline(never)]
    fn frame_out(&mut self) {
        use trino_core::trace;
        self.frames += 1;
        if self.frames.is_multiple_of(TRACE_PERIOD) {
            // Whatever is still waiting goes first (lines stay in order).
            for l in self.pending.drain(..) {
                log(&l);
            }
            let t0 = trace::now();
            let pending = &mut self.pending;
            trace::flush(self.session, &mut self.seq, &mut |l| {
                pending.push_back(alloc::string::String::from(l))
            });
            self.per_frame = self.pending.len().div_ceil(TRACE_PERIOD as usize).max(1);
            let t1 = trace::now();
            trace::record_zone(
                "trace encode",
                trace::track::CPU,
                0,
                t0,
                t1.wrapping_sub(t0),
            );
        }
        if self.pending.is_empty() {
            return;
        }
        // The flush shows on the timeline as its own zone, so its share of
        // the frame reads as the trace's, not the game's.
        let t0 = trace::now();
        for _ in 0..self.per_frame {
            match self.pending.pop_front() {
                Some(l) => log(&l),
                None => break,
            }
        }
        let t1 = trace::now();
        trace::record_zone("trace flush", trace::track::CPU, 0, t0, t1.wrapping_sub(t0));
    }
}

/// Vertical blanks since boot (60 Hz NTSC): an independent clock for
/// perf reports.
pub fn vblanks() -> u32 {
    unsafe { ffi::trino_vblank_count() }
}

struct LibdragonAlloc;

unsafe impl GlobalAlloc for LibdragonAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { ffi::memalign(layout.align().max(8), layout.size()) as *mut u8 }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe { ffi::free(ptr as *mut core::ffi::c_void) }
    }
}

#[global_allocator]
static ALLOC: LibdragonAlloc = LibdragonAlloc;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Format into a fixed buffer — the allocator may be the thing panicking.
    use core::fmt::Write;
    struct Buf {
        data: [u8; 512],
        len: usize,
    }
    impl Write for Buf {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let take = s.len().min(self.data.len() - 1 - self.len);
            self.data[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
            self.len += take;
            Ok(())
        }
    }
    let mut buf = Buf {
        data: [0; 512],
        len: 0,
    };
    let _ = write!(buf, "{info}");
    buf.data[buf.len] = 0;
    unsafe { ffi::trino_panic(buf.data.as_ptr() as *const core::ffi::c_char) }
}
