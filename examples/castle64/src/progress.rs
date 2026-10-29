//! castle64's save (issue #121): the stars, the coins taken in each level,
//! the boars stomped and the achievements, as one versioned save
//! (`trino_core::save::write_versioned`, read by `save::load_game` with the
//! game's `migrate_save`). The consoles keep achievements only in the
//! game's save (`AchievementState::pack_for` / `restore_achievements`); the
//! PC and the Web keep their own too, and the restore merges.
//!
//! Version 1 is the first: castle64 saved nothing before it. A medium that
//! holds something untagged (a self-test's `trino-save-v1`, another game's
//! save on a shared medium) is version 0, which has nothing to migrate: the
//! game starts from the beginning and its next save replaces it.

use trino_core::{AchievementState, AchievementTable};

use crate::levels::LEVELS;

/// The save's version tag.
pub const VERSION: u16 = 1;

/// Largest save: the progress, the achievements' pack, the version tag.
pub const MAX: usize = 1
    + 4 * LEVELS.len()
    + 4
    + AchievementState::packed_len(
        trino_core::services::MAX_ACHIEVEMENTS,
        trino_core::services::MAX_STATS,
    )
    + trino_core::save::VERSION_HEADER;

/// What a castle64 save holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Progress {
    /// One bit per level's star.
    pub stars: u8,
    /// One bit per coin taken, per level.
    pub coins: [u32; LEVELS.len()],
    pub stomps: u32,
}

impl Progress {
    /// The progress bytes then the achievements' pack: its length.
    pub fn encode(
        &self,
        table: &AchievementTable<'_>,
        achievements: &AchievementState,
        out: &mut [u8],
    ) -> Option<usize> {
        let head = 1 + 4 * LEVELS.len() + 4;
        let out_head = out.get_mut(..head)?;
        out_head[0] = self.stars;
        for (i, c) in self.coins.iter().enumerate() {
            out_head[1 + 4 * i..5 + 4 * i].copy_from_slice(&c.to_le_bytes());
        }
        out_head[head - 4..].copy_from_slice(&self.stomps.to_le_bytes());
        let n = achievements.pack_for(table, out.get_mut(head..)?);
        Some(head + n)
    }

    /// Read [`encode`](Self::encode)'s bytes (`None`: too short). The
    /// achievements are `None` when the save holds none or a pack this
    /// build can't read.
    pub fn decode(bytes: &[u8]) -> Option<(Progress, Option<AchievementState>)> {
        let head = 1 + 4 * LEVELS.len() + 4;
        let b = bytes.get(..head)?;
        let mut coins = [0u32; LEVELS.len()];
        for (i, c) in coins.iter_mut().enumerate() {
            *c = u32::from_le_bytes(b[1 + 4 * i..5 + 4 * i].try_into().ok()?);
        }
        let p = Progress {
            stars: b[0],
            coins,
            stomps: u32::from_le_bytes(b[head - 4..].try_into().ok()?),
        };
        let ach = AchievementState::unpack(&bytes[head..])
            .ok()
            .map(|(s, _)| s);
        Some((p, ach))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_round_trips_with_its_achievements() {
        let mut coins = [0u32; LEVELS.len()];
        coins[1] = 0b1011;
        coins[LEVELS.len() - 1] = u32::MAX;
        let p = Progress {
            stars: 0b101,
            coins,
            stomps: 7,
        };
        let mut out = [0u8; MAX];
        let n = p
            .encode(&AchievementTable::EMPTY, &AchievementState::EMPTY, &mut out)
            .unwrap();
        // No table: no achievements pack.
        assert_eq!(n, 1 + 4 * LEVELS.len() + 4);
        assert_eq!(Progress::decode(&out[..n]), Some((p, None)));
        assert_eq!(Progress::decode(&out[..n - 1]), None);
        assert!(n + trino_core::save::VERSION_HEADER <= MAX);
    }
}
