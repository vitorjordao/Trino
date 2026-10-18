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
        "TRINO_HW 3ds new={} speedup={} app_ram={}M tier={}\n",
        p.new3ds as u8,
        p.speedup as u8,
        p.app_mib,
        p.tier().key()
    ));
    crate::worker::start();
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
