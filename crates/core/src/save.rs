//! Save-data framing shared by the console backends.
//!
//! Raw save media (N64 EEPROM, a 3DS file) hold `header ‖ data`, where the
//! 8-byte header is `"TS" u16 len u32 fnv1a(data)` (little-endian). A torn
//! write or a blank/foreign medium fails the check and reads back as "no
//! save" instead of garbage — games only ever see their own last good data.

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
}
