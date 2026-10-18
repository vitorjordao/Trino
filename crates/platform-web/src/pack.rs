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
//! - [`Kind::Sound`] / [`Kind::Music`]: a `TSN2` blob
//!   ([`trino_core::SoundBlob`], issue #53) as the Web bake made it: Ogg
//!   Vorbis by default, PCM16 where the manifest says `codec = "pcm"`,
//!   with its bus, loop and stream flags. Version 1 packs held `TS16`
//!   (u32 sample rate, u32 frames, i16 mono samples); [`sound_blob`]
//!   migrates those on read.
//! - [`Kind::Model`]: a TMDL blob. [`Kind::Font`]: a TFNT glyph table (its
//!   atlas is the sprite with the same id). [`Kind::Blob`]: raw bytes.
//! - [`Kind::Mixer`] (version 2): the project's mix (`assets/audio.toml`
//!   baked to `TMIX`, [`trino_core::MixerConfig`]).
//! - [`Kind::Group`] (version 3, issue #56): an asset group the pack's
//!   assets leave out, by [`trino_core::GroupId`], its bytes the group's
//!   name. Its bundle (`trino_core::bundle`) sits beside the pack as
//!   `<pack>.<group>.tbnd` (`assets/trino.level1.tbnd`), fetched when the
//!   game loads the group.

use std::borrow::Cow;

/// The pack's magic and version (2: `TSN2` sounds and the mixer, #53; 3:
/// asset groups, #56).
pub const MAGIC: &[u8; 4] = b"TPAK";
pub const VERSION: u32 = 3;
/// The oldest version [`parse`] still reads.
pub const OLDEST: u32 = 1;

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
    Mixer = 7,
    Group = 8,
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
            7 => Kind::Mixer,
            8 => Kind::Group,
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

/// Read a pack (this version or an older one): its entries, borrowing the
/// bytes. Older entries are migrated where they are used ([`sound_blob`]).
pub fn parse(pack: &[u8]) -> Result<Vec<Entry<'_>>, String> {
    parse_versioned(pack).map(|(_, e)| e)
}

/// [`parse`] and the pack's version.
pub fn parse_versioned(pack: &[u8]) -> Result<(u32, Vec<Entry<'_>>), String> {
    let u32_at = |at: usize| -> Result<u32, String> {
        pack.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| "truncated asset pack".to_string())
    };
    if pack.get(..4) != Some(MAGIC) {
        return Err("not a Trino asset pack (no TPAK magic)".into());
    }
    let version = u32_at(4)?;
    if !(OLDEST..=VERSION).contains(&version) {
        return Err(format!(
            "asset pack version {version} (this engine reads {OLDEST} to {VERSION})"
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
        if kind == Kind::Mixer && version < 2 {
            return Err(format!("a mixer entry in a version {version} pack"));
        }
        if kind == Kind::Group && version < 3 {
            return Err(format!("a group entry in a version {version} pack"));
        }
        out.push(Entry { kind, id, bytes });
        at += 9 + len;
    }
    if at != pack.len() {
        return Err(format!(
            "{} stray bytes after the last asset",
            pack.len() - at
        ));
    }
    Ok((version, out))
}

/// A sound or music entry as a `TSN2` blob: as stored (version 2), or a
/// version 1 `TS16` migrated to PCM16 with the kind's defaults (sounds on
/// the effects bus, music on the music bus, looping; nothing streams).
pub fn sound_blob(kind: Kind, bytes: &[u8]) -> Result<Cow<'_, [u8]>, String> {
    if bytes.starts_with(&trino_core::SoundBlob::MAGIC) {
        trino_core::SoundBlob::parse(bytes).map_err(String::from)?;
        return Ok(Cow::Borrowed(bytes));
    }
    if !bytes.starts_with(b"TS16") {
        return Err("bad sound entry (neither TSN2 nor TS16)".into());
    }
    let (rate, samples) = read_sound16(bytes)?;
    let music = kind == Kind::Music;
    let payload = &bytes[12..];
    let header = trino_core::SoundBlob {
        codec: trino_core::SoundCodec::Pcm16,
        bus: if music {
            trino_core::Bus::Music
        } else {
            trino_core::Bus::Sfx
        },
        looped: music,
        stream: false,
        sample_rate: rate,
        frames: samples.len() as u32,
        payload,
    }
    .header();
    let mut out = header.to_vec();
    out.extend_from_slice(payload);
    Ok(Cow::Owned(out))
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

/// Where a group's bundle sits beside the pack at `pack_url`
/// (`assets/trino.pak` → `assets/trino.level1.tbnd`).
pub fn bundle_url(pack_url: &str, group: &str) -> String {
    let stem = pack_url.strip_suffix(".pak").unwrap_or(pack_url);
    format!("{stem}.{group}.tbnd")
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
        let mut v4 = pack;
        v4[4] = 4;
        assert!(parse(&v4).unwrap_err().contains("version"));
    }

    #[test]
    fn version_1_sounds_migrate_to_tsn2() {
        let mut ts16 = b"TS16".to_vec();
        ts16.extend_from_slice(&22050u32.to_le_bytes());
        ts16.extend_from_slice(&3u32.to_le_bytes());
        for s in [0i16, 32767, -32767] {
            ts16.extend_from_slice(&s.to_le_bytes());
        }
        let blob = sound_blob(Kind::Music, &ts16).unwrap();
        let b = trino_core::SoundBlob::parse(&blob).unwrap();
        assert_eq!(b.codec, trino_core::SoundCodec::Pcm16);
        assert_eq!((b.sample_rate, b.frames), (22050, 3));
        assert_eq!(b.bus, trino_core::Bus::Music);
        assert!(b.looped && !b.stream);
        assert_eq!(b.payload, &ts16[12..]);
        let sfx = sound_blob(Kind::Sound, &ts16).unwrap();
        assert_eq!(
            trino_core::SoundBlob::parse(&sfx).unwrap().bus,
            trino_core::Bus::Sfx
        );
        // A v2 entry passes through untouched; damage is an error.
        assert!(matches!(
            sound_blob(Kind::Sound, &blob).unwrap(),
            Cow::Borrowed(_)
        ));
        assert!(sound_blob(Kind::Sound, &ts16[..13]).is_err());
        assert!(sound_blob(Kind::Sound, b"TSND....").is_err());
        assert!(sound_blob(Kind::Sound, &blob[..blob.len() - 1]).is_err());

        // A version 1 pack still parses; a mixer can't be in one.
        let mut v1 = write(&[(Kind::Sound, 3, ts16.clone())]);
        v1[4] = 1;
        let (version, e) = parse_versioned(&v1).unwrap();
        assert_eq!((version, e[0].kind), (1, Kind::Sound));
        let mut bad = write(&[(Kind::Mixer, 3, b"TMIX".to_vec())]);
        bad[4] = 1;
        assert!(parse(&bad).unwrap_err().contains("mixer"));
        // Groups came with version 3.
        let group = write(&[(Kind::Group, 5, b"level1".to_vec())]);
        assert_eq!(parse(&group).unwrap()[0].bytes, b"level1");
        let mut old = group;
        old[4] = 2;
        assert!(parse(&old).unwrap_err().contains("group"));
    }

    #[test]
    fn bundles_sit_beside_their_pack() {
        assert_eq!(
            bundle_url("assets/trino.pak", "level1"),
            "assets/trino.level1.tbnd"
        );
        assert_eq!(bundle_url("x", "g"), "x.g.tbnd");
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
