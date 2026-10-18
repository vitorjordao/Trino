//! `castle64::selftest::asset_groups` on the PC backend (issue #56): the
//! check ares and Azahar run, through `trino_runner_pc::GroupLoader` and the
//! counting allocator a PC host reports as its heap (the runner installs it). One test (the heap is
//! the process's): every console profile and tier, then the 10 000-frame
//! load/unload soak. GPU-gated like the goldens (`TRINO_REQUIRE_GPU=1`).

use std::path::PathBuf;

use castle64::selftest::{self, GroupHost};
use trino_core::{HwTier, Platform, Renderer};
use trino_platform_pc::{PcAudio, PcPlatform, PcRenderer, SimProfile};
use trino_runner_pc::GroupLoader;

struct PcHost {
    renderer: PcRenderer,
    audio: PcAudio,
    platform: PcPlatform,
    loader: GroupLoader,
}

impl GroupHost for PcHost {
    fn parts(&mut self) -> (&mut dyn Renderer, &mut dyn Platform) {
        (&mut self.renderer, &mut self.platform)
    }

    fn pump(&mut self) {
        self.loader
            .pump(&mut self.renderer, &mut self.audio, &mut self.platform);
    }

    /// wgpu's resource trackers grow in steps as frames go by, loads or
    /// not (~3 KB over 3 000 empty frames, measured): a leak per cycle
    /// would be the level's bytes times the cycles, megabytes here.
    fn heap_tolerance(&self) -> u32 {
        64 * 1024
    }
}

/// A strict renderer with the boot assets, a platform reporting the
/// runner's counting allocator (`trino_runner_pc::HEAP`),
/// and the bake's groups behind a loader. `None` (skip) without a GPU.
fn pc_host(profile: SimProfile, tier: HwTier, baked: &std::path::Path) -> Option<PcHost> {
    let mut renderer = match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return None;
        }
    };
    renderer.set_strict(true);
    renderer.set_hw_tier(tier);
    let mut audio = PcAudio::new_offline(48_000);
    let mut platform = PcPlatform::new(&profile.caps().with_tier(tier));
    platform.set_heap(trino_runner_pc::heap().expect("runner-pc counts the heap"));
    trino_runner_pc::upload_assets(baked, &mut renderer, &mut audio, &mut platform, None).unwrap();
    let loader = GroupLoader::new(baked, &renderer, &mut platform);
    Some(PcHost {
        renderer,
        audio,
        platform,
        loader,
    })
}

#[test]
fn asset_groups_load_draw_and_unload_on_pc() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let profiles = [
        (SimProfile::N64, HwTier::Base),
        (SimProfile::N64, HwTier::Expanded),
        (SimProfile::N3ds, HwTier::Base),
        (SimProfile::N3ds, HwTier::Expanded),
        (SimProfile::Native, HwTier::Base),
    ];
    for (profile, tier) in profiles {
        let Some(mut host) = pc_host(profile, tier, baked.path()) else {
            return;
        };
        let names = host.loader.group_names();
        assert!(names.contains(&"selftest_level"), "{names:?}");
        let r = selftest::asset_groups(&mut host, 120)
            .unwrap_or_else(|e| panic!("{profile:?} {tier:?}: {e}"));
        assert!(
            r.cycles > 0 && r.level_bytes > 0,
            "{profile:?} {tier:?}: {r:?}"
        );
        assert!(r.heap_baseline > 0, "the counting allocator reports a heap");
        // The bar animates: a load takes at least the frame it was asked in.
        let m = host.platform.memory();
        assert_eq!(m.groups_loading, 0);
        assert_eq!(m.groups_resident, 1, "boot only after the check");
        println!("{profile:?} {tier:?}: {r:?}");
    }
    // Phase 4's soak: 10 000 frames of load/unload cycles.
    let Some(mut host) = pc_host(SimProfile::N64, HwTier::Base, baked.path()) else {
        return;
    };
    let r = selftest::asset_groups(&mut host, 10_000).unwrap();
    assert_eq!(r.soak_frames, 10_000);
    assert!(r.cycles >= 1_000, "{r:?}");
    println!("soak: {r:?}");
}
