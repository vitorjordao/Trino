//! Save-data framing shared by the console backends.
//!
//! **Files** (a 3DS save, tests) hold `header ‖ data`, where the 8-byte
//! header is `"TS" u16 len u32 fnv1a(data)` (little-endian). A torn write or
//! a blank/foreign medium fails the check and reads back as "no save"
//! instead of garbage — games only ever see their own last good data.
//!
//! **Raw media** (the N64 EEPROM, [`Medium`]) hold two **slots**, A at 0
//! and B at half the medium. Each slot is a [`SLOT_HEADER`]-byte header
//! `"T2" seq:u16 stored:u16 raw:u16 fnv1a:u32 flags:u8 0 0 0` (the
//! checksum over the header's fields and the stored bytes since issue #68,
//! flag 4; over the stored bytes alone before) and
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

pub use crate::platform::SaveStatus;

/// Bytes of framing in front of the data.
pub const HEADER: usize = 8;
const MAGIC: [u8; 2] = *b"TS";

/// FNV-1a 32 over bytes (same function as asset handles).
pub fn checksum(data: &[u8]) -> u32 {
    fnv_from(0x811c9dc5, data)
}

/// FNV-1a 32 continued from `hash`.
fn fnv_from(mut hash: u32, data: &[u8]) -> u32 {
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
/// The checksum covers the header's fields too (sequence number, lengths,
/// flags; issue #68): a flipped bit in a sequence number can't make an
/// older save look newer, nor a torn header pass. Every write since sets
/// it; a save without it (an older build's) checks its body only.
const FLAG_SUM_HEADER: u8 = 4;

/// The checksum a slot header `h` must carry for `body`.
fn slot_sum(h: &[u8; SLOT_HEADER], body: &[u8]) -> u32 {
    if h[12] & FLAG_SUM_HEADER != 0 {
        fnv_from(fnv_from(checksum(&h[2..8]), &h[12..16]), body)
    } else {
        checksum(body)
    }
}

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
    h[12] = flags | FLAG_SUM_HEADER;
    let sum = slot_sum(&h, stored);
    h[8..12].copy_from_slice(&sum.to_le_bytes());
    h
}

/// Is the slot at `offset` valid? `scratch` must hold the medium's size.
fn probe(m: &mut dyn Medium, offset: usize, scratch: &mut [u8]) -> Option<Found> {
    probe_slot(m, offset, scratch).ok()
}

/// What a slot of a medium holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Probe {
    /// No save header at all (blank or foreign bytes).
    Blank,
    /// A save header whose body fails its check: a torn write or bit rot.
    /// The sequence number its header claims, when it has one.
    Damaged(Option<u16>),
}

/// What the slot at `offset` of a raw medium holds (`cargo xtask save
/// inspect`): `Ok` a valid save, `Err(None)` blank or foreign bytes,
/// `Err(Some(seq))` a save header whose body fails its check (`seq` the
/// sequence number it claims, `None` for a legacy frame).
pub fn slot_state(
    m: &mut dyn Medium,
    offset: usize,
    scratch: &mut [u8],
) -> Result<Found, Option<Option<u16>>> {
    probe_slot(m, offset, scratch).map_err(|p| match p {
        Probe::Blank => None,
        Probe::Damaged(seq) => Some(seq),
    })
}

fn probe_slot(m: &mut dyn Medium, offset: usize, scratch: &mut [u8]) -> Result<Found, Probe> {
    let total = m.size();
    if offset + SLOT_HEADER > total {
        return Err(Probe::Blank);
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
            return Err(Probe::Damaged(Some(seq)));
        }
        m.read(offset + SLOT_HEADER, &mut scratch[..stored]);
        if slot_sum(&h, &scratch[..stored]) != sum {
            return Err(Probe::Damaged(Some(seq)));
        }
        return Ok(Found {
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
    if offset == 0 && h[0..2] == MAGIC {
        let mut h8 = [0u8; HEADER];
        h8.copy_from_slice(&h[..HEADER]);
        let Some(len) = data_len(&h8).filter(|&l| l <= total - HEADER && l <= scratch.len()) else {
            return Err(Probe::Damaged(None));
        };
        m.read(HEADER, &mut scratch[..len]);
        if verify(&h8, &scratch[..len]) {
            return Ok(Found {
                offset: 0,
                seq: 0,
                stored: len,
                raw: len,
                compressed: false,
                span: HEADER + len,
                legacy: true,
            });
        }
        return Err(Probe::Damaged(None));
    }
    Err(Probe::Blank)
}

/// `a` is newer than `b` (wrapping sequence numbers).
fn newer(a: u16, b: u16) -> bool {
    (a.wrapping_sub(b) as i16) > 0
}

/// The newest valid save on `m`, if any. `scratch` must hold `m.size()`
/// bytes.
pub fn find_newest(m: &mut dyn Medium, scratch: &mut [u8]) -> Option<Found> {
    find_newest_status(m, scratch).0
}

/// [`find_newest`], and what the read found ([`SaveStatus`]): a slot with a
/// save header whose body fails its check means a newer save may be lost.
pub fn find_newest_status(m: &mut dyn Medium, scratch: &mut [u8]) -> (Option<Found>, SaveStatus) {
    let (found, damaged) = probe_both(m, scratch);
    (found, status_of(found.map(|f| (f.seq, f.legacy)), damaged))
}

/// Both slots, read once: the newest valid save and each slot's damage
/// (`Some(seq)` per [`status_of`]).
fn probe_both(m: &mut dyn Medium, scratch: &mut [u8]) -> (Option<Found>, [Option<Option<u16>>; 2]) {
    let half = m.size() / 2;
    let pa = probe_slot(m, 0, scratch);
    let pb = if half >= SLOT_HEADER {
        probe_slot(m, half, scratch)
    } else {
        Err(Probe::Blank)
    };
    // A wide or legacy save at 0 covers B: B's bytes are its body.
    let covers_b = matches!(pa, Ok(f) if f.span > half);
    let pb = if covers_b { Err(Probe::Blank) } else { pb };
    let found = pick_newest(pa.ok(), pb.ok());
    let damaged = [pa, pb].map(|p| match p {
        Err(Probe::Damaged(seq)) => Some(seq),
        _ => None,
    });
    (found, damaged)
}

/// The [`SaveStatus`] of a read that found `found` (its sequence number and
/// whether it is a legacy frame) beside slots that failed their check
/// (`Some(seq)` each, the seq `None` when the header was unreadable).
///
/// A damaged slot claiming a newer save than the one found means the read
/// fell back to an older save ([`SaveStatus::RecoveredFromBackup`]). One
/// claiming an older save is the slot a torn write was overwriting: what
/// was found is still the last save that finished, so the read is `Ok`.
pub fn status_of(found: Option<(u16, bool)>, damaged: [Option<Option<u16>>; 2]) -> SaveStatus {
    let any_damaged = damaged.iter().any(Option::is_some);
    match found {
        None if any_damaged => SaveStatus::Corrupted,
        None => SaveStatus::Empty,
        Some((seq, legacy)) => {
            // A slot whose header can't be read at all (no magic) claims
            // nothing: it can't be told newer than the intact one.
            let lost_newer = damaged
                .iter()
                .flatten()
                .any(|d| legacy || matches!(d, Some(d) if newer(*d, seq)));
            if lost_newer {
                SaveStatus::RecoveredFromBackup
            } else {
                SaveStatus::Ok
            }
        }
    }
}

fn pick_newest(a: Option<Found>, b: Option<Found>) -> Option<Found> {
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
    read_newest_status(m, out, scratch).0
}

/// [`read_newest`] and its [`SaveStatus`].
pub fn read_newest_status(
    m: &mut dyn Medium,
    out: &mut [u8],
    scratch: &mut [u8],
) -> (usize, SaveStatus) {
    let (found, status) = find_newest_status(m, scratch);
    let Some(f) = found else {
        return (0, status);
    };
    let n = read_found(m, f, out, scratch);
    if n == 0 && f.raw > 0 {
        // A compressed body that passed its checksum yet doesn't decode
        // (or doesn't fit `out`): nothing usable.
        return (0, SaveStatus::Corrupted);
    }
    (n, status)
}

fn read_found(m: &mut dyn Medium, f: Found, out: &mut [u8], scratch: &mut [u8]) -> usize {
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
    // One read of both slots (each probe reads a body over the joybus).
    let (newest, damaged) = probe_both(m, scratch);
    // A damaged slot may claim a later sequence number than every intact
    // one (a torn header, bit rot): number past it, so the new save is the
    // newest and the next read reports it intact.
    let mut seq = newest.map_or(1, |f| f.seq.wrapping_add(1));
    for d in damaged.into_iter().flatten().flatten() {
        if !newer(seq, d) {
            seq = d.wrapping_add(1);
        }
    }
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

// ------------------------------------------------------------ fault tests

/// Apply `fault` to a raw medium (the N64 EEPROM; see [`SaveFault`]).
/// `scratch` must hold `m.size()` bytes. `false` when there is nothing to
/// damage (no valid save).
pub fn medium_fault(m: &mut dyn Medium, fault: SaveFault, scratch: &mut [u8]) -> bool {
    let half = m.size() / 2;
    let a = probe(m, 0, scratch);
    let b = probe(m, half, scratch)
        .filter(|_| half >= SLOT_HEADER && !matches!(a, Some(f) if f.span > half));
    let newest = pick_newest(a, b);
    let targets: [Option<Found>; 2] = match fault {
        SaveFault::FlipAll => [a, b],
        _ => [newest, None],
    };
    let mut hit = false;
    for f in targets.into_iter().flatten() {
        let body = f.offset + if f.legacy { HEADER } else { SLOT_HEADER };
        if f.stored == 0 {
            // An empty body has nothing to flip: break the header instead.
            let mut h = [0u8; 1];
            m.read(f.offset + 2, &mut h);
            m.write(f.offset + 2, &[h[0] ^ 1]);
            hit = true;
            continue;
        }
        match fault {
            SaveFault::TearNewest => {
                let keep = f.stored / 2;
                let zeros = [0u8; 64];
                let mut at = body + keep;
                let end = body + f.stored;
                while at < end {
                    let n = (end - at).min(zeros.len());
                    // A torn EEPROM block reads back as what the cell held
                    // before; zeros differ from any LZ or text body.
                    m.write(at, &zeros[..n]);
                    at += n;
                }
                // A body of zeros may still match: flip its last byte too.
                let mut last = [0u8; 1];
                m.read(end - 1, &mut last);
                m.write(end - 1, &[last[0] ^ 0x5A]);
            }
            _ => {
                let at = body + f.stored / 2;
                let mut x = [0u8; 1];
                m.read(at, &mut x);
                m.write(at, &[x[0] ^ 0x10]);
            }
        }
        hit = true;
    }
    hit
}

// ------------------------------------------------------------- slot files

/// The **slot files** of the PC, the Web and the 3DS (issue #68): two
/// whole-file slots, A and B, each the raw medium's slot header
/// ([`SLOT_HEADER`] bytes: `"T2" seq stored raw fnv1a flags`, the checksum
/// over the header's fields and the data) and the data, never compressed. A save goes to the slot that does not hold the newest
/// intact one, so a damaged file (a crash before the rename, bit rot, a
/// player's edit, two machines' files after a cloud sync) leaves the
/// previous save readable. A store writes one slot atomically where it can
/// (a temp file and a rename, one `localStorage.setItem`).
pub trait SlotStore {
    /// Read slot `slot` (0 = A, 1 = B) into `buf`: `Ok(None)` when it does
    /// not exist, else its full length (only `buf.len()` bytes are copied).
    fn read_slot(&mut self, slot: u8, buf: &mut [u8]) -> Result<Option<usize>, SaveError>;
    /// Replace slot `slot` with `bytes`.
    fn write_slot(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError>;
    /// The save from before slot files (a raw file on the PC and the Web, a
    /// `"TS"` frame on the 3DS), read when no slot is intact: `Ok(None)`
    /// when there is none (the default).
    fn read_legacy(&mut self, _buf: &mut [u8]) -> Result<Option<usize>, SaveError> {
        Ok(None)
    }
}

use crate::platform::{SaveError, SaveFault};

/// Bytes of a slot file holding `len` bytes of data.
pub const fn slot_file_len(len: usize) -> usize {
    SLOT_HEADER + len
}

/// Write a slot file of `data` with sequence number `seq` into `out`; its
/// length, `None` when `out` is short or `data` longer than `u16::MAX`.
pub fn encode_slot(seq: u16, data: &[u8], out: &mut [u8]) -> Option<usize> {
    if data.len() > u16::MAX as usize {
        return None;
    }
    let len = slot_file_len(data.len());
    let out = out.get_mut(..len)?;
    out[..SLOT_HEADER].copy_from_slice(&slot_header(seq, data, data.len(), 0));
    out[SLOT_HEADER..].copy_from_slice(data);
    Some(len)
}

/// Why a slot file is not a save.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotError {
    /// Shorter than a header, or no `"T2"` magic: not a slot file at all.
    Foreign,
    /// A header whose data is missing, longer, or fails its checksum (the
    /// sequence number it claims, for the recovery status).
    Damaged(u16),
}

/// A valid slot file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SlotView<'a> {
    pub seq: u16,
    pub data: &'a [u8],
}

/// Check a slot file's bytes.
pub fn parse_slot(bytes: &[u8]) -> Result<SlotView<'_>, SlotError> {
    if bytes.len() < SLOT_HEADER || bytes[0..2] != MAGIC2 {
        return Err(SlotError::Foreign);
    }
    let seq = u16::from_le_bytes([bytes[2], bytes[3]]);
    let stored = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let raw = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let sum = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let data = &bytes[SLOT_HEADER..];
    let mut h = [0u8; SLOT_HEADER];
    h.copy_from_slice(&bytes[..SLOT_HEADER]);
    if stored != data.len() || raw != stored || h[12] != FLAG_SUM_HEADER || h[13..16] != [0, 0, 0] {
        return Err(SlotError::Damaged(seq));
    }
    if slot_sum(&h, data) != sum {
        return Err(SlotError::Damaged(seq));
    }
    Ok(SlotView { seq, data })
}

/// What [`read_slot_files`] found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SlotRead {
    /// The data's length (copied to the front of `out`, cut at its end).
    pub len: usize,
    pub status: SaveStatus,
    /// The slot it came from (`None`: no save, or the legacy file).
    pub slot: Option<u8>,
    /// The sequence number it carries (legacy: 0).
    pub seq: u16,
    /// The slot the next save goes to, and its sequence number.
    pub next_slot: u8,
    pub next_seq: u16,
}

/// What one slot file holds: `Ok(seq)` an intact save, `Err(None)`
/// nothing (missing or empty), `Err(Some(seq))` damage claiming `seq`
/// (`None`: no readable header).
type FileProbe = Result<u16, Option<Option<u16>>>;

/// Check one slot file of `store` (`buf` holds the whole file).
fn probe_file(store: &mut dyn SlotStore, slot: u8, buf: &mut [u8]) -> Result<FileProbe, SaveError> {
    let Some(len) = store.read_slot(slot, buf)? else {
        return Ok(Err(None));
    };
    if len > buf.len() {
        // Longer than any save this game writes: not one of its slots.
        return Ok(Err(Some(None)));
    }
    Ok(match parse_slot(&buf[..len]) {
        Ok(v) => Ok(v.seq),
        // An empty or foreign file is not a save at all.
        Err(SlotError::Foreign) if len == 0 => Err(None),
        Err(SlotError::Foreign) => Err(Some(None)),
        Err(SlotError::Damaged(seq)) => Err(Some(Some(seq))),
    })
}

/// Read the newest intact save of `store` into `out`. `scratch` must hold
/// the largest slot file the game writes ([`slot_file_len`] of its
/// capacity); a slot longer than that reads as damaged. Ties (two machines'
/// slots with the same sequence number after a cloud sync) go to slot A.
pub fn read_slot_files(
    store: &mut dyn SlotStore,
    out: &mut [u8],
    scratch: &mut [u8],
) -> Result<SlotRead, SaveError> {
    let a = probe_file(store, 0, scratch)?;
    let b = probe_file(store, 1, scratch)?;
    let pick = match (a, b) {
        (Ok(sa), Ok(sb)) => Some(if newer(sb, sa) { 1 } else { 0 }),
        (Ok(_), _) => Some(0),
        (_, Ok(_)) => Some(1),
        _ => None,
    };
    let damaged = [a.err().flatten(), b.err().flatten()];
    let Some(slot) = pick else {
        // No intact slot: the save from before slot files, if any.
        let mut legacy = store.read_legacy(scratch)?;
        if legacy.is_some_and(|l| l > scratch.len()) {
            legacy = None;
        }
        let next_slot = 0;
        let (len, status) = match legacy {
            Some(l) => {
                let k = l.min(out.len());
                out[..k].copy_from_slice(&scratch[..k]);
                (l, status_of(Some((0, true)), damaged))
            }
            None => (0, status_of(None, damaged)),
        };
        let next_seq = damaged
            .iter()
            .flatten()
            .flatten()
            .fold(0u16, |m, &s| if newer(s, m) { s } else { m })
            .wrapping_add(1);
        return Ok(SlotRead {
            len,
            status,
            slot: None,
            seq: 0,
            next_slot,
            next_seq,
        });
    };
    // Read the winner again (the scratch now holds the other one).
    let n = store
        .read_slot(slot, scratch)?
        .unwrap_or(0)
        .min(scratch.len());
    let v = parse_slot(&scratch[..n]).map_err(|_| SaveError::Io)?;
    let k = v.data.len().min(out.len());
    out[..k].copy_from_slice(&v.data[..k]);
    Ok(SlotRead {
        len: v.data.len(),
        status: status_of(Some((v.seq, false)), damaged),
        slot: Some(slot),
        seq: v.seq,
        next_slot: 1 - slot,
        next_seq: v.seq.wrapping_add(1),
    })
}

/// Write `data` as the next save of `store`: to the slot that does not hold
/// the newest intact save, one sequence number past it. `scratch` as for
/// [`read_slot_files`]; `file` must hold [`slot_file_len`]`(data.len())`.
pub fn write_slot_files(
    store: &mut dyn SlotStore,
    data: &[u8],
    scratch: &mut [u8],
    file: &mut [u8],
) -> Result<SlotRead, SaveError> {
    let mut none: [u8; 0] = [];
    let r = read_slot_files(store, &mut none, scratch)?;
    let n = encode_slot(r.next_seq, data, file).ok_or(SaveError::TooLarge {
        len: data.len(),
        capacity: file.len().saturating_sub(SLOT_HEADER),
    })?;
    store.write_slot(r.next_slot, &file[..n])?;
    if r.slot.is_none() && r.status == SaveStatus::Corrupted {
        // Nothing intact to keep: repair the other slot too, or the next
        // read would still find its damage and report a lost save.
        store.write_slot(1 - r.next_slot, &file[..n])?;
    }
    Ok(SlotRead {
        len: data.len(),
        status: SaveStatus::Ok,
        slot: Some(r.next_slot),
        seq: r.next_seq,
        next_slot: 1 - r.next_slot,
        next_seq: r.next_seq.wrapping_add(1),
    })
}

/// Apply `fault` to `store`'s slot files ([`SaveFault`]); `false` when
/// there is no intact slot to damage. `scratch` as for [`read_slot_files`].
pub fn slot_files_fault(
    store: &mut dyn SlotStore,
    fault: SaveFault,
    scratch: &mut [u8],
) -> Result<bool, SaveError> {
    let mut none: [u8; 0] = [];
    let r = read_slot_files(store, &mut none, scratch)?;
    let slots: &[u8] = match (fault, r.slot) {
        (SaveFault::FlipAll, _) => &[0, 1],
        (_, Some(0)) => &[0],
        (_, Some(_)) => &[1],
        (_, None) => &[],
    };
    let mut hit = false;
    for &slot in slots {
        let Some(n) = store.read_slot(slot, scratch)? else {
            continue;
        };
        let n = n.min(scratch.len());
        if parse_slot(&scratch[..n]).is_err() {
            continue;
        }
        let bytes = &mut scratch[..n];
        let body = n - SLOT_HEADER;
        match fault {
            SaveFault::TearNewest => {
                // The file as a torn in-place write leaves it: the header
                // and the first half of the data.
                let keep = SLOT_HEADER + body / 2;
                store.write_slot(slot, &bytes[..keep])?;
            }
            _ => {
                let at = if body > 0 { SLOT_HEADER + body / 2 } else { 2 };
                bytes[at] ^= 0x10;
                store.write_slot(slot, bytes)?;
            }
        }
        hit = true;
    }
    Ok(hit)
}

// --------------------------------------------------------- versioned saves

/// The tag in front of a versioned save ([`write_versioned`]): `"SV"` and a
/// little-endian `u16` version.
pub const VERSION_TAG: [u8; 2] = *b"SV";
/// Bytes [`write_versioned`] adds in front of the data.
pub const VERSION_HEADER: usize = 4;

/// `data` behind the version tag, in `out`; its length.
pub fn write_versioned(version: u16, data: &[u8], out: &mut [u8]) -> Option<usize> {
    let len = VERSION_HEADER + data.len();
    let out = out.get_mut(..len)?;
    out[..2].copy_from_slice(&VERSION_TAG);
    out[2..4].copy_from_slice(&version.to_le_bytes());
    out[VERSION_HEADER..].copy_from_slice(data);
    Some(len)
}

/// The version of a save and its data: `(0, bytes)` for a save written
/// before the game used versions (no tag).
pub fn split_version(bytes: &[u8]) -> (u16, &[u8]) {
    match bytes {
        [b'S', b'V', lo, hi, rest @ ..] => (u16::from_le_bytes([*lo, *hi]), rest),
        _ => (0, bytes),
    }
}

/// A migration: `(from, buf, len)` rewrites `buf[..len]` of version `from`
/// as the current version and returns its length (`None`: give up).
pub type Migrate<'a> = dyn FnMut(u16, &mut [u8], usize) -> Option<usize> + 'a;

/// What [`load_versioned`] read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Loaded {
    /// Bytes of current-version data at the front of the buffer (0: none —
    /// start from defaults).
    pub len: usize,
    /// The version the medium held (equal to the current one when no
    /// migration ran).
    pub from: u16,
    /// What the platform's read found ([`Platform::save_status`]).
    pub status: SaveStatus,
    /// The save came from an older version and was migrated.
    pub migrated: bool,
}

/// Read a versioned save (issue #68): the newest intact save, its version
/// tag checked, older versions handed to `migrate(from, buf, len)` (which
/// rewrites `buf[..len]` in place as the current version and returns its
/// new length, `None` when it can't: the game starts from defaults). A save
/// from a newer version than `current` loads nothing (an older build must
/// not overwrite it unknowingly: check `from` before saving).
/// [`Game::migrate_save`](crate::Game::migrate_save) is the hook games give.
pub fn load_versioned(
    platform: &mut dyn crate::Platform,
    buf: &mut [u8],
    current: u16,
    migrate: &mut Migrate<'_>,
) -> Result<Loaded, SaveError> {
    let n = platform.save_read(buf)?.min(buf.len());
    let status = platform.save_status();
    if n == 0 {
        return Ok(Loaded {
            len: 0,
            from: current,
            status,
            migrated: false,
        });
    }
    let (from, data_len) = {
        let (v, d) = split_version(&buf[..n]);
        (v, d.len())
    };
    // Move the data to the front of the buffer.
    let skip = n - data_len;
    buf.copy_within(skip..n, 0);
    if from == current {
        return Ok(Loaded {
            len: data_len,
            from,
            status,
            migrated: false,
        });
    }
    if from > current {
        return Ok(Loaded {
            len: 0,
            from,
            status,
            migrated: false,
        });
    }
    let len = migrate(from, buf, data_len)
        .filter(|&l| l <= buf.len())
        .unwrap_or(0);
    Ok(Loaded {
        len,
        from,
        status,
        migrated: len > 0,
    })
}

/// [`load_versioned`] with the game's own
/// [`Game::migrate_save`](crate::Game::migrate_save).
pub fn load_game<G: crate::Game + ?Sized>(
    game: &mut G,
    platform: &mut dyn crate::Platform,
    buf: &mut [u8],
    current: u16,
) -> Result<Loaded, SaveError> {
    load_versioned(platform, buf, current, &mut |from, data, len| {
        game.migrate_save(from, data, len)
    })
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
