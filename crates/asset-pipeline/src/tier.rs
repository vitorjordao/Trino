//! Hardware-tier copies of a console's resolved data (issue #54, ADR 0020).
//!
//! A scene or a particle effect resolves per target, and the N64 with the
//! Expansion Pak (`n64_expanded`) or the New 3DS (`n3ds_new`) may resolve
//! differently from the base console. One ROM / app runs on both, so the
//! console bake keeps both copies when they differ: the index.tsv's 4th
//! column (`base` / `expanded`) tells the runtime which line its hardware
//! loads ([`trino_core::asset::loads_on`]). Identical copies stay one line
//! with no column, so a project without tier variants bakes byte for byte
//! what it did before.

use trino_core::HwTier;

use crate::Platform;

/// One copy of a tier-resolved blob: `tier` None loads on every tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierBlob {
    pub tier: Option<HwTier>,
    pub bytes: Vec<u8>,
}

impl TierBlob {
    /// The blob's file name for asset `id` (`<id>.<ext>`; the expanded
    /// twin `<id>.x.<ext>`).
    pub fn file(&self, id: u32, ext: &str) -> String {
        match self.tier {
            Some(HwTier::Expanded) => format!("{id:08x}.x.{ext}"),
            _ => format!("{id:08x}.{ext}"),
        }
    }

    /// The blob's index.tsv line (`id kind file [tier]`).
    pub fn index_line(&self, id: u32, kind: &str, ext: &str) -> String {
        index_line(id, kind, &self.file(id, ext), self.tier)
    }
}

/// An index.tsv line: `id kind file`, plus the tier column when the line
/// loads on one tier only.
pub fn index_line(id: u32, kind: &str, file: &str, tier: Option<HwTier>) -> String {
    match tier {
        Some(t) => format!("{id:08x}\t{kind}\t{file}\t{}\n", t.key()),
        None => format!("{id:08x}\t{kind}\t{file}\n"),
    }
}

/// Bake one blob for each hardware tier `platform` holds data for
/// ([`Platform::tiers`]): a single tier-less copy when they resolve the
/// same bytes, a `base` and an `expanded` twin when they do not.
pub fn tier_twins(
    platform: Platform,
    mut bake: impl FnMut(trino_scene::Target) -> Result<Vec<u8>, String>,
) -> Result<Vec<TierBlob>, String> {
    let base = bake(platform.target_on(HwTier::Base))?;
    if !platform.tiers().contains(&HwTier::Expanded) {
        return Ok(vec![TierBlob {
            tier: None,
            bytes: base,
        }]);
    }
    let expanded = bake(platform.target_on(HwTier::Expanded))?;
    if expanded == base {
        return Ok(vec![TierBlob {
            tier: None,
            bytes: base,
        }]);
    }
    Ok(vec![
        TierBlob {
            tier: Some(HwTier::Base),
            bytes: base,
        },
        TierBlob {
            tier: Some(HwTier::Expanded),
            bytes: expanded,
        },
    ])
}

/// The tier column of a model's index line on a console: None when the
/// base console loads it (the expanded tier falls back to it), `expanded`
/// for a model only the expanded tier names, no line at all when neither
/// bakes it.
pub fn model_tier(decl: &crate::manifest::ModelDecl, platform: Platform) -> Option<Option<HwTier>> {
    if decl.bakes_for(platform) {
        Some(None)
    } else if platform.tiers().contains(&HwTier::Expanded)
        && decl.bakes_for(platform.target_on(HwTier::Expanded))
    {
        Some(Some(HwTier::Expanded))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_scene::Target;

    #[test]
    fn identical_tiers_bake_one_untagged_copy() {
        let same = tier_twins(Platform::N64, |_| Ok(vec![1, 2, 3])).unwrap();
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].tier, None);
        assert_eq!(
            same[0].index_line(0xab, "blob", "blob"),
            "000000ab\tblob\t000000ab.blob\n"
        );
    }

    #[test]
    fn differing_tiers_bake_a_base_and_an_expanded_twin() {
        let twins = tier_twins(Platform::N3ds, |t| {
            Ok(vec![if t == Target::N3dsNew { 2 } else { 1 }])
        })
        .unwrap();
        assert_eq!(twins.len(), 2);
        assert_eq!(twins[0].bytes, [1]);
        assert_eq!(
            twins[0].index_line(0xab, "blob", "blob"),
            "000000ab\tblob\t000000ab.blob\tbase\n"
        );
        assert_eq!(twins[1].bytes, [2]);
        assert_eq!(
            twins[1].index_line(0xab, "blob", "blob"),
            "000000ab\tblob\t000000ab.x.blob\texpanded\n"
        );
        // Each tier loads exactly one of them.
        for tier in HwTier::ALL {
            let loads = twins
                .iter()
                .filter(|b| trino_core::asset::loads_on(b.tier.map(HwTier::key), tier))
                .count();
            assert_eq!(loads, 1, "{tier:?}");
        }
    }

    #[test]
    fn the_pc_bakes_its_one_tier() {
        let mut asked = Vec::new();
        let one = tier_twins(Platform::Pc, |t| {
            asked.push(t);
            Ok(vec![0])
        })
        .unwrap();
        assert_eq!((one.len(), asked), (1, vec![Target::Pc]));
    }

    #[test]
    fn a_tier_only_model_gets_the_expanded_column() {
        let decl = |platforms: Option<&[&str]>| -> crate::manifest::ModelDecl {
            let list = platforms.map_or(String::new(), |ps| format!("platforms = {ps:?}"));
            toml::from_str(&format!("file = \"m.glb\"\n{list}")).unwrap()
        };
        assert_eq!(model_tier(&decl(None), Platform::N64), Some(None));
        assert_eq!(model_tier(&decl(Some(&["n64"])), Platform::N64), Some(None));
        assert_eq!(
            model_tier(&decl(Some(&["n64_expanded"])), Platform::N64),
            Some(Some(HwTier::Expanded))
        );
        assert_eq!(
            model_tier(&decl(Some(&["n64_expanded"])), Platform::N3ds),
            None
        );
        assert_eq!(
            model_tier(&decl(Some(&["n3ds_new"])), Platform::N3ds),
            Some(Some(HwTier::Expanded))
        );
        assert_eq!(model_tier(&decl(Some(&["pc"])), Platform::N64), None);
    }
}
