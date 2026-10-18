//! `TBND`: one asset group baked for one platform (issue #56, ADR 0022).
//!
//! The bake writes `groups/<name>.tbnd` for every group but `boot` (which
//! stays the per-file `index.tsv` / `index.toml` of every earlier build).
//! A bundle is a header, an entry table, a string area and a payload area,
//! little-endian:
//!
//! ```text
//! header   24 bytes  "TBND" · u16 version · u16 entries · u32 group id
//!                    · u32 payload offset · u32 payload bytes · u32 reserved
//! entry    24 bytes  u8 kind · u8 codec · u8 flags · u8 tier · u32 id
//!                    · u32 offset · u32 stored bytes · u32 raw bytes · u32 hash
//! strings            external entries' file names (UTF-8, not terminated)
//! payload            inline entries' stored bytes
//! ```
//!
//! - An **inline** entry's bytes sit in the payload area at `offset`,
//!   stored as its `codec` says ([`Codec::Lz4`] or [`Codec::Stored`]); they
//!   decode to `raw` bytes, the memory the asset takes once loaded.
//! - An **external** entry ([`FLAG_EXTERNAL`]) names a file beside the
//!   bundle: the N64's DFS files (compressed by libdragon's `mkasset`,
//!   decoded by `asset_load`) and sounds that stream from storage.
//! - `hash` is the FNV-1a of the raw bytes: entries with the same bytes
//!   share one payload (the bake deduplicates by it).
//! - `tier` is the index's tier column ([`crate::asset::loads_on`]): 0 =
//!   every tier, 1 = base only, 2 = expanded only.
//!
//! [`Bundle::parse`] needs only the first [`Bundle::payload_offset`] bytes
//! (a loader reads the table first, then the payloads entry by entry) and
//! checks every offset against the sizes it declares, so a corrupt bundle is
//! an error, never an out-of-bounds read.

pub const MAGIC: [u8; 4] = *b"TBND";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 24;
pub const ENTRY_LEN: usize = 24;
/// The entry's bytes are a file beside the bundle (its name in the string
/// area), not in the payload.
pub const FLAG_EXTERNAL: u8 = 1;

/// What an entry holds, and so which backend registry it lands in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryKind {
    /// A sprite in the platform's format (`TSPR`, a `.sprite`, a `.t3x`).
    Sprite = 0,
    /// The PC's 3DS preview of the previous sprite entry (`TSPR`).
    SpritePreview3ds = 1,
    /// The PC's BCn blocks of the previous sprite entry (`TBCN`).
    SpriteBc = 2,
    /// The Web's KTX2 file of the previous sprite entry.
    SpriteKtx2 = 3,
    Sound = 4,
    Music = 5,
    /// A `TMDL` model.
    Model = 6,
    /// A `TFNT` glyph table (its atlas is a sprite entry with the same id).
    Font = 7,
    /// A blob, a baked scene or a particle effect.
    Blob = 8,
}

impl EntryKind {
    pub const ALL: [EntryKind; 9] = [
        EntryKind::Sprite,
        EntryKind::SpritePreview3ds,
        EntryKind::SpriteBc,
        EntryKind::SpriteKtx2,
        EntryKind::Sound,
        EntryKind::Music,
        EntryKind::Model,
        EntryKind::Font,
        EntryKind::Blob,
    ];

    pub const fn from_u8(v: u8) -> Option<EntryKind> {
        match v {
            0 => Some(EntryKind::Sprite),
            1 => Some(EntryKind::SpritePreview3ds),
            2 => Some(EntryKind::SpriteBc),
            3 => Some(EntryKind::SpriteKtx2),
            4 => Some(EntryKind::Sound),
            5 => Some(EntryKind::Music),
            6 => Some(EntryKind::Model),
            7 => Some(EntryKind::Font),
            8 => Some(EntryKind::Blob),
            _ => None,
        }
    }

    /// The index kind name (`index.tsv`, `index.toml`).
    pub const fn label(self) -> &'static str {
        match self {
            EntryKind::Sprite => "sprite",
            EntryKind::SpritePreview3ds => "sprite_3ds",
            EntryKind::SpriteBc => "sprite_bc",
            EntryKind::SpriteKtx2 => "sprite_ktx2",
            EntryKind::Sound => "sound",
            EntryKind::Music => "music",
            EntryKind::Model => "model",
            EntryKind::Font => "font",
            EntryKind::Blob => "blob",
        }
    }

    pub fn parse(label: &str) -> Option<EntryKind> {
        EntryKind::ALL.into_iter().find(|k| k.label() == label)
    }

    /// The [`crate::MemoryReport`] category its bytes count in.
    pub const fn category(self) -> MemoryCategory {
        match self {
            EntryKind::Sprite
            | EntryKind::SpritePreview3ds
            | EntryKind::SpriteBc
            | EntryKind::SpriteKtx2 => MemoryCategory::Textures,
            EntryKind::Sound | EntryKind::Music => MemoryCategory::Audio,
            EntryKind::Model => MemoryCategory::Meshes,
            EntryKind::Font | EntryKind::Blob => MemoryCategory::Data,
        }
    }
}

/// Where an asset's memory is counted ([`crate::MemoryReport`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemoryCategory {
    Meshes,
    Textures,
    Audio,
    /// Fonts' glyph tables, blobs, baked scenes, particle effects.
    Data,
}

/// How an inline entry's bytes are stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    Stored = 0,
    /// An LZ4 block ([`crate::lz4`]).
    Lz4 = 1,
    /// An external file in libdragon's compressed-asset format (`mkasset`,
    /// level 1 LZ4, 2 aPLib, 3 Shrinkler), or plain: `asset_load` reads both.
    Libdragon = 2,
}

impl Codec {
    pub const fn from_u8(v: u8) -> Option<Codec> {
        match v {
            0 => Some(Codec::Stored),
            1 => Some(Codec::Lz4),
            2 => Some(Codec::Libdragon),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Codec::Stored => "none",
            Codec::Lz4 => "lz4",
            Codec::Libdragon => "libdragon",
        }
    }
}

/// One asset of a bundle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry {
    pub kind: EntryKind,
    pub codec: Codec,
    pub flags: u8,
    /// 0 = every tier, 1 = base, 2 = expanded.
    pub tier: u8,
    /// The asset's handle (`asset_id` of its logical path).
    pub id: u32,
    /// Inline: from the payload area's start. External: into the string area.
    pub offset: u32,
    /// Inline: bytes in the payload. External: the file name's length.
    pub stored: u32,
    /// Bytes once loaded (decoded).
    pub raw: u32,
    /// FNV-1a of the raw bytes.
    pub hash: u32,
}

impl Entry {
    pub const fn is_external(&self) -> bool {
        self.flags & FLAG_EXTERNAL != 0
    }

    /// Does this entry load on `tier` hardware? (The index's tier column.)
    pub fn loads_on(&self, tier: crate::HwTier) -> bool {
        match self.tier {
            1 => tier == crate::HwTier::Base,
            2 => tier == crate::HwTier::Expanded,
            _ => true,
        }
    }

    /// The bytes the loader reads for it: the stored payload, or for an
    /// external file its decoded size (what the read costs in RAM).
    pub const fn read_bytes(&self) -> u32 {
        if self.is_external() {
            self.raw
        } else {
            self.stored
        }
    }

    fn write(&self, out: &mut [u8]) {
        out[0] = self.kind as u8;
        out[1] = self.codec as u8;
        out[2] = self.flags;
        out[3] = self.tier;
        out[4..8].copy_from_slice(&self.id.to_le_bytes());
        out[8..12].copy_from_slice(&self.offset.to_le_bytes());
        out[12..16].copy_from_slice(&self.stored.to_le_bytes());
        out[16..20].copy_from_slice(&self.raw.to_le_bytes());
        out[20..24].copy_from_slice(&self.hash.to_le_bytes());
    }
}

/// Why bytes are not a bundle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BundleError {
    BadMagic,
    /// A version this engine can't read (newer).
    Version(u16),
    /// Shorter than its header, its table or its payload says.
    Truncated,
    /// An entry with an unknown kind or codec, or bytes out of bounds.
    BadEntry(u16),
}

impl BundleError {
    pub const fn label(self) -> &'static str {
        match self {
            BundleError::BadMagic => "not a TBND bundle",
            BundleError::Version(_) => "TBND version newer than this engine",
            BundleError::Truncated => "truncated TBND bundle",
            BundleError::BadEntry(_) => "corrupt TBND entry",
        }
    }
}

/// A parsed bundle: a view over its header and table.
#[derive(Clone, Copy, Debug)]
pub struct Bundle<'a> {
    bytes: &'a [u8],
    count: u16,
    group: u32,
    payload_offset: u32,
    payload_len: u32,
}

impl<'a> Bundle<'a> {
    /// How many bytes of the file [`parse`](Self::parse) needs (header,
    /// table and strings: the payload offset), read from its first
    /// [`HEADER_LEN`] bytes.
    pub fn prefix_len(header: &[u8]) -> Result<usize, BundleError> {
        if header.len() < HEADER_LEN {
            return Err(BundleError::Truncated);
        }
        if header[..4] != MAGIC {
            return Err(BundleError::BadMagic);
        }
        Ok(u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize)
    }

    /// Parse a bundle's header and table. `bytes` may stop at the payload
    /// (`payload_offset`) or hold the whole file.
    pub fn parse(bytes: &'a [u8]) -> Result<Bundle<'a>, BundleError> {
        if bytes.len() < HEADER_LEN {
            return Err(BundleError::Truncated);
        }
        if bytes[..4] != MAGIC {
            return Err(BundleError::BadMagic);
        }
        let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let u32_at =
            |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let version = u16_at(4);
        if version == 0 || version > VERSION {
            return Err(BundleError::Version(version));
        }
        let count = u16_at(6);
        let group = u32_at(8);
        let payload_offset = u32_at(12);
        let payload_len = u32_at(16);
        let table_end = HEADER_LEN + count as usize * ENTRY_LEN;
        if (payload_offset as usize) < table_end || bytes.len() < payload_offset as usize {
            return Err(BundleError::Truncated);
        }
        let b = Bundle {
            bytes,
            count,
            group,
            payload_offset,
            payload_len,
        };
        let strings = (payload_offset as usize - table_end) as u64;
        for i in 0..count {
            let e = b.read_entry(i).ok_or(BundleError::BadEntry(i))?;
            let end = e.offset as u64 + e.stored as u64;
            let limit = if e.is_external() {
                strings
            } else {
                payload_len as u64
            };
            if end > limit || (!e.is_external() && e.codec == Codec::Libdragon) {
                return Err(BundleError::BadEntry(i));
            }
            if e.is_external() && b.path(&e).is_none() {
                return Err(BundleError::BadEntry(i));
            }
        }
        Ok(b)
    }

    /// The group this bundle holds (`GroupId::from_name`).
    pub const fn group(&self) -> u32 {
        self.group
    }

    pub const fn len(&self) -> usize {
        self.count as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Where the payload area starts in the file.
    pub const fn payload_offset(&self) -> u32 {
        self.payload_offset
    }

    /// The payload area's size.
    pub const fn payload_len(&self) -> u32 {
        self.payload_len
    }

    /// The whole file's size (header, table, strings, payload).
    pub const fn file_len(&self) -> u64 {
        self.payload_offset as u64 + self.payload_len as u64
    }

    fn read_entry(&self, i: u16) -> Option<Entry> {
        let o = HEADER_LEN + i as usize * ENTRY_LEN;
        let e = &self.bytes[o..o + ENTRY_LEN];
        let u32_at = |o: usize| u32::from_le_bytes([e[o], e[o + 1], e[o + 2], e[o + 3]]);
        Some(Entry {
            kind: EntryKind::from_u8(e[0])?,
            codec: Codec::from_u8(e[1])?,
            flags: e[2],
            tier: if e[3] <= 2 { e[3] } else { return None },
            id: u32_at(4),
            offset: u32_at(8),
            stored: u32_at(12),
            raw: u32_at(16),
            hash: u32_at(20),
        })
    }

    /// The `i`th entry (`i < len()`).
    pub fn entry(&self, i: usize) -> Entry {
        self.read_entry(i as u16).expect("checked by parse")
    }

    pub fn entries(&self) -> impl Iterator<Item = Entry> + '_ {
        (0..self.count).map(|i| self.read_entry(i).expect("checked by parse"))
    }

    /// An external entry's file name (relative to the bundle's directory).
    pub fn path(&self, e: &Entry) -> Option<&'a str> {
        if !e.is_external() {
            return None;
        }
        let start = HEADER_LEN + self.count as usize * ENTRY_LEN + e.offset as usize;
        let bytes = self.bytes.get(start..start + e.stored as usize)?;
        core::str::from_utf8(bytes).ok()
    }

    /// An inline entry's byte range in the file (`None` for an external one).
    pub fn payload_range(&self, e: &Entry) -> Option<core::ops::Range<u64>> {
        if e.is_external() {
            return None;
        }
        let start = self.payload_offset as u64 + e.offset as u64;
        Some(start..start + e.stored as u64)
    }

    /// An inline entry's stored bytes, when `bytes` holds the whole file.
    pub fn stored(&self, e: &Entry) -> Option<&'a [u8]> {
        let r = self.payload_range(e)?;
        self.bytes.get(r.start as usize..r.end as usize)
    }

    /// The bytes all entries that load on `tier` read (progress' 100%).
    pub fn read_total(&self, tier: crate::HwTier) -> u64 {
        self.entries()
            .filter(|e| e.loads_on(tier))
            .map(|e| e.read_bytes() as u64)
            .sum()
    }
}

/// Decode an inline entry's stored bytes into `out` (`out.len()` = `raw`).
pub fn decode(e: &Entry, stored: &[u8], out: &mut [u8]) -> Result<(), crate::lz4::Lz4Error> {
    match e.codec {
        Codec::Stored | Codec::Libdragon => {
            if stored.len() != out.len() {
                return Err(crate::lz4::Lz4Error::OutputShort);
            }
            out.copy_from_slice(stored);
            Ok(())
        }
        Codec::Lz4 => crate::lz4::decompress_exact(stored, out),
    }
}

/// FNV-1a of a byte string (an entry's `hash`, the bake's dedup key).
pub fn hash(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The handle of the group `name` (`groups/<name>`).
pub const fn group_id(name: &str) -> u32 {
    crate::asset::asset_id_prefixed("groups/", name)
}

/// Write a bundle's header, table and strings into `out` (the bake's side,
/// kept here so the layout has one definition). `strings` is the string
/// area; the payload follows what this writes. Returns the bytes written,
/// or `None` when `out` is too small.
pub fn write_table(
    group: u32,
    entries: &[Entry],
    strings: &[u8],
    payload_len: u32,
    out: &mut [u8],
) -> Option<usize> {
    let table = HEADER_LEN + entries.len() * ENTRY_LEN;
    let len = table + strings.len();
    if out.len() < len || entries.len() > u16::MAX as usize {
        return None;
    }
    out[..4].copy_from_slice(&MAGIC);
    out[4..6].copy_from_slice(&VERSION.to_le_bytes());
    out[6..8].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    out[8..12].copy_from_slice(&group.to_le_bytes());
    out[12..16].copy_from_slice(&(len as u32).to_le_bytes());
    out[16..20].copy_from_slice(&payload_len.to_le_bytes());
    out[20..24].fill(0);
    for (i, e) in entries.iter().enumerate() {
        let o = HEADER_LEN + i * ENTRY_LEN;
        e.write(&mut out[o..o + ENTRY_LEN]);
    }
    out[table..len].copy_from_slice(strings);
    Some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let model = [7u8; 40];
        let packed = [0x40, 1, 2, 3, 4]; // 4 literals
        let entries = [
            Entry {
                kind: EntryKind::Model,
                codec: Codec::Stored,
                flags: 0,
                tier: 0,
                id: 11,
                offset: 0,
                stored: 40,
                raw: 40,
                hash: hash(&model),
            },
            Entry {
                kind: EntryKind::Blob,
                codec: Codec::Lz4,
                flags: 0,
                tier: 2,
                id: 12,
                offset: 40,
                stored: 5,
                raw: 4,
                hash: hash(&[1, 2, 3, 4]),
            },
            Entry {
                kind: EntryKind::Sound,
                codec: Codec::Stored,
                flags: FLAG_EXTERNAL,
                tier: 0,
                id: 13,
                offset: 0,
                stored: 9,
                raw: 300,
                hash: 0,
            },
        ];
        let strings = b"0000000d.";
        let mut out = vec![0u8; 512];
        let n = write_table(group_id("level1"), &entries, strings, 45, &mut out).unwrap();
        out.truncate(n);
        out.extend_from_slice(&model);
        out.extend_from_slice(&packed);
        out
    }

    #[test]
    fn round_trip() {
        let bytes = sample();
        let b = Bundle::parse(&bytes).unwrap();
        assert_eq!(b.group(), group_id("level1"));
        assert_eq!(b.len(), 3);
        assert_eq!(b.file_len(), bytes.len() as u64);
        let es: Vec<Entry> = b.entries().collect();
        assert_eq!(b.stored(&es[0]), Some(&[7u8; 40][..]));
        let mut raw = [0u8; 4];
        decode(&es[1], b.stored(&es[1]).unwrap(), &mut raw).unwrap();
        assert_eq!(raw, [1, 2, 3, 4]);
        assert_eq!(hash(&raw), es[1].hash);
        assert_eq!(b.path(&es[2]), Some("0000000d."));
        assert!(es[1].loads_on(crate::HwTier::Expanded));
        assert!(!es[1].loads_on(crate::HwTier::Base));
        assert_eq!(b.read_total(crate::HwTier::Base), 40 + 300);
        // The table alone (a loader's first read) parses too.
        let table = Bundle::prefix_len(&bytes).unwrap();
        assert!(Bundle::parse(&bytes[..table]).is_ok());
    }

    #[test]
    fn corrupt_bundles_are_errors() {
        let good = sample();
        assert_eq!(
            Bundle::parse(&good[..10]).err(),
            Some(BundleError::Truncated)
        );
        let mut bad = good.clone();
        bad[0] = b'X';
        assert_eq!(Bundle::parse(&bad).err(), Some(BundleError::BadMagic));
        let mut bad = good.clone();
        bad[4] = 9;
        assert_eq!(Bundle::parse(&bad).err(), Some(BundleError::Version(9)));
        // An entry whose bytes run past the payload.
        let mut bad = good.clone();
        let o = HEADER_LEN + ENTRY_LEN + 12;
        bad[o..o + 4].copy_from_slice(&1000u32.to_le_bytes());
        assert_eq!(Bundle::parse(&bad).err(), Some(BundleError::BadEntry(1)));
        // An unknown kind.
        let mut bad = good.clone();
        bad[HEADER_LEN] = 200;
        assert_eq!(Bundle::parse(&bad).err(), Some(BundleError::BadEntry(0)));
        // A table that claims more entries than it holds.
        let mut bad = good.clone();
        bad[6] = 40;
        assert_eq!(Bundle::parse(&bad).err(), Some(BundleError::Truncated));
    }
}
