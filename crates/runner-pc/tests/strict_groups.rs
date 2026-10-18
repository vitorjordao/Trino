//! Strict mode on asset groups (issue #56, ADR 0022 §6): a PC host on a
//! console profile checks every group load against that console's RAM
//! (`trino_asset_pipeline::memory`) and stops the one that would not fit,
//! naming the groups and the biggest asset; the Expansion Pak's tier takes
//! it. GPU-gated like the goldens (`TRINO_REQUIRE_GPU=1`).

#[path = "../../platform-pc/tests/common/mod.rs"]
mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use trino_asset_pipeline::{Platform as Target, memory};
use trino_core::{GroupId, HwTier, LoadState, Platform};
use trino_platform_pc::{PcAudio, PcPlatform, SimProfile};
use trino_runner_pc::{GroupLoader, upload_assets};

/// A 3 MB blob in `huge` (past the N64's 1.8 MB for assets, inside the
/// Expansion Pak's 5.8 MB) and a few bytes in `level`.
fn project(assets: &std::path::Path) {
    std::fs::create_dir_all(assets.join("shared/data")).unwrap();
    std::fs::write(assets.join("shared/data/huge.bin"), vec![7u8; 3 << 20]).unwrap();
    std::fs::write(assets.join("shared/data/small.txt"), b"small").unwrap();
    std::fs::write(
        assets.join("manifest.toml"),
        "version = 1\n\n[groups.huge]\n[groups.level]\n\n\
         [blobs.huge]\nfile = \"data/huge.bin\"\ngroups = [\"huge\"]\n\n\
         [blobs.small]\nfile = \"data/small.txt\"\ngroups = [\"level\"]\n",
    )
    .unwrap();
}

#[test]
fn strict_mode_stops_a_group_past_the_consoles_ram() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    let baked = dir.path().join("baked");
    project(&assets);
    trino_asset_pipeline::bake_all(&assets, Target::Pc, &baked).unwrap_or_else(|e| panic!("{e}"));
    for (tier, fits) in [(HwTier::Base, false), (HwTier::Expanded, true)] {
        let Some(mut r) = common::headless(SimProfile::N64) else {
            return;
        };
        r.set_hw_tier(tier);
        let mut audio = PcAudio::new_offline(48_000);
        let mut platform = PcPlatform::new(&SimProfile::N64.caps().with_tier(tier));
        upload_assets(&baked, &mut r, &mut audio, &mut platform, None).unwrap();
        let plan = memory::plan(&assets, Target::N64, tier).unwrap();
        let mut loader = GroupLoader::new(&baked, &r, &mut platform).with_plan(plan);

        // A small group fits either tier.
        let level = GroupId::from_name("level");
        platform.load_group(level);
        for _ in 0..10 {
            loader.pump(&mut r, &mut audio, &mut platform);
        }
        assert_eq!(platform.group_state(level), LoadState::Ready, "{tier:?}");

        let huge = GroupId::from_name("huge");
        platform.load_group(huge);
        let run = catch_unwind(AssertUnwindSafe(|| {
            for _ in 0..50 {
                loader.pump(&mut r, &mut audio, &mut platform);
            }
        }));
        let message = |p: Box<dyn std::any::Any + Send>| {
            p.downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default()
        };
        match (run, fits) {
            (Ok(()), true) => assert_eq!(platform.group_state(huge), LoadState::Ready),
            (Err(p), false) => {
                let msg = message(p);
                assert!(msg.starts_with("strict mode: "), "{msg}");
                assert!(msg.contains("N64 for assets"), "{msg}");
                assert!(msg.contains("`blobs/huge` 3.0 MB (huge)"), "{msg}");
            }
            (Ok(()), false) => panic!("the base N64 took a 3 MB group"),
            (Err(p), true) => panic!("the Expansion Pak refused it: {}", message(p)),
        }
    }
}
