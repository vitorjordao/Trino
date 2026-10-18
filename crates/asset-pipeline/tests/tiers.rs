//! Hardware-tier twins (issue #54, ADR 0020) from the repo's own self-test
//! content: a console bake carries a `base` and an `expanded` copy of what
//! resolves differently on its expanded tier, one copy of everything else.

use std::path::PathBuf;

use trino_asset_pipeline::{Platform, bake_scene_for, particles, tier_twins};
use trino_core::{BakedScene, Effect, HwTier, ModelId};

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

fn leak(v: Vec<u8>) -> &'static [u8] {
    Box::leak(v.into_boxed_slice())
}

#[test]
fn the_tier_probe_bakes_a_twin_per_console_tier() {
    let card = ModelId::from_path("models/selftest_card");
    let tier_card = ModelId::from_path("models/selftest_tier_card");
    for platform in [Platform::N64, Platform::N3ds] {
        let scene = tier_twins(platform, |t| {
            bake_scene_for(&assets(), "scenes/selftest-tier.scene.ron", t)
        })
        .unwrap();
        let tiers: Vec<_> = scene.iter().map(|b| b.tier).collect();
        assert_eq!(
            tiers,
            [Some(HwTier::Base), Some(HwTier::Expanded)],
            "{platform:?}"
        );
        let probe = |i: usize| {
            BakedScene::parse(leak(scene[i].bytes.clone()))
                .unwrap()
                .find("probe")
                .unwrap()
                .model
        };
        assert_eq!((probe(0), probe(1)), (Some(card), Some(tier_card)));

        let src = assets().join("shared/particles/selftest-tier-sparks.particles.ron");
        let sparks = tier_twins(platform, |t| particles::bake(&src, t)).unwrap();
        let caps: Vec<u16> = sparks
            .iter()
            .map(|b| Effect::parse(leak(b.bytes.clone())).unwrap().max)
            .collect();
        assert_eq!(caps, [4, 6], "{platform:?}");
    }
    // The PC and the Web have one tier: one untagged copy.
    for platform in [Platform::Pc, Platform::Web] {
        let one = tier_twins(platform, |t| {
            bake_scene_for(&assets(), "scenes/selftest-tier.scene.ron", t)
        })
        .unwrap();
        assert_eq!((one.len(), one[0].tier), (1, None), "{platform:?}");
    }
}

/// A scene without tier variants bakes byte for byte what it did before
/// the tiers: one copy, no index column.
#[test]
fn content_without_tier_variants_bakes_one_copy() {
    for platform in [Platform::N64, Platform::N3ds] {
        let one = tier_twins(platform, |t| {
            bake_scene_for(&assets(), "scenes/interior.scene.ron", t)
        })
        .unwrap();
        assert_eq!((one.len(), one[0].tier), (1, None));
        assert_eq!(
            one[0].bytes,
            bake_scene_for(&assets(), "scenes/interior.scene.ron", platform).unwrap()
        );
        let id = trino_core::asset_id("scenes/interior");
        assert_eq!(
            one[0].index_line(id, "blob", "blob"),
            format!("{id:08x}\tblob\t{id:08x}.blob\n")
        );
    }
}
