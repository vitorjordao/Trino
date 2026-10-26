//! The `players` and `replay` self-test steps on the PC backend (issue
//! #67): the code ares, Azahar and the browsers run. The replay plays the
//! committed PC recording of castle64's bot from its asset group and must
//! match every recorded state hash; a recording tampered with on frame 100
//! must not. GPU-gated like the goldens (`TRINO_REQUIRE_GPU=1`).

use std::path::PathBuf;

use castle64::selftest::{self, GroupHost};
use trino_core::{Platform, Renderer, Vec2};
use trino_platform_pc::{PcAudio, PcInput, PcPlatform, PcRenderer, SimProfile};
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
        self.renderer.wait_gpu();
        self.loader
            .pump(&mut self.renderer, &mut self.audio, &mut self.platform);
    }
}

fn pc_host(profile: SimProfile, baked: &std::path::Path) -> Option<PcHost> {
    let mut renderer = match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return None;
        }
    };
    renderer.set_strict(true);
    let mut audio = PcAudio::new_offline(48_000);
    let mut platform = PcPlatform::new(&profile.caps());
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
fn the_pc_recording_replays_hash_for_hash_and_a_tampered_one_does_not() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    for (profile, screen) in [
        (SimProfile::N64, Vec2::new(320.0, 240.0)),
        (SimProfile::N3ds, Vec2::new(400.0, 240.0)),
    ] {
        let Some(mut host) = pc_host(profile, baked.path()) else {
            return;
        };
        let mut game = castle64::Castle64Game::new(screen);
        let mut again = Vec::new();
        let r = selftest::replay(
            &mut host,
            &mut game,
            false,
            Some(&mut |b: &[u8]| again.extend_from_slice(b)),
        )
        .unwrap_or_else(|e| panic!("{profile:?}: {e}"));
        assert_eq!((r.frames, r.checked), (731, 731), "{profile:?}: {r:?}");
        // What this target fed and hashed, recorded again: the PC's file,
        // byte for byte.
        let fixture =
            std::fs::read(root.join("assets/shared/replays/castle64_green.trec")).unwrap();
        assert!(
            again == fixture,
            "{profile:?}: the re-recording differs from the fixture"
        );
        selftest::log_replay(&host.platform, &r);
        assert_eq!(
            host.platform.group_state(selftest::REPLAY_GROUP),
            trino_core::LoadState::Unloaded,
            "the step unloads its group"
        );
        // The mutation the step exists to catch.
        let e = selftest::replay(&mut host, &mut game, true, None).unwrap_err();
        assert!(e.contains("diverged"), "{e}");
    }
}

#[test]
fn the_players_step_passes_on_the_pc_input() {
    let mut input = PcInput::new();
    let r = selftest::players_checks(&mut input).unwrap();
    // The keyboard is always there: player 1.
    assert_eq!(r.connected, 1, "{r:?}");
}
