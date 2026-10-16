//! `castle64::selftest::engine_checks` on the PC backend — the same checks
//! the N64/3DS self-tests run inside ares/Azahar. GPU-gated like the
//! goldens (`TRINO_REQUIRE_GPU=1` turns a missing adapter into a failure).

use std::path::PathBuf;

use trino_platform_pc::{PcPlatform, PcRenderer, SimProfile};

/// A strict PC renderer with every baked asset, and a platform serving the
/// blobs and a save file. `None` (skip) without a GPU.
fn pc_target(profile: SimProfile, save_dir: &std::path::Path) -> Option<(PcRenderer, PcPlatform)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let mut renderer = match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return None;
        }
    };
    renderer.set_strict(true);
    let mut platform =
        PcPlatform::new(&profile.caps()).with_save_file(save_dir.join("selftest.sav"));
    for s in assets.sprites {
        renderer.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for f in assets.fonts {
        renderer.upload_font(trino_core::FontId(f.id), f.table);
    }
    for m in assets.models {
        renderer.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    for b in assets.blobs {
        platform.insert_blob(trino_core::BlobId(b.id), b.bytes);
    }
    Some((renderer, platform))
}

/// Every check on the N64 profile (one screen: `dual_screen` collapses).
#[test]
fn engine_checks_pass_on_pc() {
    let save_dir = tempfile::tempdir().unwrap();
    let Some((mut renderer, mut platform)) = pc_target(SimProfile::N64, save_dir.path()) else {
        return;
    };
    castle64::selftest::engine_checks(&mut renderer, &mut platform).unwrap();
    assert_eq!(
        castle64::selftest::dual_screen(&mut renderer),
        Ok(false),
        "single screen"
    );
}

/// Every check on the 3DS profile, whose simulated second screen the
/// `dual_screen` step probes (3D and 2D on both screens, read back).
#[test]
fn engine_checks_pass_on_the_pc_3ds_profile() {
    let save_dir = tempfile::tempdir().unwrap();
    let Some((mut renderer, mut platform)) = pc_target(SimProfile::N3ds, save_dir.path()) else {
        return;
    };
    castle64::selftest::engine_checks(&mut renderer, &mut platform).unwrap();
    assert_eq!(
        castle64::selftest::dual_screen(&mut renderer),
        Ok(true),
        "both screens probed"
    );
}

/// `[scenes.showcase]` baked by the PC pipeline, served by `PcPlatform`,
/// parsed by `BakedScene`: identical to the exported code, and playable
/// (the same `check_baked` the console self-tests run). No GPU needed.
#[test]
fn baked_showcase_matches_code_and_plays() {
    use trino_core::{Audio, Game};
    struct NullAudio;
    impl Audio for NullAudio {
        fn play_sound(&mut self, _: trino_core::SoundId) {}
        fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let mut platform = PcPlatform::new(&SimProfile::N64.caps());
    for b in assets.blobs {
        platform.insert_blob(trino_core::BlobId(b.id), b.bytes);
    }
    showcase::check_baked(&platform, |g, input| {
        g.update(input, &mut NullAudio, 1.0 / 60.0)
    })
    .unwrap();
}

/// The PC input backend with nothing pressed — the console self-tests run
/// the same check against their shims.
#[test]
fn idle_input_checks_pass_on_pc() {
    let mut input = trino_platform_pc::PcInput::new();
    castle64::selftest::idle_input_checks(&mut input).unwrap();
}

/// Audio v2 on the PC backend (silent without a device, same voice
/// bookkeeping) — the console self-tests run the same check.
#[test]
fn audio_checks_pass_on_pc() {
    use trino_platform_pc::PcAudio;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let mut audio = PcAudio::new();
    for s in assets.sounds {
        audio.upload_sound(trino_core::SoundId(s.id), s.samples);
    }
    for m in assets.music {
        audio.upload_music(trino_core::MusicId(m.id), m.samples);
    }
    castle64::selftest::audio_checks(&mut audio).unwrap();
}
