//! The Web's asset pack (`assets/<name>.pak`): every baked asset of a game
//! in one file, fetched once at boot. A browser can't list a directory,
//! and one request beats a hundred small ones. `cargo xtask build web`
//! writes it from a Web bake (`index.toml` + blobs, the PC's formats); the
//! runtime reads it here.
//!
//! Layout (little-endian): magic `TPAK`, u32 version, u32 count, then per
//! asset a u8 [`Kind`], u32 id, u32 length and the bytes:
//!
//! - [`Kind::Sprite`]: a `TSPR` blob (u32 width, u32 height, RGBA8).
//! - [`Kind::SpriteKtx2`]: a `.ktx2` file (Basis Universal), transcoded at
//!   load to what the GPU samples.
//! - [`Kind::Sound`] / [`Kind::Music`]: `TS16` (u32 sample rate, u32
//!   frames, i16 mono samples) — half the PC's f32 `TSND`, the same PCM a
//!   browser plays.
//! - [`Kind::Model`]: a TMDL blob. [`Kind::Font`]: a TFNT glyph table (its
//!   atlas is the sprite with the same id). [`Kind::Blob`]: raw bytes.

/// The pack's magic and version.
pub const MAGIC: &[u8; 4] = b"TPAK";
pub const VERSION: u32 = 1;

/// What one entry holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sprite = 0,
    SpriteKtx2 = 1,
    Sound = 2,
    Music = 3,
    Model = 4,
    Font = 5,
    Blob = 6,
}

impl Kind {
    fn from_u8(v: u8) -> Option<Kind> {
        Some(match v {
            0 => Kind::Sprite,
            1 => Kind::SpriteKtx2,
            2 => Kind::Sound,
            3 => Kind::Music,
            4 => Kind::Model,
            5 => Kind::Font,
            6 => Kind::Blob,
            _ => return None,
        })
    }
}

/// One asset of a pack.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry<'a> {
    pub kind: Kind,
    pub id: u32,
    pub bytes: &'a [u8],
}

/// Write a pack.
pub fn write(entries: &[(Kind, u32, Vec<u8>)]) -> Vec<u8> {
    let total: usize = entries.iter().map(|e| 9 + e.2.len()).sum();
    let mut out = Vec::with_capacity(12 + total);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (kind, id, bytes) in entries {
        out.push(*kind as u8);
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    out
}

/// Read a pack: its entries, borrowing the bytes.
pub fn parse(pack: &[u8]) -> Result<Vec<Entry<'_>>, String> {
    let u32_at = |at: usize| -> Result<u32, String> {
        pack.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| "truncated asset pack".to_string())
    };
    if pack.get(..4) != Some(MAGIC) {
        return Err("not a Trino asset pack (no TPAK magic)".into());
    }
    let version = u32_at(4)?;
    if version != VERSION {
        return Err(format!(
            "asset pack version {version} (this engine reads {VERSION})"
        ));
    }
    let count = u32_at(8)? as usize;
    let mut at = 12;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = *pack.get(at).ok_or("truncated asset pack")?;
        let kind = Kind::from_u8(kind).ok_or_else(|| format!("unknown asset kind {kind}"))?;
        let id = u32_at(at + 1)?;
        let len = u32_at(at + 5)? as usize;
        let bytes = pack
            .get(at + 9..at + 9 + len)
            .ok_or("truncated asset pack")?;
        out.push(Entry { kind, id, bytes });
        at += 9 + len;
    }
    if at != pack.len() {
        return Err(format!(
            "{} stray bytes after the last asset",
            pack.len() - at
        ));
    }
    Ok(out)
}

/// A `TSND` blob (f32 samples) as `TS16`.
pub fn sound16(tsnd: &[u8]) -> Result<Vec<u8>, String> {
    if tsnd.len() < 12 || &tsnd[..4] != b"TSND" {
        return Err("bad sound blob".into());
    }
    let rate = u32::from_le_bytes(tsnd[4..8].try_into().unwrap());
    let count = u32::from_le_bytes(tsnd[8..12].try_into().unwrap()) as usize;
    let data = &tsnd[12..];
    if data.len() != count * 4 {
        return Err("truncated sound blob".into());
    }
    let mut out = Vec::with_capacity(12 + count * 2);
    out.extend_from_slice(b"TS16");
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(count as u32).to_le_bytes());
    for s in data.as_chunks::<4>().0 {
        let v = f32::from_le_bytes(*s).clamp(-1.0, 1.0);
        out.extend_from_slice(&((v * 32767.0).round() as i16).to_le_bytes());
    }
    Ok(out)
}

/// A `TS16` blob: its sample rate and samples as f32 in -1..=1.
pub fn read_sound16(blob: &[u8]) -> Result<(u32, Vec<f32>), String> {
    if blob.len() < 12 || &blob[..4] != b"TS16" {
        return Err("bad TS16 sound".into());
    }
    let rate = u32::from_le_bytes(blob[4..8].try_into().unwrap());
    let count = u32::from_le_bytes(blob[8..12].try_into().unwrap()) as usize;
    let data = &blob[12..];
    if data.len() != count * 2 {
        return Err("truncated TS16 sound".into());
    }
    let samples = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|s| i16::from_le_bytes(*s) as f32 / 32767.0)
        .collect();
    Ok((rate, samples))
}

/// A `TSPR` blob: width, height and its RGBA8 pixels.
pub fn read_sprite(blob: &[u8]) -> Result<(u32, u32, &[u8]), String> {
    if blob.len() < 12 || &blob[..4] != b"TSPR" {
        return Err("bad sprite blob".into());
    }
    let w = u32::from_le_bytes(blob[4..8].try_into().unwrap());
    let h = u32::from_le_bytes(blob[8..12].try_into().unwrap());
    let px = &blob[12..];
    if px.len() != (w * h * 4) as usize {
        return Err("truncated sprite blob".into());
    }
    Ok((w, h, px))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_round_trip_and_reject_damage() {
        let entries = vec![
            (Kind::Model, 7, b"TMDLxyz".to_vec()),
            (Kind::Blob, 9, Vec::new()),
            (Kind::SpriteKtx2, 1, vec![0xAB; 40]),
        ];
        let pack = write(&entries);
        let read = parse(&pack).unwrap();
        assert_eq!(read.len(), 3);
        for (e, (kind, id, bytes)) in read.iter().zip(&entries) {
            assert_eq!((e.kind, e.id, e.bytes), (*kind, *id, &bytes[..]));
        }
        assert!(parse(&pack[..pack.len() - 1]).is_err());
        let mut extra = pack.clone();
        extra.push(0);
        assert!(parse(&extra).is_err());
        assert!(parse(b"PNG\0").is_err());
        let mut v2 = pack;
        v2[4] = 2;
        assert!(parse(&v2).unwrap_err().contains("version"));
    }

    #[test]
    fn sounds_shrink_to_16_bits() {
        let samples = [0.0f32, 1.0, -1.0, 0.5, 2.0];
        let mut tsnd = b"TSND".to_vec();
        tsnd.extend_from_slice(&22050u32.to_le_bytes());
        tsnd.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        for s in samples {
            tsnd.extend_from_slice(&s.to_le_bytes());
        }
        let s16 = sound16(&tsnd).unwrap();
        assert_eq!(s16.len(), 12 + samples.len() * 2);
        let (rate, back) = read_sound16(&s16).unwrap();
        assert_eq!(rate, 22050);
        let want = [0.0, 1.0, -1.0, 0.5, 1.0];
        for (a, b) in back.iter().zip(want) {
            assert!((a - b).abs() < 1.0 / 16000.0, "{a} vs {b}");
        }
    }

    #[test]
    fn sprites_read_back() {
        let mut blob = b"TSPR".to_vec();
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            read_sprite(&blob).unwrap(),
            (1, 2, &[1u8, 2, 3, 4, 5, 6, 7, 8][..])
        );
        assert!(read_sprite(&blob[..15]).is_err());
    }
}
