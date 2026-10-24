//! `TACH`: a project's achievements, stats and rich-presence keys, baked
//! from `assets/achievements.ron` into the `services/achievements` blob on
//! every target (no manifest entry, issue #61). Zero-copy:
//! [`AchievementTable::parse`] checks it once, lookups walk it (a game
//! declares at most [`MAX_ACHIEVEMENTS`](super::MAX_ACHIEVEMENTS) achievements).
//!
//! An achievement's **bit** in [`AchievementState`](super::AchievementState)
//! is its position in the file: append new ones, never reorder or remove
//! (a save made by an older build would read the wrong bits).
//! `cargo xtask services check` warns about a removed or moved id against
//! the last export.
//!
//! Layout (little-endian), version 1:
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic `TACH` |
//! | 4 | 2 | version (1) |
//! | 6 | 2 | achievements |
//! | 8 | 2 | stats |
//! | 10 | 2 | presence keys |
//! | 12 | 4 | reserved |
//! | 16 | 28 × achievements | id (`asset_id(api name)`), api name, name key, description key (u16 offset + u16 length into the pool each), icon sprite id (0 = none), flags (u8: bit 0 hidden), stat index (u8, 0xFF = none), reserved (u16), threshold (i32) |
//! | … | 20 × stats | id, api name (offset, length), min (i32), max (i32), flags (u16: bit 0 increment only), reserved (u16) |
//! | … | 8 × presence | id (`asset_id(key)`), key (offset, length) |
//! | … | rest | string pool (UTF-8) |

use super::{AchievementId, MAX_ACHIEVEMENTS, MAX_STATS, StatId};
use crate::i18n::StrKey;
use crate::platform::{BlobId, Platform};
use crate::render::SpriteId;

pub const MAGIC: &[u8; 4] = b"TACH";
pub const VERSION: u16 = 1;
/// The blob's logical path.
pub const LOGICAL: &str = "services/achievements";
/// The blob's handle.
pub const BLOB: BlobId = BlobId::from_path(LOGICAL);
pub const HEADER: usize = 16;
pub const ACHIEVEMENT_RECORD: usize = 28;
pub const STAT_RECORD: usize = 20;
pub const PRESENCE_RECORD: usize = 8;
/// [`AchievementInfo::stat`] when the achievement has no threshold.
const NO_STAT: u8 = 0xFF;

/// Why a blob was refused, or why one could not be written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    /// A string outside the pool, or not UTF-8.
    BadString,
    /// More than [`MAX_ACHIEVEMENTS`] achievements or [`MAX_STATS`] stats.
    TooMany,
    /// An achievement's threshold names a stat that isn't there.
    BadStat,
    /// The output buffer is too small.
    NoRoom,
}

/// One achievement as the table holds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AchievementInfo<'a> {
    pub id: AchievementId,
    /// Its bit in [`AchievementState`](super::AchievementState).
    pub index: usize,
    /// The store's name for it (`FIRST_COIN`): Steam's API name.
    pub api: &'a str,
    /// String-table keys (`ach.first_coin.name`): the text in the player's
    /// language through [`crate::ui::UiStrings`].
    pub name: &'a str,
    pub description: &'a str,
    /// Its icon (`None`: the list draws a medal).
    pub icon: Option<SpriteId>,
    /// Hidden until unlocked: the list shows `???`.
    pub hidden: bool,
    /// Unlocked by the host when this stat reaches `threshold`.
    pub stat: Option<usize>,
    pub threshold: i32,
}

/// One stat.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StatInfo<'a> {
    pub id: StatId,
    pub index: usize,
    pub api: &'a str,
    pub min: i32,
    pub max: i32,
    /// A lower value is refused (Steam's "increment only"); restoring
    /// keeps the higher of the two.
    pub increment_only: bool,
}

/// A parsed `services/achievements` blob.
#[derive(Clone, Copy, Debug)]
pub struct AchievementTable<'a> {
    achievements: &'a [u8],
    stats: &'a [u8],
    presence: &'a [u8],
    pool: &'a str,
}

impl<'a> AchievementTable<'a> {
    /// No table (a game without `assets/achievements.ron`): nothing to
    /// unlock.
    pub const EMPTY: AchievementTable<'static> = AchievementTable {
        achievements: &[],
        stats: &[],
        presence: &[],
        pool: "",
    };

    pub fn parse(bytes: &'a [u8]) -> Result<AchievementTable<'a>, TableError> {
        if bytes.len() < HEADER {
            return Err(TableError::Truncated);
        }
        if &bytes[0..4] != MAGIC {
            return Err(TableError::BadMagic);
        }
        let version = rd16(bytes, 4);
        if version != VERSION {
            return Err(TableError::UnsupportedVersion(version));
        }
        let (na, ns, np) = (
            rd16(bytes, 6) as usize,
            rd16(bytes, 8) as usize,
            rd16(bytes, 10) as usize,
        );
        if na > MAX_ACHIEVEMENTS || ns > MAX_STATS {
            return Err(TableError::TooMany);
        }
        let mut at = HEADER;
        let mut take = |len: usize| -> Result<&'a [u8], TableError> {
            let s = bytes.get(at..at + len).ok_or(TableError::Truncated)?;
            at += len;
            Ok(s)
        };
        let achievements = take(na * ACHIEVEMENT_RECORD)?;
        let stats = take(ns * STAT_RECORD)?;
        let presence = take(np * PRESENCE_RECORD)?;
        let pool = core::str::from_utf8(&bytes[at..]).map_err(|_| TableError::BadString)?;
        let string = |r: &[u8], at: usize| {
            let (off, len) = (rd16(r, at) as usize, rd16(r, at + 2) as usize);
            pool.get(off..off + len).ok_or(TableError::BadString)
        };
        for r in achievements.as_chunks::<ACHIEVEMENT_RECORD>().0.iter() {
            string(r, 4)?;
            string(r, 8)?;
            string(r, 12)?;
            if r[21] != NO_STAT && r[21] as usize >= ns {
                return Err(TableError::BadStat);
            }
        }
        for r in stats.as_chunks::<STAT_RECORD>().0.iter() {
            string(r, 4)?;
        }
        for r in presence.as_chunks::<PRESENCE_RECORD>().0.iter() {
            string(r, 4)?;
        }
        Ok(AchievementTable {
            achievements,
            stats,
            presence,
            pool,
        })
    }

    /// The table the platform baked (`None`: no blob, or a broken one).
    pub fn from_platform(platform: &dyn Platform) -> Option<AchievementTable<'static>> {
        platform
            .blob(BLOB)
            .and_then(|b| AchievementTable::parse(b).ok())
    }

    pub fn len(&self) -> usize {
        self.achievements.len() / ACHIEVEMENT_RECORD
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn stat_count(&self) -> usize {
        self.stats.len() / STAT_RECORD
    }

    pub fn presence_count(&self) -> usize {
        self.presence.len() / PRESENCE_RECORD
    }

    fn str_at(&self, r: &[u8], at: usize) -> &'a str {
        let (off, len) = (rd16(r, at) as usize, rd16(r, at + 2) as usize);
        self.pool.get(off..off + len).unwrap_or("")
    }

    /// The achievement at bit `index`.
    pub fn get(&self, index: usize) -> Option<AchievementInfo<'a>> {
        let r = self
            .achievements
            .get(index * ACHIEVEMENT_RECORD..(index + 1) * ACHIEVEMENT_RECORD)?;
        let icon = rd32(r, 16);
        Some(AchievementInfo {
            id: AchievementId(rd32(r, 0)),
            index,
            api: self.str_at(r, 4),
            name: self.str_at(r, 8),
            description: self.str_at(r, 12),
            icon: (icon != 0).then_some(SpriteId(icon)),
            hidden: r[20] & 1 != 0,
            stat: (r[21] != NO_STAT).then_some(r[21] as usize),
            threshold: rd32(r, 24) as i32,
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = AchievementInfo<'a>> + '_ {
        (0..self.len()).filter_map(|i| self.get(i))
    }

    /// The bit of `id`.
    pub fn index(&self, id: AchievementId) -> Option<usize> {
        self.achievements
            .as_chunks::<ACHIEVEMENT_RECORD>()
            .0
            .iter()
            .position(|r| rd32(r, 0) == id.0)
    }

    pub fn find(&self, id: AchievementId) -> Option<AchievementInfo<'a>> {
        self.index(id).and_then(|i| self.get(i))
    }

    pub fn stat(&self, index: usize) -> Option<StatInfo<'a>> {
        let r = self
            .stats
            .get(index * STAT_RECORD..(index + 1) * STAT_RECORD)?;
        Some(StatInfo {
            id: StatId(rd32(r, 0)),
            index,
            api: self.str_at(r, 4),
            min: rd32(r, 8) as i32,
            max: rd32(r, 12) as i32,
            increment_only: rd16(r, 16) & 1 != 0,
        })
    }

    pub fn stats(&self) -> impl Iterator<Item = StatInfo<'a>> + '_ {
        (0..self.stat_count()).filter_map(|i| self.stat(i))
    }

    pub fn stat_index(&self, id: StatId) -> Option<usize> {
        self.stats
            .as_chunks::<STAT_RECORD>()
            .0
            .iter()
            .position(|r| rd32(r, 0) == id.0)
    }

    /// A rich-presence key's text form (`presence.in_match`), the store's
    /// token for it.
    pub fn presence_key(&self, key: StrKey) -> Option<&'a str> {
        self.presence
            .as_chunks::<PRESENCE_RECORD>()
            .0
            .iter()
            .find(|r| rd32(&r[..], 0) == key.0)
            .map(|r| self.str_at(r, 4))
    }

    pub fn presence_keys(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.presence
            .as_chunks::<PRESENCE_RECORD>()
            .0
            .iter()
            .map(|r| self.str_at(r, 4))
    }
}

/// An achievement to bake ([`encode`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AchievementRecord<'a> {
    pub api: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub icon: u32,
    pub hidden: bool,
    /// (stat index, threshold).
    pub stat: Option<(usize, i32)>,
}

/// A stat to bake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StatRecord<'a> {
    pub api: &'a str,
    pub min: i32,
    pub max: i32,
    pub increment_only: bool,
}

/// Bytes [`encode`] writes.
pub fn encoded_len(
    achievements: &[AchievementRecord<'_>],
    stats: &[StatRecord<'_>],
    presence: &[&str],
) -> usize {
    let strings: usize = achievements
        .iter()
        .map(|a| a.api.len() + a.name.len() + a.description.len())
        .sum::<usize>()
        + stats.iter().map(|s| s.api.len()).sum::<usize>()
        + presence.iter().map(|p| p.len()).sum::<usize>();
    HEADER
        + achievements.len() * ACHIEVEMENT_RECORD
        + stats.len() * STAT_RECORD
        + presence.len() * PRESENCE_RECORD
        + strings
}

/// Write a `TACH` blob into `out`; returns its length. Ids are
/// `asset_id(api)` for achievements and stats, `asset_id(key)` for
/// presence keys.
pub fn encode(
    achievements: &[AchievementRecord<'_>],
    stats: &[StatRecord<'_>],
    presence: &[&str],
    out: &mut [u8],
) -> Result<usize, TableError> {
    if achievements.len() > MAX_ACHIEVEMENTS || stats.len() > MAX_STATS {
        return Err(TableError::TooMany);
    }
    let len = encoded_len(achievements, stats, presence);
    if out.len() < len {
        return Err(TableError::NoRoom);
    }
    let out = &mut out[..len];
    out.fill(0);
    out[0..4].copy_from_slice(MAGIC);
    wr16(out, 4, VERSION);
    wr16(out, 6, achievements.len() as u16);
    wr16(out, 8, stats.len() as u16);
    wr16(out, 10, presence.len() as u16);
    let pool_at = HEADER
        + achievements.len() * ACHIEVEMENT_RECORD
        + stats.len() * STAT_RECORD
        + presence.len() * PRESENCE_RECORD;
    let mut pool = pool_at;
    let mut put = |out: &mut [u8], at: usize, s: &str| -> Result<(), TableError> {
        let off = pool - pool_at;
        if off > u16::MAX as usize || s.len() > u16::MAX as usize {
            return Err(TableError::NoRoom);
        }
        out[pool..pool + s.len()].copy_from_slice(s.as_bytes());
        wr16(out, at, off as u16);
        wr16(out, at + 2, s.len() as u16);
        pool += s.len();
        Ok(())
    };
    let mut at = HEADER;
    for a in achievements {
        wr32(out, at, crate::asset::asset_id(a.api));
        put(out, at + 4, a.api)?;
        put(out, at + 8, a.name)?;
        put(out, at + 12, a.description)?;
        wr32(out, at + 16, a.icon);
        out[at + 20] = a.hidden as u8;
        match a.stat {
            Some((i, threshold)) => {
                if i >= stats.len() {
                    return Err(TableError::BadStat);
                }
                out[at + 21] = i as u8;
                wr32(out, at + 24, threshold as u32);
            }
            None => out[at + 21] = NO_STAT,
        }
        at += ACHIEVEMENT_RECORD;
    }
    for s in stats {
        wr32(out, at, crate::asset::asset_id(s.api));
        put(out, at + 4, s.api)?;
        wr32(out, at + 8, s.min as u32);
        wr32(out, at + 12, s.max as u32);
        wr16(out, at + 16, s.increment_only as u16);
        at += STAT_RECORD;
    }
    for p in presence {
        wr32(out, at, crate::asset::asset_id(p));
        put(out, at + 4, p)?;
        at += PRESENCE_RECORD;
    }
    Ok(len)
}

fn rd16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn rd32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn wr16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn wr32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Three achievements (one hidden, one on a stat), two stats, two
    /// presence keys: the fixture every services test uses.
    pub fn fixture() -> Vec<u8> {
        let ach = [
            AchievementRecord {
                api: "FIRST_COIN",
                name: "ach.first_coin.name",
                description: "ach.first_coin.desc",
                icon: crate::asset::asset_id("sprites/coin"),
                hidden: false,
                stat: None,
            },
            AchievementRecord {
                api: "SECRET_ROOM",
                name: "ach.secret.name",
                description: "ach.secret.desc",
                icon: 0,
                hidden: true,
                stat: None,
            },
            AchievementRecord {
                api: "COINS_100",
                name: "ach.coins_100.name",
                description: "ach.coins_100.desc",
                icon: 0,
                hidden: false,
                stat: Some((0, 100)),
            },
        ];
        let stats = [
            StatRecord {
                api: "COINS",
                min: 0,
                max: 9999,
                increment_only: true,
            },
            StatRecord {
                api: "BEST_TIME",
                min: 0,
                max: 1_000_000,
                increment_only: false,
            },
        ];
        let presence = ["presence.menu", "presence.stage"];
        let mut out = vec![0u8; encoded_len(&ach, &stats, &presence)];
        let n = encode(&ach, &stats, &presence, &mut out).unwrap();
        assert_eq!(n, out.len());
        out
    }

    #[test]
    fn round_trip() {
        let blob = fixture();
        let t = AchievementTable::parse(&blob).unwrap();
        assert_eq!((t.len(), t.stat_count(), t.presence_count()), (3, 2, 2));
        let a = t.get(0).unwrap();
        assert_eq!(a.id, AchievementId::new("FIRST_COIN"));
        assert_eq!((a.api, a.name), ("FIRST_COIN", "ach.first_coin.name"));
        assert_eq!(a.icon, Some(SpriteId::from_path("sprites/coin")));
        assert!(!a.hidden && a.stat.is_none());
        assert!(t.get(1).unwrap().hidden);
        let c = t.find(AchievementId::new("COINS_100")).unwrap();
        assert_eq!((c.index, c.stat, c.threshold), (2, Some(0), 100));
        let s = t.stat(0).unwrap();
        assert_eq!(
            (s.api, s.min, s.max, s.increment_only),
            ("COINS", 0, 9999, true)
        );
        assert_eq!(t.stat_index(StatId::new("BEST_TIME")), Some(1));
        assert_eq!(
            t.presence_key(StrKey::new("presence.stage")),
            Some("presence.stage")
        );
        assert_eq!(t.presence_key(StrKey::new("nope")), None);
        assert_eq!(t.index(AchievementId::new("NOPE")), None);
    }

    #[test]
    fn refuses_broken_blobs() {
        let blob = fixture();
        assert_eq!(
            AchievementTable::parse(&blob[..10]).err(),
            Some(TableError::Truncated)
        );
        let mut bad = blob.clone();
        bad[0] = b'X';
        assert_eq!(
            AchievementTable::parse(&bad).err(),
            Some(TableError::BadMagic)
        );
        let mut bad = blob.clone();
        bad[4] = 9;
        assert_eq!(
            AchievementTable::parse(&bad).err(),
            Some(TableError::UnsupportedVersion(9))
        );
        // A threshold on a stat past the end.
        let mut bad = blob.clone();
        bad[HEADER + 2 * ACHIEVEMENT_RECORD + 21] = 7;
        assert_eq!(
            AchievementTable::parse(&bad).err(),
            Some(TableError::BadStat)
        );
        // A string past the pool.
        let mut bad = blob.clone();
        bad[HEADER + 6] = 0xFF;
        assert_eq!(
            AchievementTable::parse(&bad).err(),
            Some(TableError::BadString)
        );
        assert_eq!(
            AchievementTable::parse(&blob[..blob.len() - 1]).err(),
            Some(TableError::BadString)
        );
    }

    #[test]
    fn encode_checks_limits() {
        let a = AchievementRecord {
            api: "A",
            name: "n",
            description: "d",
            icon: 0,
            hidden: false,
            stat: Some((0, 1)),
        };
        let mut out = [0u8; 256];
        assert_eq!(encode(&[a], &[], &[], &mut out), Err(TableError::BadStat));
        let mut small = [0u8; 8];
        assert_eq!(encode(&[], &[], &[], &mut small), Err(TableError::NoRoom));
        assert_eq!(encode(&[], &[], &[], &mut out), Ok(HEADER));
        assert!(AchievementTable::parse(&out[..HEADER]).unwrap().is_empty());
    }
}
