//! Stable asset identity.
//!
//! An asset handle is the FNV-1a hash of its **logical path** — the path
//! declared in `assets/manifest.toml`, e.g. `"sprites/player"`. Games compute
//! handles at compile time (`const`), the asset pipeline computes the same
//! hash at bake time, and platforms load baked data by hash. Renaming an
//! asset therefore changes its handle (a deliberate breaking change);
//! re-baking its content does not — which is what makes live reload swap
//! content in place without invalidating anything.
//!
//! The pipeline fails the bake if two logical paths collide.

/// FNV-1a 32-bit. `const`, `no_std`, dependency-free — must produce the same
/// value in game code, pipeline and platforms.
pub const fn asset_id(logical_path: &str) -> u32 {
    let bytes = logical_path.as_bytes();
    let mut hash: u32 = 0x811c9dc5;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

/// [`asset_id`] of `prefix` followed by `name` (`"groups/"` + `"level1"`),
/// without building the string: `const` handles from a bare name.
pub const fn asset_id_prefixed(prefix: &str, name: &str) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    let parts = [prefix.as_bytes(), name.as_bytes()];
    let mut p = 0;
    while p < parts.len() {
        let bytes = parts[p];
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u32;
            hash = hash.wrapping_mul(0x0100_0193);
            i += 1;
        }
        p += 1;
    }
    hash
}

/// Whether a console's asset-index line with the tier column `column`
/// (`None` = no column: every tier; `"base"` or `"expanded"`, the
/// [`HwTier::key`](crate::HwTier::key)s) loads on `tier` hardware. The N64
/// and 3DS builds hold both tiers' data where it differs (issue #54, ADR
/// 0020): a scene or particle effect baked twice under one handle, a model
/// listed for the expanded tier only. An unknown tier name loads nowhere.
pub fn loads_on(column: Option<&str>, tier: crate::HwTier) -> bool {
    match column.map(str::trim) {
        None | Some("") => true,
        Some(name) => name == tier.key(),
    }
}

/// What an [`IndexLine`]'s kind column names: an asset for a backend, or
/// one of the index's own lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IndexKind<'a> {
    /// An asset ([`EntryKind`](crate::bundle::EntryKind)'s label).
    Entry(crate::bundle::EntryKind),
    /// `<group id> group groups/<g>.tbnd`: a bundle the game loads later.
    Group,
    /// The 3DS's packed boot bundle (`groups/boot.tbnd`).
    Boot,
    /// The N64's libdragon decompression level (the id column) to register
    /// before the assets it decodes.
    Compression,
    /// The project's audio mix (`TMIX`).
    Mixer,
    /// A kind this build doesn't know (skipped).
    Unknown(&'a str),
}

impl<'a> IndexKind<'a> {
    pub fn parse(label: &'a str) -> IndexKind<'a> {
        match label {
            "group" => IndexKind::Group,
            "boot" => IndexKind::Boot,
            "compression" => IndexKind::Compression,
            "mixer" => IndexKind::Mixer,
            _ => match crate::bundle::EntryKind::parse(label) {
                Some(k) => IndexKind::Entry(k),
                None => IndexKind::Unknown(label),
            },
        }
    }
}

/// One line of a console build's asset index (`index.tsv` in the N64's DFS
/// and the 3DS's RomFS), tab-separated: `id` (hex), `kind`, `file`, then an
/// optional tier column ([`loads_on`]) and the kind's own columns (a
/// sound's codec and bus). The N64 and 3DS loaders share this parse, which
/// the `asset_index` fuzz target and the unit tests below cover on the host
/// (issue #121). No allocation: every field borrows the line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IndexLine<'a> {
    pub id: u32,
    pub kind: IndexKind<'a>,
    /// The file, relative to the DFS / RomFS root.
    pub file: &'a str,
    /// The tier column as written (`None`: no column).
    pub tier: Option<&'a str>,
    /// Everything after the tier column (`None`: nothing there).
    rest: Option<&'a str>,
}

impl<'a> IndexLine<'a> {
    /// `None` for a line with fewer than three columns or an id that isn't
    /// hex (the loaders skip it). The kind may be unknown
    /// ([`IndexKind::Unknown`]).
    pub fn parse(line: &'a str) -> Option<IndexLine<'a>> {
        let mut cols = line.splitn(5, '\t');
        let (id_hex, kind, file) = (cols.next()?, cols.next()?, cols.next()?);
        let tier = cols.next();
        let rest = cols.next();
        let id = u32::from_str_radix(id_hex, 16).ok()?;
        Some(IndexLine {
            id,
            kind: IndexKind::parse(kind),
            file,
            tier,
            rest,
        })
    }

    /// Whether this line loads on `tier` hardware ([`loads_on`]).
    pub fn loads_on(&self, tier: crate::HwTier) -> bool {
        loads_on(self.tier, tier)
    }

    /// The kind's own columns, after the tier column (a sound's
    /// [`SoundColumns`]).
    pub fn extra(&self) -> Columns<'a> {
        Columns(self.rest.map(|r| r.split('\t')))
    }
}

/// The columns after an index line's fixed ones ([`IndexLine::extra`],
/// [`Locator::extra`]).
#[derive(Clone, Debug)]
pub struct Columns<'a>(Option<core::str::Split<'a, char>>);

impl<'a> Iterator for Columns<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<&'a str> {
        self.0.as_mut()?.next()
    }
}

/// A bundle entry's locator (`file`, then the same per-kind columns as an
/// index line: `file \t codec \t bus` for a sound), what an external entry
/// of a group's `TBND` names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Locator<'a> {
    pub file: &'a str,
    rest: Option<&'a str>,
}

impl<'a> Locator<'a> {
    pub fn parse(locator: &'a str) -> Locator<'a> {
        let mut cols = locator.splitn(2, '\t');
        Locator {
            file: cols.next().unwrap_or(""),
            rest: cols.next(),
        }
    }

    pub fn extra(&self) -> Columns<'a> {
        Columns(self.rest.map(|r| r.split('\t')))
    }
}

/// A sound's or music's columns (N64): the codec (`vadpcm` when missing,
/// `opus`, `pcm`) and the bus label (`None`: missing or unknown, the
/// loader picks Music or Sfx by kind).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SoundColumns<'a> {
    pub codec: &'a str,
    pub bus: Option<crate::Bus>,
}

impl<'a> SoundColumns<'a> {
    pub fn read(cols: &mut impl Iterator<Item = &'a str>) -> SoundColumns<'a> {
        let codec = cols.next().unwrap_or("vadpcm");
        let bus = cols.next().and_then(crate::Bus::from_label);
        SoundColumns { codec, bus }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_lines_parse_like_the_console_loaders_split_them() {
        use crate::HwTier::{Base, Expanded};
        use crate::bundle::EntryKind;
        let l = IndexLine::parse("0000abcd\tsound\t0000abcd.wav64\t\topus\tmusic").unwrap();
        assert_eq!(l.id, 0xabcd);
        assert_eq!(l.kind, IndexKind::Entry(EntryKind::Sound));
        assert_eq!(l.file, "0000abcd.wav64");
        assert_eq!(l.tier, Some(""));
        assert!(l.loads_on(Base) && l.loads_on(Expanded));
        let s = SoundColumns::read(&mut l.extra());
        assert_eq!((s.codec, s.bus), ("opus", Some(crate::Bus::Music)));
        // No columns past the file: every tier, the codec's default.
        let b = IndexLine::parse("00000001\tblob\t00000001.blob").unwrap();
        assert_eq!((b.tier, b.extra().count()), (None, 0));
        assert_eq!(SoundColumns::read(&mut b.extra()).codec, "vadpcm");
        let x = IndexLine::parse("00000001\tblob\t00000001.x.blob\texpanded").unwrap();
        assert!(x.loads_on(Expanded) && !x.loads_on(Base));
        assert_eq!(x.extra().count(), 0);
        // The index's own lines, and unknown kinds (skipped by the loaders).
        let kind = |l: &'static str| IndexLine::parse(l).unwrap().kind;
        assert_eq!(kind("2\tcompression\tlibdragon"), IndexKind::Compression);
        assert_eq!(kind("abc\tgroup\tgroups/l1.tbnd"), IndexKind::Group);
        assert_eq!(kind("abc\tboot\tgroups/boot.tbnd"), IndexKind::Boot);
        assert_eq!(kind("0\tmixer\taudio/mixer"), IndexKind::Mixer);
        assert_eq!(kind("0\tdeluxe\tf"), IndexKind::Unknown("deluxe"));
        // Fewer than three columns, or an id that isn't hex: skipped.
        for bad in [
            "",
            "00000001",
            "00000001\tblob",
            "zz\tblob\tf",
            "1ffffffff\tblob\tf",
        ] {
            assert_eq!(IndexLine::parse(bad), None, "{bad:?}");
        }
        // An unknown tier loads nowhere; the bus is optional.
        let u = IndexLine::parse("1\tsound\tf\tdeluxe\tvadpcm\tnope").unwrap();
        assert!(!u.loads_on(Base) && !u.loads_on(Expanded));
        assert_eq!(SoundColumns::read(&mut u.extra()).bus, None);
        // A trailing empty tier column, and columns past the bus.
        let t = IndexLine::parse("1\tmusic\tf\t").unwrap();
        assert_eq!((t.tier, t.extra().count()), (Some(""), 0));
        let more = IndexLine::parse("1\tmusic\tf\tbase\tpcm\tui\tlater").unwrap();
        assert_eq!(more.extra().collect::<Vec<_>>(), ["pcm", "ui", "later"]);
        // A group's locator: the file, then the same columns.
        let loc = Locator::parse("0000abcd.wav64\tvadpcm\tsfx");
        assert_eq!(loc.file, "0000abcd.wav64");
        let s = SoundColumns::read(&mut loc.extra());
        assert_eq!((s.codec, s.bus), ("vadpcm", Some(crate::Bus::Sfx)));
        assert_eq!(Locator::parse("").file, "");
        assert_eq!(Locator::parse("a.blob").extra().count(), 0);
    }

    #[test]
    fn is_const_and_stable() {
        // Golden values: changing the hash function breaks every baked asset
        // and every scene file in existence. Do not.
        const PLAYER: u32 = asset_id("sprites/player");
        assert_eq!(PLAYER, asset_id("sprites/player"));
        assert_eq!(asset_id(""), 0x811c9dc5);
        assert_ne!(asset_id("sprites/player"), asset_id("sprites/enemy"));
        assert_eq!(
            asset_id_prefixed("groups/", "level1"),
            asset_id("groups/level1")
        );
    }

    #[test]
    fn index_lines_load_on_their_tier() {
        use crate::HwTier::{Base, Expanded};
        for t in [Base, Expanded] {
            assert!(loads_on(None, t));
            assert!(loads_on(Some(""), t));
            assert!(!loads_on(Some("deluxe"), t));
        }
        assert!(loads_on(Some("base"), Base) && !loads_on(Some("base"), Expanded));
        assert!(loads_on(Some("expanded"), Expanded) && !loads_on(Some("expanded"), Base));
    }
}
