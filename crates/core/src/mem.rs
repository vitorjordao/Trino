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
        let mut mem = [MaybeUninit::<u8>::uninit(); 32];
        let mut a = Arena::new(&mut mem);
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
