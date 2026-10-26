//! A small deterministic mutator for the stable test run: the same seeds
//! and the same `seed` give the same inputs on every machine, so a failure
//! in CI reproduces locally (the input is written to
//! `target/fuzz-smoke/<target>/` for `cargo xtask fuzz repro`).
//!
//! It is no replacement for libFuzzer's coverage guidance; it holds the
//! cheap invariants on every PR: truncations at every length class, bit
//! flips, and header words set to the values that break size arithmetic.

/// xorshift64*.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

/// Words that break length and offset arithmetic.
const WORDS: [u32; 10] = [
    0,
    1,
    2,
    0x7F,
    0xFF,
    0xFFFF,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFFE,
    0xFFFF_FFFF,
];

/// One mutation of `input` (another seed in `other` may be spliced in).
pub fn mutate(rng: &mut Rng, input: &[u8], other: &[u8], max_len: usize) -> Vec<u8> {
    let mut v = input.to_vec();
    let steps = 1 + rng.below(4);
    for _ in 0..steps {
        let len = v.len();
        match rng.below(9) {
            // Truncate (a torn write, a short read).
            0 => v.truncate(rng.below(len + 1)),
            // Flip one bit.
            1 if len > 0 => {
                let i = rng.below(len);
                v[i] ^= 1 << rng.below(8);
            }
            // A byte set to an edge value.
            2 if len > 0 => {
                let i = rng.below(len);
                v[i] = [0, 1, 0x7F, 0x80, 0xFF][rng.below(5)];
            }
            // A little-endian word (counts, offsets, lengths) set to an
            // edge value, mostly inside the header.
            3 if len >= 4 => {
                let span = if rng.below(3) == 0 {
                    len - 3
                } else {
                    (len - 3).min(64)
                };
                let i = rng.below(span) & !1;
                let i = i.min(len - 4);
                let w = match rng.below(3) {
                    0 => len as u32,
                    1 => (len as u32).wrapping_add(rng.below(16) as u32),
                    _ => WORDS[rng.below(WORDS.len())],
                };
                v[i..i + 4].copy_from_slice(&w.to_le_bytes());
            }
            // A 16-bit field set to an edge value.
            4 if len >= 2 => {
                let i = rng.below((len - 1).min(64));
                let w = [0u16, 1, 0x7FFF, 0x8000, 0xFFFF][rng.below(5)];
                v[i..i + 2].copy_from_slice(&w.to_le_bytes());
            }
            // Delete a run.
            5 if len > 0 => {
                let i = rng.below(len);
                let n = 1 + rng.below((len - i).min(32));
                v.drain(i..i + n);
            }
            // Insert random bytes.
            6 => {
                let i = rng.below(len + 1);
                let n = 1 + rng.below(16);
                let bytes: Vec<u8> = (0..n).map(|_| rng.next_u64() as u8).collect();
                v.splice(i..i, bytes);
            }
            // Duplicate a run (repeated records).
            7 if len > 0 => {
                let i = rng.below(len);
                let n = 1 + rng.below((len - i).min(64));
                let run = v[i..i + n].to_vec();
                let at = rng.below(len + 1);
                v.splice(at..at, run);
            }
            // Splice the tail of another seed.
            8 if !other.is_empty() => {
                let i = rng.below(len + 1);
                let j = rng.below(other.len());
                v.truncate(i);
                v.extend_from_slice(&other[j..]);
            }
            _ => {}
        }
    }
    v.truncate(max_len);
    v
}
