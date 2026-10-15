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
pub const KEY_WORDS: usize = 16;

/// Everything a memoized command depends on, as words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoKey(pub [u32; KEY_WORDS]);

impl MemoKey {
    /// The key of drawing `model` (a handle) with `matrix` and `params`
    /// under generation `generation`, or `None` when the draw is not a
    /// plain one (emissive, double-sided, animated or textured draws are
    /// never memoized: they are rare and their commands depend on more).
    /// A new [`ModelParams`] field must be added here or excluded.
    pub fn new(model: u32, generation: u32, matrix: &Mat34, params: &ModelParams) -> Option<Self> {
        let e = params.emissive;
        if e.r | e.g | e.b != 0
            || params.double_sided
            || params.anim.is_some()
            || params.texture.is_some()
        {
            return None;
        }
        let mut k = [0u32; KEY_WORDS];
        k[0] = model;
        k[1] = generation;
        let t = params.tint;
        k[2] = u32::from_be_bytes([t.r, t.g, t.b, t.a]);
        k[3] = params.shading as u32;
        for (r, row) in matrix.m.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                k[4 + r * 4 + c] = v.to_bits();
            }
        }
        Some(MemoKey(k))
    }

    /// FNV-1a over the words: the slot.
    fn slot(&self, n: usize) -> usize {
        let mut h = 0x811c_9dc5u32;
        for w in self.0 {
            h = (h ^ w).wrapping_mul(0x0100_0193);
        }
        (h ^ (h >> 16)) as usize % n.max(1)
    }
}

/// One memo entry: the key, the command (`W` words) and one extra word
/// for the backend (e.g. the triangle count for its statistics).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoSlot<const W: usize> {
    key: MemoKey,
    pub words: [u32; W],
    pub extra: u32,
}

impl<const W: usize> MemoSlot<W> {
    /// An empty slot (model handle 0 with generation `u32::MAX` never
    /// matches a real key: backends start at generation 0 and a handle
    /// is a path hash).
    pub const EMPTY: MemoSlot<W> = MemoSlot {
        key: MemoKey([0, u32::MAX, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        words: [0; W],
        extra: 0,
    };
}

/// The memoized command for `key`, if `slots` holds it.
pub fn find<'a, const W: usize>(
    slots: &'a [MemoSlot<W>],
    key: &MemoKey,
) -> Option<&'a MemoSlot<W>> {
    if slots.is_empty() {
        return None;
    }
    let s = &slots[key.slot(slots.len())];
    (s.key == *key).then_some(s)
}

/// Remember `words` (and `extra`) for `key`, replacing whatever shared its
/// slot.
pub fn store<const W: usize>(slots: &mut [MemoSlot<W>], key: MemoKey, words: [u32; W], extra: u32) {
    if slots.is_empty() {
        return;
    }
    let i = key.slot(slots.len());
    slots[i] = MemoSlot { key, words, extra };
}
