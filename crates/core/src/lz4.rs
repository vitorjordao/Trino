//! LZ4 block decompression (issue #56): the codec of asset bundles on the
//! 3DS, the PC and the Web (`codec = "lz4"`, [`crate::bundle`]).
//!
//! This is the LZ4 *block* format, the one `LZ4_compress_default` and
//! `LZ4_compress_HC` write and libdragon's compression level 1 holds inside
//! its asset header. A block is a run of sequences: a token (literal length
//! in the high nibble, match length − 4 in the low one; 15 means "more
//! bytes follow, each adding up to 255"), the literals, then a 16-bit
//! little-endian back offset and the match. The last sequence has literals
//! only. The bake's encoder is `trino_asset_pipeline::lz4`.
//!
//! The decoder is safe on any input: a corrupt or truncated block is an
//! error, never a read or write out of bounds. No allocation, no
//! dependency, so the same code runs on every target.

/// Why a block did not decode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lz4Error {
    /// The block ended inside a length, literal run or offset.
    Truncated,
    /// A match points before the start of the output (or offset 0).
    BadOffset,
    /// The block decodes to more bytes than the output holds.
    OutputFull,
    /// The block ended before the output was full ([`decompress_exact`]).
    OutputShort,
}

impl Lz4Error {
    pub const fn label(self) -> &'static str {
        match self {
            Lz4Error::Truncated => "truncated LZ4 block",
            Lz4Error::BadOffset => "LZ4 match before the start of the output",
            Lz4Error::OutputFull => "LZ4 block larger than its declared size",
            Lz4Error::OutputShort => "LZ4 block smaller than its declared size",
        }
    }
}

/// Decode the LZ4 block `src` into `dst`; returns the bytes written.
pub fn decompress(src: &[u8], dst: &mut [u8]) -> Result<usize, Lz4Error> {
    let mut ip = 0usize;
    let mut op = 0usize;
    if src.is_empty() {
        return Ok(0);
    }
    loop {
        let token = *src.get(ip).ok_or(Lz4Error::Truncated)?;
        ip += 1;
        let literals = read_length(src, &mut ip, (token >> 4) as usize)?;
        let lit_end = ip.checked_add(literals).ok_or(Lz4Error::Truncated)?;
        if lit_end > src.len() {
            return Err(Lz4Error::Truncated);
        }
        let out_end = op.checked_add(literals).ok_or(Lz4Error::OutputFull)?;
        if out_end > dst.len() {
            return Err(Lz4Error::OutputFull);
        }
        dst[op..out_end].copy_from_slice(&src[ip..lit_end]);
        ip = lit_end;
        op = out_end;
        if ip == src.len() {
            return Ok(op);
        }
        if ip + 2 > src.len() {
            return Err(Lz4Error::Truncated);
        }
        let offset = u16::from_le_bytes([src[ip], src[ip + 1]]) as usize;
        ip += 2;
        if offset == 0 || offset > op {
            return Err(Lz4Error::BadOffset);
        }
        let len = read_length(src, &mut ip, (token & 15) as usize)? + 4;
        let end = op.checked_add(len).ok_or(Lz4Error::OutputFull)?;
        if end > dst.len() {
            return Err(Lz4Error::OutputFull);
        }
        let from = op - offset;
        if offset >= len {
            dst.copy_within(from..from + len, op);
        } else {
            // Overlapping: the match repeats the bytes it is writing.
            for i in 0..len {
                dst[op + i] = dst[from + i];
            }
        }
        op = end;
    }
}

/// [`decompress`] that must fill `dst` exactly (an entry's declared size).
pub fn decompress_exact(src: &[u8], dst: &mut [u8]) -> Result<(), Lz4Error> {
    match decompress(src, dst)? {
        n if n == dst.len() => Ok(()),
        _ => Err(Lz4Error::OutputShort),
    }
}

/// A token nibble plus its extension bytes.
fn read_length(src: &[u8], ip: &mut usize, nibble: usize) -> Result<usize, Lz4Error> {
    let mut len = nibble;
    if nibble == 15 {
        loop {
            let b = *src.get(*ip).ok_or(Lz4Error::Truncated)?;
            *ip += 1;
            len = len.checked_add(b as usize).ok_or(Lz4Error::Truncated)?;
            if b != 255 {
                break;
            }
        }
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_only() {
        // token 0x50: five literals, no match (the last sequence).
        let block = [0x50, b'h', b'e', b'l', b'l', b'o'];
        let mut out = [0u8; 5];
        assert_eq!(decompress(&block, &mut out), Ok(5));
        assert_eq!(&out, b"hello");
    }

    #[test]
    fn overlapping_match_repeats_a_run() {
        // "a" then a match of 9 at offset 1, then 1 literal "b".
        let block = [0x15, b'a', 1, 0, 0x10, b'b'];
        let mut out = [0u8; 11];
        decompress_exact(&block, &mut out).unwrap();
        assert_eq!(&out, b"aaaaaaaaaab");
    }

    #[test]
    fn long_lengths_use_extension_bytes() {
        // 15 + 5 = 20 literals.
        let mut block = vec![0xF0, 5];
        block.extend(core::iter::repeat_n(b'x', 20));
        let mut out = [0u8; 20];
        decompress_exact(&block, &mut out).unwrap();
        assert!(out.iter().all(|&b| b == b'x'));
    }

    #[test]
    fn corrupt_blocks_are_errors_not_panics() {
        let mut out = [0u8; 16];
        assert_eq!(
            decompress(&[0x50, b'a'], &mut out),
            Err(Lz4Error::Truncated)
        );
        assert_eq!(
            decompress(&[0x14, b'a', 2, 0, 0x00], &mut out),
            Err(Lz4Error::BadOffset)
        );
        assert_eq!(
            decompress(&[0x10, b'a', 0, 0, 0x00], &mut out),
            Err(Lz4Error::BadOffset)
        );
        assert_eq!(
            decompress(&[0x1F, b'a', 1, 0, 200, 0x00], &mut out),
            Err(Lz4Error::OutputFull)
        );
        assert_eq!(
            decompress_exact(&[0x10, b'a'], &mut out),
            Err(Lz4Error::OutputShort)
        );
        // Every prefix of a valid block fails cleanly or decodes less.
        let block = [0x15, b'a', 1, 0, 0x10, b'b'];
        for n in 0..block.len() {
            let mut out = [0u8; 11];
            assert!(decompress_exact(&block[..n], &mut out).is_err());
        }
    }
}
