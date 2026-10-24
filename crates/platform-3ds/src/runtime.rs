//! Boot, logging, time, panic handler and global allocator — the no_std
//! runtime glue that every 3DS binary needs exactly once.

use alloc::ffi::CString;
use core::alloc::{GlobalAlloc, Layout};

use crate::ffi;

/// Initialize libctru subsystems (gfx, romfs, citro3d/2d, ndsp), detect
/// the New 3DS and turn its speedup on (unless the RomFS holds
/// [`crate::hw::NO_SPEEDUP_FILE`]), print the hardware tier
/// (`TRINO_HW 3ds new=1 speedup=1 …`, ADR 0020) and start the worker core
/// (`crate::worker`: asset groups decode there, issue #56). Call first,
/// once.
pub fn init() {
    unsafe { ffi::trino_init() }
    let want = !crate::N3dsAssets::exists(crate::hw::NO_SPEEDUP_FILE);
    let word = unsafe { ffi::trino_hw_probe(want as u32) };
    PROBE.store(word, core::sync::atomic::Ordering::Relaxed);
    let p = hw();
    log(&alloc::format!(
        "TRINO_HW 3ds new={} speedup={} app_ram={}M tier={} stereo={}\n",
        p.new3ds as u8,
        p.speedup as u8,
        p.app_mib,
        p.tier().key(),
        p.stereo as u8
    ));
    let lang = unsafe { ffi::trino_system_language() };
    LANGUAGE.store(lang, core::sync::atomic::Ordering::Relaxed);
    log(&alloc::format!(
        "TRINO_LANG {}\n",
        system_language().as_ref().map_or("none", |t| t.as_str())
    ));
    crate::worker::start();
}

/// The boot's `trino_system_language` code (-1: none).
static LANGUAGE: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(-1);

/// The system language the boot read (`crate::language`).
pub fn system_language() -> Option<trino_core::LangTag> {
    crate::language::tag(LANGUAGE.load(core::sync::atomic::Ordering::Relaxed))
}

/// The boot's probe word (`trino_hw_probe`).
static PROBE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// What the boot found: New 3DS, speedup, application memory.
pub fn hw() -> crate::hw::Probe {
    crate::hw::Probe::decode(PROBE.load(core::sync::atomic::Ordering::Relaxed))
}

/// The hardware tier: `Expanded` on a New 3DS with the speedup on.
pub fn hw_tier() -> trino_core::HwTier {
    hw().tier()
}

pub fn log(msg: &str) {
    if let Ok(c) = CString::new(msg) {
        unsafe { ffi::trino_log(c.as_ptr()) }
    }
}

/// Monotonic microseconds (wraps every ~71 minutes; diff arithmetic only).
pub fn now_us() -> u32 {
    unsafe { ffi::trino_ticks_us() }
}

/// Start recording the engine's zones into a static ring, timed by
/// [`now_us`] (issue #62). `false` (and no ring linked) unless the build has
/// `trino-core/trace` (`cargo xtask test 3ds --trace`).
pub fn trace_start() -> bool {
    use trino_core::trace::{self, GlobalRing, TracePlatform};
    if !trace::ENABLED {
        return false;
    }
    if trace::is_on() {
        return true;
    }
    static mut RING: GlobalRing = GlobalRing::new();
    // SAFETY: the only reference, handed over once (`is_on` above).
    trace::install(
        unsafe { &mut *core::ptr::addr_of_mut!(RING) },
        now_us,
        TracePlatform::N3ds,
    )
}

/// Print what the ring holds as `TRINO_TRACE:` lines of `session` through
/// `svcOutputDebugString` (and stdout under 3dslink), numbered from `*seq`.
pub fn trace_flush(session: &str, seq: &mut u32) -> bool {
    trino_core::trace::flush(session, seq, &mut |l| log(l))
}

/// The play loop's trace (`run 3ds --trace`, issue #62): the engine's
/// zones flushed as `session` every 60 frames. No code at all without
/// `trino-core/trace`.
pub struct TraceLoop {
    on: bool,
    seq: u32,
    frames: u32,
    session: &'static str,
}

impl TraceLoop {
    pub fn start(session: &'static str) -> TraceLoop {
        TraceLoop {
            on: trino_core::trace::ENABLED && trace_start(),
            seq: 0,
            frames: 0,
            session,
        }
    }

    /// After each frame.
    pub fn frame(&mut self) {
        if !trino_core::trace::ENABLED || !self.on {
            return;
        }
        self.frames += 1;
        if self.frames.is_multiple_of(60) {
            // Printing is slow (ISViewer: ~0.1 s for 60 frames' events): the
            // flush shows on the timeline as its own zone, so its frame's
            // spike reads as the trace's, not the game's.
            let t0 = trino_core::trace::now();
            trace_flush(self.session, &mut self.seq);
            let t1 = trino_core::trace::now();
            trino_core::trace::record_zone(
                "trace flush",
                trino_core::trace::track::CPU,
                0,
                t0,
                t1.wrapping_sub(t0),
            );
        }
    }
}

/// False when the applet manager wants the app to quit (HOME / power) —
/// the main loop must return so the shim can tear down cleanly.
pub fn running() -> bool {
    unsafe { ffi::trino_app_running() != 0 }
}

struct NewlibAlloc;

unsafe impl GlobalAlloc for NewlibAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { ffi::memalign(layout.align().max(8), layout.size()) as *mut u8 }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe { ffi::free(ptr as *mut core::ffi::c_void) }
    }
}

#[global_allocator]
static ALLOC: NewlibAlloc = NewlibAlloc;

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
