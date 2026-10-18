//! LZ4 block compression for asset bundles (issue #56): the encoder side of
//! `trino_core::lz4`. A hash-chain match finder with one-step lazy matching
//! (what LZ4's own HC mode does), deterministic, no dependency. It trades
//! bake time for ratio: the consoles decode it, so a smaller block is less
//! ROM and fewer bytes read at load.
//!
//! Block rules kept (the LZ4 spec's end-of-block conditions): the last five
//! bytes are literals and no match starts in the last twelve.

const MIN_MATCH: usize = 4;
const WINDOW: usize = 65_535;
const HASH_BITS: u32 = 16;
const CHAIN: usize = 256;
/// The end-of-block margins of the LZ4 format.
const LAST_LITERALS: usize = 5;
const MF_LIMIT: usize = 12;

fn hash4(b: &[u8], i: usize) -> usize {
    let v = u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    (v.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
}

struct Finder {
    head: Vec<u32>,
    prev: Vec<u32>,
    /// Next position to insert into the chains.
    next: usize,
}

const NONE: u32 = u32::MAX;

impl Finder {
    fn new() -> Self {
        Finder {
            head: vec![NONE; 1 << HASH_BITS],
            prev: vec![NONE; WINDOW + 1],
            next: 0,
        }
    }

    /// Insert every position up to (not including) `upto`.
    fn insert_until(&mut self, src: &[u8], upto: usize) {
        while self.next < upto {
            let i = self.next;
            if i + MIN_MATCH <= src.len() {
                let h = hash4(src, i);
                self.prev[i % (WINDOW + 1)] = self.head[h];
                self.head[h] = i as u32;
            }
            self.next += 1;
        }
    }

    /// The longest match for `pos` (length, offset), within `limit` bytes.
    fn best(&mut self, src: &[u8], pos: usize, limit: usize) -> (usize, usize) {
        self.insert_until(src, pos);
        if pos + MIN_MATCH > limit {
            return (0, 0);
        }
        let mut cand = self.head[hash4(src, pos)];
        let (mut best_len, mut best_off) = (0, 0);
        let mut depth = 0;
        while cand != NONE && depth < CHAIN {
            let c = cand as usize;
            if pos - c > WINDOW {
                break;
            }
            if src[c + best_len.min(limit - pos - 1)] == src[pos + best_len.min(limit - pos - 1)] {
                let mut n = 0;
                while pos + n < limit && src[c + n] == src[pos + n] {
                    n += 1;
                }
                if n > best_len {
                    best_len = n;
                    best_off = pos - c;
                    if pos + n == limit {
                        break;
                    }
                }
            }
            let next = self.prev[c % (WINDOW + 1)];
            if next == NONE || next as usize >= c {
                break;
            }
            cand = next;
            depth += 1;
        }
        if best_len < MIN_MATCH {
            (0, 0)
        } else {
            (best_len, best_off)
        }
    }
}

fn push_length(out: &mut Vec<u8>, mut n: usize) {
    while n >= 255 {
        out.push(255);
        n -= 255;
    }
    out.push(n as u8);
}

fn sequence(out: &mut Vec<u8>, literals: &[u8], m: Option<(usize, usize)>) {
    let lit = literals.len();
    let ml = m.map_or(0, |(len, _)| len - MIN_MATCH);
    out.push(((lit.min(15) as u8) << 4) | ml.min(15) as u8);
    if lit >= 15 {
        push_length(out, lit - 15);
    }
    out.extend_from_slice(literals);
    if let Some((_, off)) = m {
        out.extend_from_slice(&(off as u16).to_le_bytes());
        if ml >= 15 {
            push_length(out, ml - 15);
        }
    }
}

/// Compress `src` into one LZ4 block (`trino_core::lz4::decompress` reads it).
pub fn compress(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() / 2 + 16);
    let n = src.len();
    if n < MF_LIMIT + 1 {
        sequence(&mut out, src, None);
        return out;
    }
    // Matches may start before `n - MF_LIMIT` and must end before the last
    // literals.
    let match_start_limit = n - MF_LIMIT;
    let match_end_limit = n - LAST_LITERALS;
    let mut f = Finder::new();
    let mut anchor = 0;
    let mut pos = 0;
    while pos < match_start_limit {
        let (len, off) = f.best(src, pos, match_end_limit);
        if len == 0 {
            pos += 1;
            continue;
        }
        // One step of lazy matching: a longer match one byte later wins.
        let (len, off, at) = if pos + 1 < match_start_limit {
            let (l2, o2) = f.best(src, pos + 1, match_end_limit);
            if l2 > len + 1 {
                (l2, o2, pos + 1)
            } else {
                (len, off, pos)
            }
        } else {
            (len, off, pos)
        };
        sequence(&mut out, &src[anchor..at], Some((len, off)));
        pos = at + len;
        anchor = pos;
    }
    sequence(&mut out, &src[anchor..], None);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(src: &[u8]) -> usize {
        let block = compress(src);
        let mut out = vec![0u8; src.len()];
        trino_core::lz4::decompress_exact(&block, &mut out).expect("decodes");
        assert_eq!(out, src);
        block.len()
    }

    #[test]
    fn round_trips_every_shape() {
        round_trip(b"");
        round_trip(b"a");
        round_trip(b"hello world");
        round_trip(&[0u8; 1000]);
        round_trip(&(0..=255u8).cycle().take(70_000).collect::<Vec<_>>());
        // Pseudo-random bytes: incompressible, still a valid block.
        let mut x = 12345u32;
        let noise: Vec<u8> = (0..5000)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (x >> 16) as u8
            })
            .collect();
        round_trip(&noise);
        let text = b"the quick brown fox jumps over the lazy dog. ".repeat(200);
        assert!(
            round_trip(&text) < text.len() / 10,
            "repetitive text shrinks"
        );
    }

    #[test]
    fn real_assets_shrink_and_round_trip() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shared");
        let mut total = (0usize, 0usize);
        for f in ["models/castle.glb", "sprites/brick.png", "fonts/ui.png"] {
            if let Ok(bytes) = std::fs::read(root.join(f)) {
                total.0 += bytes.len();
                total.1 += round_trip(&bytes);
            }
        }
        assert!(total.1 <= total.0 + total.0 / 200 + 64, "{total:?}");
    }

    #[test]
    fn deterministic() {
        let text = b"abcabcabcabcabcabcabc-xyz-abcabcabc".repeat(50);
        assert_eq!(compress(&text), compress(&text));
    }
}
