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

/// Monotonic microseconds (wraps every ~71 minutes; diff arithmetic only).
pub fn now_us() -> u32 {
    unsafe { ffi::trino_ticks_us() }
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
