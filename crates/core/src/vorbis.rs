//! A bound check of an Ogg Vorbis file's setup header, before a decoder
//! sees it (issue #121).
//!
//! The PC and the Web decode `TSN2` Vorbis payloads with lewton, which
//! reserves memory for a codebook from the header's own counts before it
//! reads the values: 24-bit entries × 16-bit dimensions, up to 2^40 values.
//! A damaged or modded sound file of a few bytes then asked for 40 GB and
//! aborted the game (the `tsn2_decode` fuzz target, an OOM). [`check`]
//! walks the codebooks the way the Vorbis I spec (§3.2.1) lays them out and
//! refuses a file whose codebooks need more than [`MAX_ENTRIES`] entries,
//! [`MAX_VALUES`] lookup values or more value bits than the packet holds:
//! libvorbis' and aoTuV's books stay far below (a few thousand entries,
//! dimensions ≤ 8). `no_std`, no allocation: the setup packet is read in
//! place across its Ogg pages.

/// The most entries a codebook may have.
pub const MAX_ENTRIES: u32 = 1 << 16;
/// The most `entries × dimensions` of a codebook with a lookup table (the
/// decoder's table of floats).
pub const MAX_VALUES: u64 = 1 << 20;
/// The most Ogg pages the setup packet may span.
const MAX_SPANS: usize = 32;

/// Why [`check`] refused a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VorbisError {
    /// No Ogg pages, or the stream ends before the setup header.
    Truncated,
    /// The third packet is not a Vorbis setup header.
    NotSetup,
    /// A codebook's sync pattern or layout is wrong.
    BadCodebook,
    /// A codebook asks for more than the bounds above.
    TooLarge,
}

/// The setup packet (the stream's third) as byte spans of `ogg`, one per
/// page it crosses.
struct Spans<'a> {
    ogg: &'a [u8],
    spans: [(usize, usize); MAX_SPANS],
    n: usize,
}

fn setup_packet(ogg: &[u8]) -> Result<Spans<'_>, VorbisError> {
    let mut out = Spans {
        ogg,
        spans: [(0, 0); MAX_SPANS],
        n: 0,
    };
    let mut packet = 0u32;
    let mut serial = None;
    let mut at = 0usize;
    while at + 27 <= ogg.len() {
        if &ogg[at..at + 4] != b"OggS" {
            return Err(VorbisError::Truncated);
        }
        let page_serial =
            u32::from_le_bytes([ogg[at + 14], ogg[at + 15], ogg[at + 16], ogg[at + 17]]);
        let nsegs = ogg[at + 26] as usize;
        let table = at + 27;
        let data = table + nsegs;
        if data > ogg.len() {
            return Err(VorbisError::Truncated);
        }
        let len: usize = ogg[table..data].iter().map(|&l| l as usize).sum();
        if data + len > ogg.len() {
            return Err(VorbisError::Truncated);
        }
        // Only the first logical stream (what the decoder plays).
        if *serial.get_or_insert(page_serial) == page_serial {
            let mut seg_at = data;
            let mut start = data;
            for &lace in &ogg[table..data] {
                seg_at += lace as usize;
                if lace == 255 {
                    continue;
                }
                // A packet (or its last part) ends at `seg_at`.
                if packet == 2 {
                    push(&mut out, start, seg_at)?;
                    return Ok(out);
                }
                packet += 1;
                start = seg_at;
            }
            // A packet continuing on the next page.
            if packet == 2 && seg_at > start {
                push(&mut out, start, seg_at)?;
            }
        }
        at = data + len;
    }
    Err(VorbisError::Truncated)
}

fn push(s: &mut Spans<'_>, from: usize, to: usize) -> Result<(), VorbisError> {
    if s.n == MAX_SPANS {
        return Err(VorbisError::TooLarge);
    }
    s.spans[s.n] = (from, to);
    s.n += 1;
    Ok(())
}

/// LSB-first bits over the setup packet's spans (Vorbis bit packing).
struct Bits<'a> {
    spans: &'a Spans<'a>,
    span: usize,
    at: usize,
    bit: u32,
    left: u64,
}

impl<'a> Bits<'a> {
    fn new(spans: &'a Spans<'a>) -> Bits<'a> {
        let left = spans.spans[..spans.n]
            .iter()
            .map(|(a, b)| (b - a) as u64 * 8)
            .sum();
        Bits {
            spans,
            span: 0,
            at: spans.spans[0].0,
            bit: 0,
            left,
        }
    }

    fn read(&mut self, n: u32) -> Result<u32, VorbisError> {
        let mut v = 0u32;
        for i in 0..n {
            while self.span < self.spans.n && self.at >= self.spans.spans[self.span].1 {
                self.span += 1;
                self.at = self.spans.spans.get(self.span).map_or(0, |s| s.0);
            }
            if self.span >= self.spans.n {
                return Err(VorbisError::Truncated);
            }
            let b = (self.spans.ogg[self.at] >> self.bit) & 1;
            v |= (b as u32) << i;
            self.bit += 1;
            self.left -= 1;
            if self.bit == 8 {
                self.bit = 0;
                self.at += 1;
            }
        }
        Ok(v)
    }

    fn flag(&mut self) -> Result<bool, VorbisError> {
        Ok(self.read(1)? == 1)
    }
}

/// `ilog` of the Vorbis spec: bits needed for `v`.
fn ilog(v: u32) -> u32 {
    32 - v.leading_zeros()
}

/// `lookup1_values`: the greatest `r` with `r^dims <= entries`.
fn lookup1_values(entries: u32, dims: u32) -> u64 {
    if dims == 0 {
        return 0;
    }
    let mut r: u64 = 1;
    loop {
        let mut p: u64 = 1;
        for _ in 0..dims {
            p = p.saturating_mul(r + 1);
            if p > entries as u64 {
                return r;
            }
        }
        r += 1;
    }
}

/// Check the Vorbis setup header of an Ogg file against the bounds above
/// (see the module docs). `Ok` says nothing about the rest of the stream:
/// the decoder still validates it.
pub fn check(ogg: &[u8]) -> Result<(), VorbisError> {
    let spans = setup_packet(ogg)?;
    if spans.n == 0 {
        return Err(VorbisError::NotSetup);
    }
    let mut b = Bits::new(&spans);
    if b.read(8)? != 5 {
        return Err(VorbisError::NotSetup);
    }
    for &c in b"vorbis" {
        if b.read(8)? != c as u32 {
            return Err(VorbisError::NotSetup);
        }
    }
    let books = b.read(8)? + 1;
    for _ in 0..books {
        if b.read(24)? != 0x56_4342 {
            return Err(VorbisError::BadCodebook);
        }
        let dims = b.read(16)?;
        let entries = b.read(24)?;
        if entries > MAX_ENTRIES {
            return Err(VorbisError::TooLarge);
        }
        if b.flag()? {
            // Ordered: runs of entries per length.
            let mut length = b.read(5)? + 1;
            let mut at = 0u32;
            while at < entries {
                let n = b.read(ilog(entries - at))?;
                at = at.checked_add(n).ok_or(VorbisError::BadCodebook)?;
                if at > entries || length > 32 {
                    return Err(VorbisError::BadCodebook);
                }
                length += 1;
            }
        } else {
            let sparse = b.flag()?;
            for _ in 0..entries {
                if !sparse || b.flag()? {
                    b.read(5)?;
                }
            }
        }
        let lookup = b.read(4)?;
        match lookup {
            0 => {}
            1 | 2 => {
                if entries as u64 * dims as u64 > MAX_VALUES {
                    return Err(VorbisError::TooLarge);
                }
                b.read(32)?;
                b.read(32)?;
                let value_bits = b.read(4)? + 1;
                b.read(1)?;
                let values = if lookup == 1 {
                    lookup1_values(entries, dims)
                } else {
                    entries as u64 * dims as u64
                };
                // The values are in the packet: a count it can't hold is a
                // lie the decoder would reserve memory for first.
                if values * value_bits as u64 > b.left {
                    return Err(VorbisError::Truncated);
                }
                for _ in 0..values {
                    b.read(value_bits)?;
                }
            }
            _ => return Err(VorbisError::BadCodebook),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One Ogg page holding `packets` (each under 255 × 255 bytes).
    pub(crate) fn page(serial: u32, packets: &[&[u8]]) -> std::vec::Vec<u8> {
        let mut table = std::vec::Vec::new();
        let mut data = std::vec::Vec::new();
        for p in packets {
            let mut n = p.len();
            while n >= 255 {
                table.push(255u8);
                n -= 255;
            }
            table.push(n as u8);
            data.extend_from_slice(p);
        }
        let mut out = b"OggS".to_vec();
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&serial.to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        out.push(table.len() as u8);
        out.extend_from_slice(&table);
        out.extend_from_slice(&data);
        out
    }

    /// LSB-first bit writer.
    #[derive(Default)]
    struct W {
        out: std::vec::Vec<u8>,
        bit: u32,
    }

    impl W {
        fn put(&mut self, v: u64, n: u32) {
            for i in 0..n {
                if self.bit == 0 {
                    self.out.push(0);
                }
                let last = self.out.last_mut().unwrap();
                *last |= (((v >> i) & 1) as u8) << self.bit;
                self.bit = (self.bit + 1) % 8;
            }
        }
    }

    /// A setup packet with one codebook.
    fn setup(dims: u32, entries: u32, lookup: u32, values: u32) -> std::vec::Vec<u8> {
        let mut w = W::default();
        w.put(5, 8);
        for &c in b"vorbis" {
            w.put(c as u64, 8);
        }
        w.put(0, 8); // one codebook
        w.put(0x56_4342, 24);
        w.put(dims as u64, 16);
        w.put(entries as u64, 24);
        w.put(0, 1); // not ordered
        w.put(0, 1); // not sparse
        for _ in 0..entries.min(4096) {
            w.put(4, 5);
        }
        w.put(lookup as u64, 4);
        if lookup != 0 {
            w.put(0, 32);
            w.put(0, 32);
            w.put(0, 4); // 1 bit per value
            w.put(0, 1);
            for _ in 0..values {
                w.put(1, 1);
            }
        }
        w.out
    }

    fn file(setup: &[u8]) -> std::vec::Vec<u8> {
        let mut f = page(7, &[b"\x01vorbis ident"]);
        f.extend(page(7, &[b"\x03vorbis comments", setup]));
        f
    }

    #[test]
    fn plausible_codebooks_pass_and_huge_ones_are_refused() {
        assert_eq!(check(&file(&setup(2, 16, 2, 32))), Ok(()));
        assert_eq!(check(&file(&setup(2, 16, 1, 4))), Ok(()));
        assert_eq!(check(&file(&setup(1, 8, 0, 0))), Ok(()));
        // The fuzzer's case: a lookup table of 2^40 values in a few bytes.
        let mut w = W::default();
        w.put(5, 8);
        for &c in b"vorbis" {
            w.put(c as u64, 8);
        }
        w.put(0, 8);
        w.put(0x56_4342, 24);
        w.put(0xFFFF, 16);
        w.put(64, 24);
        w.put(1, 1); // ordered: one run of 64 entries
        w.put(0, 5);
        w.put(64, 7);
        w.put(2, 4);
        w.put(0, 64);
        w.put(0, 5);
        assert_eq!(check(&file(&w.out)), Err(VorbisError::TooLarge));
        // More values than bits left in the packet.
        let mut short = setup(4, 64, 2, 256);
        short.truncate(short.len() - 8);
        assert_eq!(check(&file(&short)), Err(VorbisError::Truncated));
        // Too many entries.
        assert_eq!(
            check(&file(&setup(1, MAX_ENTRIES + 1, 0, 0))),
            Err(VorbisError::TooLarge)
        );
        // Not Ogg, not a setup header, truncated.
        assert_eq!(check(b""), Err(VorbisError::Truncated));
        assert_eq!(check(b"RIFF....WAVE"), Err(VorbisError::Truncated));
        assert_eq!(check(&file(b"\x04vorbis")), Err(VorbisError::NotSetup));
    }

    #[test]
    fn a_setup_packet_across_pages_reads_in_place() {
        let s = setup(2, 1000, 2, 2000);
        let (a, rest) = s.split_at(255 * 2);
        // The setup packet's first part ends the second page with 255
        // laces; the third page holds the rest.
        let mut f = page(7, &[b"\x01vorbis ident"]);
        let mut second = page(7, &[b"\x03vorbis comments", a]);
        // `page` ended `a` with a 0 lace (a packet end): drop it so the
        // packet continues on the next page.
        let lace_at = 27 + 1 + 2;
        assert_eq!(second[lace_at], 0);
        second.remove(lace_at);
        second[26] -= 1;
        f.extend(second);
        // Another stream's page in between is skipped.
        f.extend(page(9, &[b"another stream"]));
        f.extend(page(7, &[rest]));
        assert_eq!(check(&f), Ok(()));
        assert!(lookup1_values(16, 2) == 4 && lookup1_values(15, 2) == 3);
    }
}
