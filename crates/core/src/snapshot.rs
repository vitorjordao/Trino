//! Rollback snapshots of game state (issue #67): what
//! [`Game::save_state`](crate::Game::save_state) and
//! [`Game::load_state`](crate::Game::load_state) write and read.
//!
//! A rollback session saves the state every frame and loads one back when a
//! late remote input arrives, so a snapshot must be cheap and complete. A
//! snapshot never leaves the process that took it (sessions exchange inputs
//! and state hashes), so a `Copy` value can be copied as raw bytes: its
//! padding and any `&'static` it holds stay valid in the same process.
//! [`save_copy`] and [`load_copy`] do that behind a 4-byte size tag, so a
//! snapshot of another type (another build, another game) is refused
//! instead of read.

/// Bytes [`save_copy`] writes for a `T`.
pub const fn size_of_copy<T: Copy>() -> usize {
    4 + core::mem::size_of::<T>()
}

/// Copy `value` into `out` (its size, then its bytes). `None` when `out` is
/// shorter than [`size_of_copy::<T>`](size_of_copy).
pub fn save_copy<T: Copy>(value: &T, out: &mut [u8]) -> Option<usize> {
    let n = core::mem::size_of::<T>();
    let total = 4 + n;
    let out = out.get_mut(..total)?;
    out[..4].copy_from_slice(&(n as u32).to_le_bytes());
    // SAFETY: `T: Copy` has no drop glue; reading its `n` bytes (padding
    // included, which a snapshot only ever copies back) is defined for a
    // byte copy into a `u8` buffer of the same length.
    unsafe {
        core::ptr::copy_nonoverlapping(value as *const T as *const u8, out[4..].as_mut_ptr(), n);
    }
    Some(total)
}

/// Read a [`save_copy`] back into `value`. `false` (and `value` untouched)
/// when `bytes` is not a `T`'s snapshot.
pub fn load_copy<T: Copy>(value: &mut T, bytes: &[u8]) -> bool {
    let n = core::mem::size_of::<T>();
    if bytes.len() != 4 + n || bytes[..4] != (n as u32).to_le_bytes() {
        return false;
    }
    // SAFETY: the bytes were written by `save_copy::<T>` in this process
    // (sessions never send snapshots), so they are a valid `T`.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes[4..].as_ptr(), value as *mut T as *mut u8, n);
    }
    true
}

/// [`save_copy`] for a type that is plain data without being `Copy` (a
/// game struct that is only `Clone` because it is big).
///
/// # Safety
///
/// `T` must own nothing a byte copy would duplicate: no `Box`, `Vec`,
/// `Rc`, handle or file — only values, arrays and `&'static` borrows (a
/// `no_std` game without `alloc` is that by construction). Then a byte copy
/// is as good as a clone and overwriting needs no drop.
pub unsafe fn save_raw<T>(value: &T, out: &mut [u8]) -> Option<usize> {
    let n = core::mem::size_of::<T>();
    let total = 4 + n;
    let out = out.get_mut(..total)?;
    out[..4].copy_from_slice(&(n as u32).to_le_bytes());
    // SAFETY: the caller's contract (plain data); a byte copy out.
    unsafe {
        core::ptr::copy_nonoverlapping(value as *const T as *const u8, out[4..].as_mut_ptr(), n);
    }
    Some(total)
}

/// [`load_copy`] for [`save_raw`]'s snapshots.
///
/// # Safety
///
/// As [`save_raw`]; `bytes` must come from `save_raw::<T>` in this process.
pub unsafe fn load_raw<T>(value: &mut T, bytes: &[u8]) -> bool {
    let n = core::mem::size_of::<T>();
    if bytes.len() != 4 + n || bytes[..4] != (n as u32).to_le_bytes() {
        return false;
    }
    // SAFETY: the caller's contract.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes[4..].as_ptr(), value as *mut T as *mut u8, n);
    }
    true
}

/// Bytes [`save_raw`] writes for a `T`.
pub const fn size_of_raw<T>() -> usize {
    4 + core::mem::size_of::<T>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Debug)]
    struct State {
        pos: (f32, f32),
        tick: u32,
        name: &'static str,
        flag: bool,
    }

    #[test]
    fn a_copy_round_trips_and_a_foreign_snapshot_is_refused() {
        let s = State {
            pos: (1.5, -2.0),
            tick: 99,
            name: "hero",
            flag: true,
        };
        let mut buf = [0u8; 64];
        let n = save_copy(&s, &mut buf).unwrap();
        assert_eq!(n, size_of_copy::<State>());
        let mut t = State {
            pos: (0.0, 0.0),
            tick: 0,
            name: "",
            flag: false,
        };
        assert!(load_copy(&mut t, &buf[..n]));
        assert_eq!(t, s);
        assert!(save_copy(&s, &mut buf[..n - 1]).is_none());
        assert!(!load_copy(&mut t, &buf[..n - 1]));
        let mut other = 0u64;
        assert!(!load_copy(&mut other, &buf[..n]));
    }
}
