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
        assert_eq!((r.frames, r.checked), (761, 761), "{profile:?}: {r:?}");
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

/// Issue #117: the lockstep step on the PC's CPU, its hash pinned for
/// the 3DS and the browsers.
#[test]
fn the_lockstep_step_ends_on_the_pinned_hash() {
    let r = selftest::lockstep_checks().unwrap();
    println!("{r:?}");
    assert_eq!(r.frames, selftest::LOCKSTEP_FRAMES);
    assert!(r.lost > 0 && r.stalls > 0 && r.checked > 0, "{r:?}");
    assert_eq!(r.hash, selftest::LOCKSTEP_HASH, "pin the new hash");
}

/// What two 3DS consoles in Azahar logged over local wireless (issue #117,
/// `tools/uds-test/two-azahar.sh`): castle64 in lockstep, input delay 4,
/// each console playing its seat of `castle64_green.trec`, frame 600.
const THREE_DS_UDS_600: u64 = 0x94fa_2260_aa5f_5154;

/// The same session on the PC's CPU: two lockstep peers in this process,
/// castle64 at the 3DS's screen, a fixed 1/60 s, the green run's input on
/// seat 1. Frame 600 must be the hash both emulated 3DS consoles printed.
#[test]
fn castle64_in_lockstep_ends_where_the_two_azahar_consoles_did() {
    use trino_core::lockstep::{LockConfig, Lockstep, MAX_PACKET};
    use trino_core::{Audio, Game, HostedPlayers, MusicId, SoundId};
    struct Host(HostedPlayers);
    impl Platform for Host {
        fn now_micros(&self) -> u64 {
            0
        }
        fn log(&self, _: &str) {}
        fn hosted_players(&self) -> Option<&HostedPlayers> {
            Some(&self.0)
        }
        fn hosted_players_mut(&mut self) -> Option<&mut HostedPlayers> {
            Some(&mut self.0)
        }
    }
    struct Mute;
    impl Audio for Mute {
        fn play_sound(&mut self, _: SoundId) {}
        fn play_music(&mut self, _: MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let bytes = std::fs::read(root.join("assets/shared/replays/castle64_green.trec")).unwrap();
    let mut r = trino_core::replay::Reader::new(&bytes).unwrap();
    let mut script = Vec::new();
    while let Some(f) = r.next_frame() {
        script.push(f.unwrap().players);
    }
    let delay = 4u8;
    let mut games: Vec<Box<castle64::Castle64Game>> = (0..2)
        .map(|_| Box::new(castle64::Castle64Game::new(Vec2::new(400.0, 240.0))))
        .collect();
    let mut locks: Vec<Lockstep> = (0..2u8)
        .map(|me| {
            Lockstep::new(LockConfig {
                delay,
                ..LockConfig::new(2, me)
            })
        })
        .collect();
    let mut hosts = [Host(HostedPlayers::new()), Host(HostedPlayers::new())];
    let mut at_600 = [None; 2];
    let mut buf = [0u8; MAX_PACKET];
    for tick in 0..5000u64 {
        if at_600.iter().all(Option::is_some) {
            break;
        }
        for me in 0..2 {
            let f = (locks[me].frame() + delay as u32) as usize;
            let local = script
                .get(f)
                .map_or(trino_core::InputState::IDLE, |p| p.players[me]);
            locks[me].add_local(&local);
            if let Some(p) = locks[me].advance() {
                hosts[me].0.publish_net(&p, locks[me].local_seats());
                let mut neutral = trino_core::replay::Neutral(&mut hosts[me]);
                games[me].update_with(&mut neutral, &p.players[0], &mut Mute, 1.0 / 60.0);
                let h = games[me].state_hash().unwrap();
                locks[me].note_hash(h);
                if locks[me].frame() == 600 {
                    at_600[me] = Some(h);
                }
            }
            let n = locks[me].packet(1 - me as u8, &mut buf);
            locks[1 - me].receive(&buf[..n], tick * 16).unwrap();
        }
    }
    assert_eq!(at_600, [Some(THREE_DS_UDS_600); 2], "{at_600:x?}");
}
