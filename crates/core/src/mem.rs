//! Allocator discipline without a heap (issue #56, ADR 0022 §6): bump
//! arenas for data that dies together, and a counting wrapper for hosts
//! that want their heap in `TRINO_FRAME`.
//!
//! - [`FrameArena`]: scratch for one frame (layout, text, T&L temporaries),
//!   reset at the frame's start. Its storage is part of the value, so a
//!   frame's temporaries never touch the global heap and never fragment it.
//! - [`Arena`]: the same over memory the caller owns (a `Vec` taken once at
//!   boot on the consoles, a static buffer).
//! - [`LevelArena`]: an [`Arena`] tied to an asset group, reset when the
//!   group unloads, so per-level state dies with the level's assets.
//! - [`Blocks`]: offsets in a region allocated once (the N64's RSP mesh
//!   copies, GitHub #110), freed in any order, so long-lived blocks don't
//!   interleave with the general heap's short-lived ones.
//! - [`TrimWindow`]: when a growable buffer may give memory back — after a
//!   window of frames that used far less than it holds.
//!
//! An allocation is a `&mut [T]` (or `&mut T`) of `Copy` data that borrows
//! the arena: [`reset`](FrameArena::reset) takes `&mut self`, so the borrow
//! checker proves nothing allocated outlives it. On overflow `alloc` returns
//! `None`; [`high_water`](FrameArena::high_water) tells a test or a budget
//! how close a frame came.

use core::cell::{Cell, UnsafeCell};
use core::mem::{MaybeUninit, align_of, size_of};

/// A bump arena over `N` bytes it holds itself.
pub struct FrameArena<const N: usize> {
    buf: UnsafeCell<[MaybeUninit<u8>; N]>,
    top: Cell<usize>,
    high: Cell<usize>,
}

impl<const N: usize> Default for FrameArena<N> {
    fn default() -> Self {
        Self::new()
    }
}

// A bump arena hands out disjoint `&mut` ranges from `&self` (bumpalo's
// API): `reset(&mut self)` ends them all.
#[allow(clippy::mut_from_ref)]
impl<const N: usize> FrameArena<N> {
    pub const fn new() -> Self {
        FrameArena {
            buf: UnsafeCell::new([MaybeUninit::uninit(); N]),
            top: Cell::new(0),
            high: Cell::new(0),
        }
    }

    /// `len` copies of `fill`, or `None` when the arena is full.
    pub fn alloc_slice<T: Copy>(&self, len: usize, fill: T) -> Option<&mut [T]> {
        // SAFETY: the storage lives as long as `self`; `bump` hands out
        // disjoint, aligned ranges until `reset(&mut self)`.
        let base = self.buf.get() as *mut u8;
        unsafe { bump(base, N, &self.top, &self.high, len, fill) }
    }

    /// One value, or `None` when the arena is full.
    pub fn alloc<T: Copy>(&self, value: T) -> Option<&mut T> {
        self.alloc_slice(1, value).map(|s| &mut s[0])
    }

    /// Copy `bytes` into the arena (a string for this frame's text).
    pub fn alloc_bytes(&self, bytes: &[u8]) -> Option<&mut [u8]> {
        let out = self.alloc_slice(bytes.len(), 0u8)?;
        out.copy_from_slice(bytes);
        Some(out)
    }

    /// Free everything at once (every allocation's borrow has ended).
    pub fn reset(&mut self) {
        self.top.set(0);
    }

    /// Bytes in use now.
    pub fn used(&self) -> usize {
        self.top.get()
    }

    /// The most bytes ever in use (since [`new`](Self::new)).
    pub fn high_water(&self) -> usize {
        self.high.get()
    }

    pub const fn capacity(&self) -> usize {
        N
    }
}

/// A bump arena over caller-owned bytes.
pub struct Arena<'a> {
    base: *mut u8,
    len: usize,
    top: Cell<usize>,
    high: Cell<usize>,
    _memory: core::marker::PhantomData<&'a mut [u8]>,
}

// As `FrameArena`: disjoint `&mut` ranges from `&self` until `reset`.
#[allow(clippy::mut_from_ref)]
impl<'a> Arena<'a> {
    pub fn new(memory: &'a mut [MaybeUninit<u8>]) -> Arena<'a> {
        Arena {
            base: memory.as_mut_ptr() as *mut u8,
            len: memory.len(),
            top: Cell::new(0),
            high: Cell::new(0),
            _memory: core::marker::PhantomData,
        }
    }

    pub fn alloc_slice<T: Copy>(&self, len: usize, fill: T) -> Option<&mut [T]> {
        // SAFETY: `base..base + len` is the borrowed memory (`'a`), and
        // `bump` hands out disjoint ranges until `reset(&mut self)`.
        unsafe { bump(self.base, self.len, &self.top, &self.high, len, fill) }
    }

    pub fn alloc<T: Copy>(&self, value: T) -> Option<&mut T> {
        self.alloc_slice(1, value).map(|s| &mut s[0])
    }

    pub fn reset(&mut self) {
        self.top.set(0);
    }

    pub fn used(&self) -> usize {
        self.top.get()
    }

    pub fn high_water(&self) -> usize {
        self.high.get()
    }

    pub const fn capacity(&self) -> usize {
        self.len
    }
}

/// An [`Arena`] whose contents belong to an asset group: [`sync`] resets it
/// once the group is no longer ready, so state built from a level's assets
/// is dropped with them.
///
/// [`sync`]: LevelArena::sync
pub struct LevelArena<'a> {
    arena: Arena<'a>,
    group: crate::GroupId,
}

impl<'a> LevelArena<'a> {
    pub fn new(group: crate::GroupId, memory: &'a mut [MaybeUninit<u8>]) -> Self {
        LevelArena {
            arena: Arena::new(memory),
            group,
        }
    }

    pub const fn group(&self) -> crate::GroupId {
        self.group
    }

    /// Call once per frame: resets the arena when its group is not ready
    /// (unloaded, failed, or loading again). Returns whether it reset.
    pub fn sync(&mut self, platform: &dyn crate::Platform) -> bool {
        if platform.group_state(self.group).is_ready() || self.arena.used() == 0 {
            return false;
        }
        self.arena.reset();
        true
    }

    /// Allocate only while the group is ready ([`sync`](Self::sync) first).
    pub fn arena(&self) -> &Arena<'a> {
        &self.arena
    }
}

/// The shared bump: `len` values of `T` at the next `T`-aligned offset.
///
/// # Safety
/// `base..base + cap` must be valid for writes for as long as the returned
/// borrow, and no other code may use the bytes above `top`.
unsafe fn bump<'s, T: Copy>(
    base: *mut u8,
    cap: usize,
    top: &Cell<usize>,
    high: &Cell<usize>,
    len: usize,
    fill: T,
) -> Option<&'s mut [T]> {
    let size = size_of::<T>().checked_mul(len)?;
    let addr = base as usize + top.get();
    let start = addr.checked_add(align_of::<T>() - 1)? & !(align_of::<T>() - 1);
    let offset = start - base as usize;
    let end = offset.checked_add(size)?;
    if end > cap {
        return None;
    }
    top.set(end);
    high.set(high.get().max(end));
    // SAFETY: in bounds, aligned, and above every live allocation.
    unsafe {
        let p = base.add(offset) as *mut T;
        for i in 0..len {
            p.add(i).write(fill);
        }
        Some(core::slice::from_raw_parts_mut(p, len))
    }
}

/// A free list over offsets `0..size` of a region the caller allocated
/// once: best fit, freed blocks merge with their neighbors, at most `N`
/// free ranges. It never touches the region itself (the N64 keeps it in
/// uncached RDRAM the RSP reads), so the bookkeeping lives on the heap or
/// the stack like any value. Blocks are [`Blocks::ALIGN`]-aligned and
/// rounded up to it.
///
/// Why (GitHub #110): on the base N64 the RSP's mesh copies (up to ~45 KB
/// each) came from the general heap, between short-lived buffers; after a
/// group load and unload the heap had 137 KB free but no 45 KB hole. A
/// region of their own keeps their holes for each other.
#[derive(Clone, Debug)]
pub struct Blocks<const N: usize> {
    size: u32,
    /// Free ranges `(offset, len)`, sorted by offset, never touching.
    free: [(u32, u32); N],
    count: usize,
    used: u32,
    high: u32,
    /// Bytes given up when a free found the list full (never reused).
    lost: u32,
}

impl<const N: usize> Blocks<N> {
    /// Block alignment and granularity, bytes (the RSP DMAs 8-byte
    /// aligned; 16 keeps a cache line's half).
    pub const ALIGN: u32 = 16;

    /// A region of `size` bytes (rounded down to [`Self::ALIGN`]), all free.
    pub const fn new(size: u32) -> Self {
        let size = size / Self::ALIGN * Self::ALIGN;
        let mut free = [(0, 0); N];
        let count = if N > 0 && size > 0 {
            free[0] = (0, size);
            1
        } else {
            0
        };
        Blocks {
            size,
            free,
            count,
            used: 0,
            high: 0,
            lost: 0,
        }
    }

    fn round(len: u32) -> u32 {
        len.max(1).div_ceil(Self::ALIGN) * Self::ALIGN
    }

    /// A block of `len` bytes: its offset, or `None` when no free range
    /// holds it (the caller falls back to its heap). Best fit: the
    /// smallest free range that holds it, its lowest end.
    pub fn alloc(&mut self, len: u32) -> Option<u32> {
        let len = Self::round(len);
        let mut best: Option<usize> = None;
        for i in 0..self.count {
            let l = self.free[i].1;
            if l >= len && best.is_none_or(|b| l < self.free[b].1) {
                best = Some(i);
            }
        }
        let i = best?;
        let (off, l) = self.free[i];
        if l == len {
            self.free.copy_within(i + 1..self.count, i);
            self.count -= 1;
        } else {
            self.free[i] = (off + len, l - len);
        }
        self.used += len;
        self.high = self.high.max(self.used);
        Some(off)
    }

    /// Give back the block at `off` of `len` bytes (what
    /// [`alloc`](Self::alloc) was asked for). Neighbors merge; a free that
    /// would need a range past `N` gives the block up instead (counted in
    /// [`lost`](Self::lost)).
    pub fn free(&mut self, off: u32, len: u32) {
        let len = Self::round(len);
        debug_assert!(off.is_multiple_of(Self::ALIGN) && off + len <= self.size);
        self.used -= len;
        // First free range after the block.
        let at = self.free[..self.count].partition_point(|r| r.0 < off);
        let joins_prev = at > 0 && {
            let p = self.free[at - 1];
            p.0 + p.1 == off
        };
        let joins_next = at < self.count && off + len == self.free[at].0;
        match (joins_prev, joins_next) {
            (true, true) => {
                self.free[at - 1].1 += len + self.free[at].1;
                self.free.copy_within(at + 1..self.count, at);
                self.count -= 1;
            }
            (true, false) => self.free[at - 1].1 += len,
            (false, true) => self.free[at] = (off, len + self.free[at].1),
            (false, false) if self.count < N => {
                self.free.copy_within(at..self.count, at + 1);
                self.free[at] = (off, len);
                self.count += 1;
            }
            (false, false) => self.lost += len,
        }
    }

    /// Bytes of the region.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Bytes in blocks now.
    pub fn used(&self) -> u32 {
        self.used
    }

    /// The most [`used`](Self::used) ever was.
    pub fn high_water(&self) -> u32 {
        self.high
    }

    /// The biggest block [`alloc`](Self::alloc) could hand out now.
    pub fn largest_free(&self) -> u32 {
        self.free[..self.count]
            .iter()
            .map(|r| r.1)
            .max()
            .unwrap_or(0)
    }

    /// Bytes no free could put back on the list.
    pub fn lost(&self) -> u32 {
        self.lost
    }

    /// Is `off..off + len` inside the region?
    pub fn holds(&self, off: u32, len: u32) -> bool {
        off.checked_add(len).is_some_and(|end| end <= self.size)
    }
}

/// When `N` growable buffers (a renderer's per-batch `Vec`s) may give
/// memory back: [`note`](Self::note) each buffer's length where it is
/// emptied, call [`end_frame`](Self::end_frame) once a frame, and every
/// `window` frames it hands back the window's peaks. [`shrink_to`] then
/// says whether a buffer holds enough more than its peak to be worth a
/// shrink.
///
/// Why (GitHub #110): one N64 frame drawing 16 rigid models (176 parts)
/// grew the batch buffers by ~24 KB for good; on the base N64 that is a
/// heap block nothing else can use, and a hole once the buffer moves.
#[derive(Clone, Debug)]
pub struct TrimWindow<const N: usize> {
    peaks: [u32; N],
    frames: u16,
    window: u16,
}

impl<const N: usize> TrimWindow<N> {
    /// A window of `window` frames (at least 1).
    pub const fn new(window: u16) -> Self {
        TrimWindow {
            peaks: [0; N],
            frames: 0,
            window: if window == 0 { 1 } else { window },
        }
    }

    /// Buffer `i` held `len` elements (before it was cleared).
    #[inline]
    pub fn note(&mut self, i: usize, len: usize) {
        let len = len.min(u32::MAX as usize) as u32;
        if len > self.peaks[i] {
            self.peaks[i] = len;
        }
    }

    /// Count a frame: at the window's end, the peaks it saw (and a new
    /// window starts), else `None`.
    pub fn end_frame(&mut self) -> Option<[u32; N]> {
        self.frames += 1;
        if self.frames < self.window {
            return None;
        }
        self.frames = 0;
        Some(core::mem::replace(&mut self.peaks, [0; N]))
    }
}

/// The capacity to shrink a buffer of `capacity` elements of `elem` bytes
/// to, given the `peak` its last window used: `Some(peak)` when it holds
/// more than twice that and more than `floor` bytes beyond it, else `None`
/// (small buffers and ones in use stay, so a steady game never reallocates).
pub fn shrink_to(capacity: usize, peak: u32, elem: usize, floor: usize) -> Option<usize> {
    let peak = peak as usize;
    let spare = capacity.saturating_sub(peak).saturating_mul(elem);
    (capacity > peak.saturating_mul(2) && spare > floor).then_some(peak)
}

/// A global allocator that counts what it holds: a PC or Web host wraps the
/// system allocator in it (`#[global_allocator] static A: Counting<System> =
/// Counting::new(System);`) and reports [`used`](Counting::used) and
/// [`peak`](Counting::peak) as its heap. Consoles read their allocator's
/// own statistics instead.
#[cfg(target_has_atomic = "ptr")]
pub struct Counting<A> {
    inner: A,
    used: core::sync::atomic::AtomicUsize,
    peak: core::sync::atomic::AtomicUsize,
}

#[cfg(target_has_atomic = "ptr")]
impl<A> Counting<A> {
    pub const fn new(inner: A) -> Self {
        Counting {
            inner,
            used: core::sync::atomic::AtomicUsize::new(0),
            peak: core::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn used(&self) -> usize {
        self.used.load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn peak(&self) -> usize {
        self.peak.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Start a new watermark from what is in use now.
    pub fn reset_peak(&self) {
        self.peak
            .store(self.used(), core::sync::atomic::Ordering::Relaxed);
    }

    fn grew(&self, bytes: usize) {
        use core::sync::atomic::Ordering::Relaxed;
        let now = self.used.fetch_add(bytes, Relaxed) + bytes;
        self.peak.fetch_max(now, Relaxed);
    }

    fn shrank(&self, bytes: usize) {
        self.used
            .fetch_sub(bytes, core::sync::atomic::Ordering::Relaxed);
    }
}

/// A heap a host can read: what [`Counting`] counts, handed to a platform
/// as `&'static dyn HeapStats` (the allocator is a `static`).
pub trait HeapStats: Sync {
    /// Bytes allocated now.
    fn used(&self) -> usize;
    /// The most bytes allocated at once since the start (or the last
    /// [`Counting::reset_peak`]).
    fn peak(&self) -> usize;
}

#[cfg(target_has_atomic = "ptr")]
impl<A: Sync> HeapStats for Counting<A> {
    fn used(&self) -> usize {
        Counting::used(self)
    }

    fn peak(&self) -> usize {
        Counting::peak(self)
    }
}

#[cfg(target_has_atomic = "ptr")]
unsafe impl<A: core::alloc::GlobalAlloc> core::alloc::GlobalAlloc for Counting<A> {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged.
        let p = unsafe { self.inner.alloc(layout) };
        if !p.is_null() {
            self.grew(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: core::alloc::Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged.
        let p = unsafe { self.inner.alloc_zeroed(layout) };
        if !p.is_null() {
            self.grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        // SAFETY: forwarded unchanged.
        unsafe { self.inner.dealloc(ptr, layout) };
        self.shrank(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: core::alloc::Layout, new: usize) -> *mut u8 {
        // SAFETY: forwarded unchanged.
        let p = unsafe { self.inner.realloc(ptr, layout, new) };
        if !p.is_null() {
            self.shrank(layout.size());
            self.grew(new);
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_fit_best_merge_and_count() {
        let mut b = Blocks::<8>::new(1000);
        assert_eq!(b.size(), 992, "rounded down to 16");
        let x = b.alloc(100).unwrap();
        let y = b.alloc(1).unwrap();
        let z = b.alloc(200).unwrap();
        assert_eq!((x, y, z), (0, 112, 128));
        assert_eq!(b.used(), 112 + 16 + 208);
        // A hole between x and z: a small block takes it (best fit), a
        // big one goes after z.
        b.free(y, 1);
        assert_eq!(b.alloc(10), Some(112));
        b.free(112, 10);
        assert_eq!(b.alloc(300), Some(336));
        // Freeing everything merges back into one range.
        for (off, len) in [(0, 100), (128, 200), (336, 300)] {
            b.free(off, len);
        }
        assert_eq!((b.used(), b.largest_free()), (0, 992));
        assert_eq!(b.high_water(), 112 + 208 + 304, "x, z and the 300");
        assert!(b.alloc(993).is_none());
        assert!(b.holds(976, 16) && !b.holds(976, 17));
    }

    #[test]
    fn blocks_give_a_block_up_when_the_list_is_full() {
        // Two free ranges at most: alternate blocks free make a third.
        let mut b = Blocks::<2>::new(16 * 8);
        let offs: [u32; 8] = core::array::from_fn(|_| b.alloc(16).unwrap());
        for &o in offs.iter().step_by(2) {
            b.free(o, 16);
        }
        assert_eq!(b.lost(), 32, "the 3rd and 4th holes are given up");
        assert_eq!(b.used(), 64);
        // A neighbor's free still merges into a listed range.
        b.free(offs[1], 16);
        assert_eq!(b.largest_free(), 48);
    }

    #[test]
    fn trim_window_hands_back_peaks_per_window() {
        let mut t = TrimWindow::<2>::new(3);
        t.note(0, 500);
        t.note(0, 20);
        t.note(1, 7);
        assert_eq!(t.end_frame(), None);
        assert_eq!(t.end_frame(), None);
        assert_eq!(t.end_frame(), Some([500, 7]));
        // A new window starts from zero.
        t.note(1, 3);
        t.end_frame();
        t.end_frame();
        assert_eq!(t.end_frame(), Some([0, 3]));
        // 1024 triangles of 88 bytes held after a window of 100: shrink;
        // a buffer in use, or one under the floor, stays.
        assert_eq!(shrink_to(1024, 100, 88, 16 * 1024), Some(100));
        assert_eq!(shrink_to(1024, 600, 88, 16 * 1024), None);
        assert_eq!(shrink_to(64, 0, 88, 16 * 1024), None);
    }

    #[test]
    fn frame_arena_bumps_aligns_and_resets() {
        let mut a = FrameArena::<64>::new();
        let x = a.alloc(1u8).unwrap();
        let y = a.alloc_slice(3, 7u32).unwrap();
        assert_eq!(y.as_ptr() as usize % 4, 0, "u32s aligned");
        *x = 9;
        y[2] = 5;
        assert_eq!((*x, y[0], y[2]), (9, 7, 5));
        assert!(a.used() >= 13);
        assert!(a.alloc_slice(100, 0u8).is_none(), "full");
        let used = a.used();
        a.reset();
        assert_eq!(a.used(), 0);
        assert_eq!(a.high_water(), used);
        assert_eq!(a.alloc_bytes(b"hello").unwrap(), b"hello");
    }

    #[test]
    fn arena_over_borrowed_memory() {
        // Aligned: a bare `[u8; 32]` may start at an odd address (Miri
        // places it so), and 16 `u16`s then don't fit after the padding.
        #[repr(align(8))]
        struct Mem([MaybeUninit<u8>; 32]);
        let mut mem = Mem([MaybeUninit::<u8>::uninit(); 32]);
        let mut a = Arena::new(&mut mem.0);
        assert_eq!(a.alloc_slice(8, 1u16).unwrap().len(), 8);
        assert!(a.alloc_slice(16, 1u16).is_none());
        a.reset();
        assert!(a.alloc_slice(16, 1u16).is_some());
        assert_eq!(a.capacity(), 32);
    }

    struct Fake(crate::LoadState);
    impl crate::Platform for Fake {
        fn now_micros(&self) -> u64 {
            0
        }
        fn log(&self, _: &str) {}
        fn group_state(&self, _: crate::GroupId) -> crate::LoadState {
            self.0
        }
    }

    #[test]
    fn level_arena_resets_when_its_group_goes() {
        let mut mem = [MaybeUninit::<u8>::uninit(); 32];
        let g = crate::GroupId::from_name("level1");
        let mut a = LevelArena::new(g, &mut mem);
        let mut p = Fake(crate::LoadState::Ready);
        assert!(!a.sync(&p));
        a.arena().alloc(5u32).unwrap();
        assert!(!a.sync(&p), "ready: kept");
        p.0 = crate::LoadState::Unloaded;
        assert!(a.sync(&p));
        assert_eq!(a.arena().used(), 0);
    }

    #[test]
    fn counting_allocator_follows_use_and_peak() {
        use core::alloc::{GlobalAlloc, Layout};
        let c = Counting::new(std::alloc::System);
        let l = Layout::from_size_align(1000, 8).unwrap();
        unsafe {
            let p = c.alloc(l);
            assert_eq!(c.used(), 1000);
            let q = c.realloc(p, l, 3000);
            assert_eq!(c.used(), 3000);
            c.dealloc(q, Layout::from_size_align(3000, 8).unwrap());
        }
        assert_eq!(c.used(), 0);
        assert_eq!(c.peak(), 3000);
        c.reset_peak();
        assert_eq!(c.peak(), 0);
    }
}
