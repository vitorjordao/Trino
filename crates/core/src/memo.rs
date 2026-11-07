//! A small direct-mapped memo for per-draw commands that depend only on
//! the draw (model, matrix, surface) and on the camera and light: a
//! backend that turns a draw into a command for its hardware (the N64's
//! RSP overlay) reuses last frame's command when nothing changed.
//!
//! In a scene where most things stand still under a still camera, almost
//! every draw repeats bit for bit from one frame to the next. On the N64 a
//! draw's eligibility test and command cost ~100 µs of CPU, mostly float
//! latency and cache misses (measured in ares), and a memo hit costs a
//! hash and a copy.
//!
//! The memo never decides anything by itself: the backend computes the
//! command on a miss and stores it. A key covers everything the command
//! depends on, including a *generation* the backend bumps whenever its
//! camera, light or meshes change, so a stale entry can never match.
//! No allocation: the backend owns the slots (on the console, on the heap:
//! `vec![MemoSlot::EMPTY; n]`).

use crate::math3d::Mat34;
use crate::render::ModelParams;

/// Words in a [`MemoKey`].
pub const KEY_WORDS: usize = 18;

/// Everything a memoized command depends on, as words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoKey(pub [u32; KEY_WORDS]);

impl MemoKey {
    /// The key of drawing `model` (a handle) with `matrix` and `params`
    /// under generation `generation`, or `None` when the draw is not a
    /// plain one (emissive, animated or palette draws are never memoized:
    /// they are rare and their commands depend on more; a palette alone is
    /// 4 words the key has no room for; water moves every frame). A
    /// textured draw is keyed by its texture, filter and detail layer.
    /// Double-sided draws are keyed (their command differs only in its cull
    /// mode).
    /// A new [`ModelParams`] field must be added here or excluded.
    /// `gloss` is excluded: only the per-pixel tier (PC) reads it, and the
    /// memo serves the N64's RSP draws. `shadow` is excluded too: it adds a
    /// shadow draw beside the command and never changes the command.
    pub fn new(model: u32, generation: u32, matrix: &Mat34, params: &ModelParams) -> Option<Self> {
        let e = params.emissive;
        if e.r | e.g | e.b != 0
            || params.anim.is_some()
            || params.palette.is_some()
            || params.water.is_some()
        {
            return None;
        }
        let mut k = [0u32; KEY_WORDS];
        k[0] = model;
        k[1] = generation;
        let t = params.tint;
        k[2] = u32::from_be_bytes([t.r, t.g, t.b, t.a]);
        k[3] = params.shading as u32
            | (params.double_sided as u32) << 8
            | (params.filter as u32) << 16
            | params.detail.map_or(0, |d| 1 + d.scale_log2 as u32) << 24;
        for (r, row) in matrix.m.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                k[4 + r * 4 + c] = v.to_bits();
            }
        }
        k[16] = params.texture.map_or(0, |t| t.0);
        k[17] = params.detail.map_or(0, |d| d.sprite.0);
        Some(MemoKey(k))
    }

    /// [`MemoKey::new`] for a backend that re-places a remembered command
    /// itself (GitHub #139): the matrix's translation enters the key only
    /// coarsely — each coordinate's float with its 16 low mantissa bits
    /// cleared, a cell 1/128 of the coordinate's magnitude wide — so a
    /// draw that moved a little (someone walking) finds its last command
    /// and the backend moves it, instead of building a new one. The
    /// backend must compare the exact translation it stored with the
    /// draw's: two draws in one cell share an entry.
    #[inline]
    pub fn placed(
        model: u32,
        generation: u32,
        matrix: &Mat34,
        params: &ModelParams,
    ) -> Option<Self> {
        let mut k = Self::new(model, generation, matrix, params)?;
        for i in [7, 11, 15] {
            k.0[i] &= 0xFFFF_0000;
        }
        Some(k)
    }

    /// FNV-1a over the words: the slot (a multiply-shift into `n`, no
    /// division: GitHub #139 measured the `%` among the draw path's costs).
    #[inline]
    fn slot(&self, n: usize) -> usize {
        let mut h = 0x811c_9dc5u32;
        for w in self.0 {
            h = (h ^ w).wrapping_mul(0x0100_0193);
        }
        let h = h ^ (h >> 16);
        ((h as u64 * n as u64) >> 32) as usize
    }
}

/// Slots a key may live in: a set of this many, next to each other
/// (GitHub #139). Direct-mapped, ~100 draws over 1024 slots left about five
/// pairs of draws sharing a slot (the birthday bound), each pair rebuilding
/// the other's command every frame; with four ways and the least recently
/// used one replaced, a frame's draws keep their entries.
pub const WAYS: usize = 4;

/// One memo entry: the key, the command (`W` words), one extra word for
/// the backend (e.g. the triangle count for its statistics) and when it was
/// last used ([`touch`], a backend's frame counter: the least recent way of
/// a set is the one [`store_at`] replaces).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoSlot<const W: usize> {
    key: MemoKey,
    pub words: [u32; W],
    pub extra: u32,
    pub used: u32,
}

impl<const W: usize> MemoSlot<W> {
    /// An empty slot (model handle 0 with generation `u32::MAX` never
    /// matches a real key: backends start at generation 0 and a handle
    /// is a path hash).
    pub const EMPTY: MemoSlot<W> = MemoSlot {
        key: MemoKey([0, u32::MAX, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        words: [0; W],
        extra: 0,
        used: 0,
    };

    fn is_empty(&self) -> bool {
        words_eq(&self.key.0, &MemoSlot::<W>::EMPTY.key.0)
    }
}

/// Whether `a` and `b` hold the same words. Not `==`: on the N64 that is a
/// call to newlib's `memcmp`, which compares byte by byte (~600 cycles for
/// a memo key; the water draws spent ~10% of their CPU there, issue #69).
pub fn words_eq(a: &[u32], b: &[u32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

/// `key`'s set: its first slot (`slots.len()` a multiple of [`WAYS`]; a
/// shorter tail is never used).
#[inline]
fn set_of<const W: usize>(slots: &[MemoSlot<W>], key: &MemoKey) -> Option<usize> {
    let sets = slots.len() / WAYS;
    (sets > 0).then(|| key.slot(sets) * WAYS)
}

/// The way of `key`'s set that holds it.
#[inline]
fn way_of<const W: usize>(slots: &[MemoSlot<W>], key: &MemoKey) -> Option<usize> {
    let first = set_of(slots, key)?;
    // The model handle first: most ways differ there.
    (first..first + WAYS)
        .find(|&i| slots[i].key.0[0] == key.0[0] && words_eq(&slots[i].key.0, &key.0))
}

/// The memoized command for `key`, if `slots` holds it.
pub fn find<'a, const W: usize>(
    slots: &'a [MemoSlot<W>],
    key: &MemoKey,
) -> Option<&'a MemoSlot<W>> {
    way_of(slots, key).map(|i| &slots[i])
}

/// [`find`], to update the command in place (a backend that patches a
/// remembered command keeps the patched one; it marks the use with
/// [`touch`]).
pub fn find_mut<'a, const W: usize>(
    slots: &'a mut [MemoSlot<W>],
    key: &MemoKey,
) -> Option<&'a mut MemoSlot<W>> {
    way_of(slots, key).map(|i| &mut slots[i])
}

/// Mark `slot` used at `now` (a backend's frame counter).
#[inline]
pub fn touch<const W: usize>(slot: &mut MemoSlot<W>, now: u32) {
    slot.used = now;
}

/// The way [`store_at`] writes `key` to: its own, an empty one, else the
/// least recently used.
fn victim<const W: usize>(slots: &[MemoSlot<W>], key: &MemoKey) -> Option<usize> {
    let first = set_of(slots, key)?;
    let set = first..first + WAYS;
    way_of(slots, key)
        .or_else(|| set.clone().find(|&i| slots[i].is_empty()))
        .or_else(|| set.min_by_key(|&i| slots[i].used))
}

/// Whether storing `key` would replace another draw's entry (every way of
/// its set holds a different, real key). Instrumentation (issue #79) asks
/// before [`store`].
pub fn evicts<const W: usize>(slots: &[MemoSlot<W>], key: &MemoKey) -> bool {
    victim(slots, key).is_some_and(|i| !slots[i].is_empty() && !words_eq(&slots[i].key.0, &key.0))
}

/// Remember `words` (and `extra`) for `key`, used at `now`: in its own way,
/// an empty one, or over the least recently used of its set.
pub fn store_at<const W: usize>(
    slots: &mut [MemoSlot<W>],
    key: MemoKey,
    words: [u32; W],
    extra: u32,
    now: u32,
) {
    if let Some(i) = victim(slots, &key) {
        slots[i] = MemoSlot {
            key,
            words,
            extra,
            used: now,
        };
    }
}

/// [`store_at`] with no clock (every entry equally old).
pub fn store<const W: usize>(slots: &mut [MemoSlot<W>], key: MemoKey, words: [u32; W], extra: u32) {
    store_at(slots, key, words, extra, 0);
}
