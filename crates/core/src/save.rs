//! Save-data framing shared by the console backends.
//!
//! **Files** (a 3DS save, tests) hold `header ‖ data`, where the 8-byte
//! header is `"TS" u16 len u32 fnv1a(data)` (little-endian). A torn write or
//! a blank/foreign medium fails the check and reads back as "no save"
//! instead of garbage — games only ever see their own last good data.
//!
//! **Raw media** (the N64 EEPROM, [`Medium`]) hold two **slots**, A at 0
//! and B at half the medium. Each slot is a [`SLOT_HEADER`]-byte header
//! `"T2" seq:u16 stored:u16 raw:u16 fnv1a(stored):u32 flags:u8 0 0 0` and
//! the stored bytes, LZ-compressed ([`compress`]) when that is smaller. A
//! save goes to the slot that does **not** hold the newest valid one, body
//! first and header last, so a power cut mid-write leaves the previous save
//! readable: [`read_newest`] picks the valid slot with the highest sequence
//! number. A save too big for a slot is written **wide** (one slot over the
//! whole medium), the only case without a fallback. Media written by the
//! older single-frame layout (`"TS"` at 0) still read back.
//!
//! Writing can be split across frames ([`plan_write`] then the body and the
//! header in any number of steps): the N64 EEPROM takes ~7.5 ms per 8-byte
//! block, so a 1 KB save in one go is a visible freeze.

/// Bytes of framing in front of the data.
pub const HEADER: usize = 8;
const MAGIC: [u8; 2] = *b"TS";

/// FNV-1a 32 over bytes (same function as asset handles).
pub fn checksum(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Header for `data` (callers guarantee `data.len() <= u16::MAX`).
pub fn header(data: &[u8]) -> [u8; HEADER] {
    let len = (data.len() as u16).to_le_bytes();
    let sum = checksum(data).to_le_bytes();
    [
        MAGIC[0], MAGIC[1], len[0], len[1], sum[0], sum[1], sum[2], sum[3],
    ]
}

/// Data length announced by `header`, or `None` if it is not a save header.
pub fn data_len(header: &[u8; HEADER]) -> Option<usize> {
    (header[0..2] == MAGIC).then(|| u16::from_le_bytes([header[2], header[3]]) as usize)
}

/// Does `data` match the checksum in `header`?
pub fn verify(header: &[u8; HEADER], data: &[u8]) -> bool {
    data_len(header) == Some(data.len())
        && checksum(data) == u32::from_le_bytes([header[4], header[5], header[6], header[7]])
}

// ---------------------------------------------------------------- slots

/// Bytes of a slot header (two 8-byte EEPROM blocks, so a body block never
/// shares a block with the header and the header can be written last).
pub const SLOT_HEADER: usize = 16;
const MAGIC2: [u8; 2] = *b"T2";
const FLAG_COMPRESSED: u8 = 1;
const FLAG_WIDE: u8 = 2;

/// A raw save medium addressed by byte (the N64 EEPROM; a RAM image in
/// tests). Writes may be torn at any point by a power cut.
pub trait Medium {
    /// Bytes on the medium.
    fn size(&self) -> usize;
    /// Read `buf.len()` bytes at `at`.
    fn read(&mut self, at: usize, buf: &mut [u8]);
    /// Write `data` at `at`.
    fn write(&mut self, at: usize, data: &[u8]);
}

/// Largest save [`plan_write`] accepts on a medium of `total` bytes (a wide
/// slot holding the data uncompressed).
pub const fn capacity(total: usize) -> usize {
    total.saturating_sub(SLOT_HEADER)
}

/// Largest stored (possibly compressed) body that fits one of the two
/// slots of a medium of `total` bytes.
pub const fn slot_capacity(total: usize) -> usize {
    (total / 2).saturating_sub(SLOT_HEADER)
}

/// A valid save found on a medium.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Found {
    /// Byte offset of its header (0 or half the medium).
    pub offset: usize,
    /// Sequence number (wrapping; higher = newer). Legacy frames are 0.
    pub seq: u16,
    /// Bytes stored after the header.
    pub stored: usize,
    /// Bytes after decompression.
    pub raw: usize,
    /// Body is [`compress`]ed.
    pub compressed: bool,
    /// Bytes of the medium it covers (header included).
    pub span: usize,
    /// Written by the single-frame layout (`"TS"` header, 8 bytes).
    pub legacy: bool,
}

fn slot_header(seq: u16, stored: &[u8], raw: usize, flags: u8) -> [u8; SLOT_HEADER] {
    let mut h = [0u8; SLOT_HEADER];
    h[0..2].copy_from_slice(&MAGIC2);
    h[2..4].copy_from_slice(&seq.to_le_bytes());
    h[4..6].copy_from_slice(&(stored.len() as u16).to_le_bytes());
    h[6..8].copy_from_slice(&(raw as u16).to_le_bytes());
    h[8..12].copy_from_slice(&checksum(stored).to_le_bytes());
    h[12] = flags;
    h
}

/// Is the slot at `offset` valid? `scratch` must hold the medium's size.
fn probe(m: &mut dyn Medium, offset: usize, scratch: &mut [u8]) -> Option<Found> {
    let total = m.size();
    if offset + SLOT_HEADER > total {
        return None;
    }
    let mut h = [0u8; SLOT_HEADER];
    m.read(offset, &mut h);
    if h[0..2] == MAGIC2 {
        let seq = u16::from_le_bytes([h[2], h[3]]);
        let stored = u16::from_le_bytes([h[4], h[5]]) as usize;
        let raw = u16::from_le_bytes([h[6], h[7]]) as usize;
        let sum = u32::from_le_bytes([h[8], h[9], h[10], h[11]]);
        let flags = h[12];
        let room = if flags & FLAG_WIDE != 0 || offset != 0 {
            total - offset - SLOT_HEADER
        } else {
            total / 2 - SLOT_HEADER
        };
        if stored > room || stored > scratch.len() || h[13..] != [0, 0, 0] {
            return None;
        }
        m.read(offset + SLOT_HEADER, &mut scratch[..stored]);
        if checksum(&scratch[..stored]) != sum {
            return None;
        }
        return Some(Found {
            offset,
            seq,
            stored,
            raw,
            compressed: flags & FLAG_COMPRESSED != 0,
            span: SLOT_HEADER + stored,
            legacy: false,
        });
    }
    // The single-frame layout only ever lived at 0.
    if offset == 0 {
        let h8: [u8; HEADER] = h[..HEADER].try_into().ok()?;
        let len = data_len(&h8).filter(|&l| l <= total - HEADER && l <= scratch.len())?;
        m.read(HEADER, &mut scratch[..len]);
        if verify(&h8, &scratch[..len]) {
            return Some(Found {
                offset: 0,
                seq: 0,
                stored: len,
                raw: len,
                compressed: false,
                span: HEADER + len,
                legacy: true,
            });
        }
    }
    None
}

/// `a` is newer than `b` (wrapping sequence numbers).
fn newer(a: u16, b: u16) -> bool {
    (a.wrapping_sub(b) as i16) > 0
}

/// The newest valid save on `m`, if any. `scratch` must hold `m.size()`
/// bytes.
pub fn find_newest(m: &mut dyn Medium, scratch: &mut [u8]) -> Option<Found> {
    let half = m.size() / 2;
    let a = probe(m, 0, scratch);
    let b = probe(m, half, scratch).filter(|_| half >= SLOT_HEADER);
    match (a, b) {
        // A wide or legacy save at 0 that reaches into B means B's header
        // is stale (or is its data): trust whichever was written last.
        (Some(a), Some(b)) => Some(if newer(b.seq, a.seq) || a.legacy {
            b
        } else {
            a
        }),
        (a, b) => a.or(b),
    }
}

/// Read the newest valid save into `out`: its length, 0 when the medium
/// holds none (blank, foreign or torn both ways). `scratch` must hold
/// `m.size()` bytes. Uncompressed data longer than `out` is cut (the length
/// returned is the full one); compressed data needs `out` to hold it all.
pub fn read_newest(m: &mut dyn Medium, out: &mut [u8], scratch: &mut [u8]) -> usize {
    let Some(f) = find_newest(m, scratch) else {
        return 0;
    };
    let body_at = f.offset + if f.legacy { HEADER } else { SLOT_HEADER };
    m.read(body_at, &mut scratch[..f.stored]);
    if f.compressed {
        // Straight into `out` (no stack buffer: console stacks are small).
        if out.len() < f.raw {
            return 0;
        }
        match decompress(&scratch[..f.stored], &mut out[..f.raw]) {
            Some(n) if n == f.raw => n,
            _ => 0,
        }
    } else {
        let k = f.stored.min(out.len());
        out[..k].copy_from_slice(&scratch[..k]);
        f.stored
    }
}

/// Where and what to write for the next save (see [`plan_write`]): the
/// body (`body[..body_len]`) goes at `offset + SLOT_HEADER`, then `header`
/// at `offset`. Until the header lands the previous save stays the newest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WritePlan {
    pub offset: usize,
    pub header: [u8; SLOT_HEADER],
    pub body_len: usize,
    /// The previous save survives a torn write (false only when the data
    /// needs a wide slot, or the previous save already covers both).
    pub safe: bool,
}

/// Prepare the next save of `data` on `m`: compresses into `body` (which
/// must hold `m.size()` bytes) and picks the slot that keeps the newest
/// valid save intact. `scratch` must hold `m.size()` bytes.
pub fn plan_write(
    m: &mut dyn Medium,
    data: &[u8],
    body: &mut [u8],
    scratch: &mut [u8],
    lz: &mut Lz,
) -> Result<WritePlan, crate::platform::SaveError> {
    let total = m.size();
    if data.len() > capacity(total) || data.len() > u16::MAX as usize {
        return Err(crate::platform::SaveError::TooLarge {
            len: data.len(),
            capacity: capacity(total),
        });
    }
    let newest = find_newest(m, scratch);
    let room = total.min(body.len());
    let packed = compress(data, &mut body[..room], lz);
    let (stored, flags) = match packed {
        Some(n) if n < data.len() => (n, FLAG_COMPRESSED),
        _ => {
            body[..data.len()].copy_from_slice(data);
            (data.len(), 0)
        }
    };
    let half = total / 2;
    let fits_slot = stored <= slot_capacity(total);
    let seq = newest.map_or(1, |f| f.seq.wrapping_add(1));
    let (offset, wide, safe) = match (fits_slot, newest) {
        (false, _) => (0, true, newest.is_none()),
        (true, None) => (0, false, true),
        // Newest in A: B is free.
        (true, Some(f)) if f.offset == 0 && f.span <= half => (half, false, true),
        // Newest in B: A is free.
        (true, Some(f)) if f.offset == half => (0, false, true),
        // A wide/legacy save over both halves: nothing is free.
        (true, Some(_)) => (half, false, false),
    };
    let flags = flags | if wide { FLAG_WIDE } else { 0 };
    Ok(WritePlan {
        offset,
        header: slot_header(seq, &body[..stored], data.len(), flags),
        body_len: stored,
        safe,
    })
}

/// [`plan_write`] and write it all now (body, then header).
pub fn write_now(
    m: &mut dyn Medium,
    data: &[u8],
    body: &mut [u8],
    scratch: &mut [u8],
    lz: &mut Lz,
) -> Result<WritePlan, crate::platform::SaveError> {
    let plan = plan_write(m, data, body, scratch, lz)?;
    m.write(plan.offset + SLOT_HEADER, &body[..plan.body_len]);
    m.write(plan.offset, &plan.header);
    Ok(plan)
}

// ------------------------------------------------------------ compression

/// LZSS scratch: a 3-byte hash chain over the input (no allocation; put it
/// on the heap or in a static on consoles, it is ~4.6 KB).
pub struct Lz {
    head: [u16; 256],
    prev: [u16; 2048],
}

impl Default for Lz {
    fn default() -> Self {
        Lz {
            head: [u16::MAX; 256],
            prev: [u16::MAX; 2048],
        }
    }
}

const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 18;
const WINDOW: usize = 4096;
const CHAIN: usize = 48;

fn hash3(b: &[u8]) -> usize {
    ((b[0] as usize) << 4 ^ (b[1] as usize) << 2 ^ b[2] as usize) & 255
}

/// LZSS-compress `src` (≤ 2048 bytes) into `dst`. Groups of eight items
/// behind a flag byte (bit set = match); a literal is one byte, a match two:
/// `(offset − 1) | (len − 3) << 12`, little-endian. Deterministic. `None`
/// when `dst` is too small or `src` too long.
pub fn compress(src: &[u8], dst: &mut [u8], lz: &mut Lz) -> Option<usize> {
    if src.len() > lz.prev.len() {
        return None;
    }
    lz.head = [u16::MAX; 256];
    let mut out = 0usize;
    let mut flag_at = 0usize;
    let mut items = 8u32;
    let mut i = 0usize;
    let put = |out: &mut usize, b: u8, dst: &mut [u8]| -> Option<()> {
        *dst.get_mut(*out)? = b;
        *out += 1;
        Some(())
    };
    let insert = |lz: &mut Lz, src: &[u8], at: usize| {
        if at + MIN_MATCH <= src.len() {
            let h = hash3(&src[at..]);
            lz.prev[at] = lz.head[h];
            lz.head[h] = at as u16;
        }
    };
    while i < src.len() {
        if items == 8 {
            flag_at = out;
            put(&mut out, 0, dst)?;
            items = 0;
        }
        // Longest match in the chain (first found wins ties: deterministic).
        let (mut best_len, mut best_off) = (0usize, 0usize);
        if i + MIN_MATCH <= src.len() {
            let mut cand = lz.head[hash3(&src[i..])];
            let mut depth = 0;
            while cand != u16::MAX && depth < CHAIN {
                let c = cand as usize;
                if i - c > WINDOW {
                    break;
                }
                let max = MAX_MATCH.min(src.len() - i);
                let mut l = 0;
                while l < max && src[c + l] == src[i + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_off = i - c;
                    if l == max {
                        break;
                    }
                }
                cand = lz.prev[c];
                depth += 1;
            }
        }
        if best_len >= MIN_MATCH {
            let tok = ((best_off - 1) as u16) | (((best_len - MIN_MATCH) as u16) << 12);
            *dst.get_mut(flag_at)? |= 1 << items;
            let [lo, hi] = tok.to_le_bytes();
            put(&mut out, lo, dst)?;
            put(&mut out, hi, dst)?;
            for k in 0..best_len {
                insert(lz, src, i + k);
            }
            i += best_len;
        } else {
            put(&mut out, src[i], dst)?;
            insert(lz, src, i);
            i += 1;
        }
        items += 1;
    }
    Some(out)
}

/// Inverse of [`compress`]: the decompressed length, `None` on malformed
/// input or when `dst` is too small.
pub fn decompress(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    let (mut i, mut out) = (0usize, 0usize);
    while i < src.len() {
        let flags = src[i];
        i += 1;
        for bit in 0..8 {
            if i >= src.len() {
                break;
            }
            if flags & (1 << bit) != 0 {
                let tok = u16::from_le_bytes([src[i], *src.get(i + 1)?]);
                i += 2;
                let off = (tok & 0x0FFF) as usize + 1;
                let len = (tok >> 12) as usize + MIN_MATCH;
                if off > out || out + len > dst.len() {
                    return None;
                }
                for k in 0..len {
                    dst[out + k] = dst[out + k - off];
                }
                out += len;
            } else {
                *dst.get_mut(out)? = src[i];
                i += 1;
                out += 1;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_and_corruption() {
        let data = b"stars=3;coins=42";
        let h = header(data);
        assert_eq!(data_len(&h), Some(data.len()));
        assert!(verify(&h, data));
        let mut torn = *data;
        torn[3] ^= 1;
        assert!(!verify(&h, &torn), "a flipped bit reads as no save");
        assert_eq!(data_len(&[0xFF; HEADER]), None, "blank EEPROM");
        assert!(verify(&header(&[]), &[]));
    }

    #[test]
    fn lz_round_trips_and_shrinks_repetitive_data() {
        let mut lz = Lz::default();
        let mut data = [0u8; 1500];
        for (i, b) in data.iter_mut().enumerate() {
            *b = if i % 7 == 0 { (i / 7) as u8 } else { 0 };
        }
        let mut packed = [0u8; 2048];
        let n = compress(&data, &mut packed, &mut lz).unwrap();
        assert!(n < data.len() / 2, "{n}");
        let mut back = [0u8; 1500];
        assert_eq!(decompress(&packed[..n], &mut back), Some(1500));
        assert_eq!(back, data);
        // Deterministic.
        let mut again = [0u8; 2048];
        assert_eq!(compress(&data, &mut again, &mut Lz::default()), Some(n));
        assert_eq!(packed[..n], again[..n]);
        // Malformed input never panics.
        assert_eq!(decompress(&[0x01, 0xFF, 0xFF], &mut back), None);
    }
}
