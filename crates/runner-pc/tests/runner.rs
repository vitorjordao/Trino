//! The ready-made PC host's building blocks, without a window: project
//! paths, window/picture sizing, the resampler, the profile knob and the
//! bake → `upload_assets` → draw path a standalone project runs at start.
//! (`run` itself is the winit event loop; the standalone smoke test in
//! `xtask/tests/standalone.rs` drives it under a display.)

// The PC backend's synthetic fixture project (manifest + generated masters).
#[path = "../../platform-pc/tests/common/mod.rs"]
mod common;

use std::path::Path;

use trino_core::{
    Audio, BlobId, Color, DisplaySettings, Material, ModelParams, Platform, Rect, Renderer,
    Shading, SoundId, SpriteParams, TextParams, Transform3, Vec2,
};
use trino_platform_pc::{PcAudio, PcPlatform, SimProfile};
use trino_runner_pc::{
    RunConfig, picture_for, profile_from_env, resample, upload_assets, window_size_for,
};

#[test]
fn run_config_lays_out_the_project() {
    let mut c = RunConfig::new("Demo", "/work/demo");
    assert_eq!(c.title, "Demo");
    assert_eq!(c.assets_dir(), Path::new("/work/demo/assets"));
    assert_eq!(c.baked_dir(), Path::new("/work/demo/target/assets/pc"));
    assert_eq!(c.save_file(), Path::new("/work/demo/target/save/game.sav"));
    assert_eq!(
        c.display_file(),
        Path::new("/work/demo/target/save/game.display")
    );
    c.save_name = "slot2".into();
    assert_eq!(c.save_file(), Path::new("/work/demo/target/save/slot2.sav"));
    assert_eq!(
        c.display_file(),
        Path::new("/work/demo/target/save/slot2.display")
    );
}

#[test]
fn window_and_picture_sizes_follow_settings_and_profiles() {
    let saved = DisplaySettings {
        fullscreen: false,
        window_w: 1024,
        window_h: 768,
        ..DisplaySettings::DEFAULT
    };
    assert_eq!(
        window_size_for(&saved, (1920, 1080)),
        (1024, 768),
        "saved size fits"
    );
    // Too big for the monitor, too small, or unset: a 16:9 default that
    // leaves room for the desktop.
    assert_eq!(window_size_for(&saved, (800, 600)), (720, 405));
    let tiny = DisplaySettings {
        window_w: 100,
        window_h: 100,
        ..saved
    };
    assert_eq!(window_size_for(&tiny, (1920, 1080)), (1280, 720));
    assert_eq!(
        window_size_for(&DisplaySettings::DEFAULT, (1920, 1080)),
        (1280, 720)
    );
    // A monitor smaller than the floor is treated as 640x360.
    assert_eq!(
        window_size_for(&DisplaySettings::DEFAULT, (100, 100)),
        (576, 324)
    );

    // Console profiles keep their fixed picture; native scales.
    assert_eq!(picture_for(SimProfile::N64, (1920, 1080), 3), (320, 240, 0));
    assert_eq!(
        picture_for(SimProfile::N3ds, (1920, 1080), 0),
        (400, 240, 0)
    );
    assert_eq!(
        picture_for(SimProfile::Native, (1920, 1080), 0),
        (640, 360, 3)
    );
    assert_eq!(
        picture_for(SimProfile::Native, (1920, 1080), 1),
        (1920, 1080, 1)
    );
}

#[test]
fn resampler_preserves_level_and_duration() {
    assert!(resample(&[], 22_050, 44_100).is_empty());
    let same = [0.1, 0.2, 0.3];
    assert_eq!(resample(&same, 48_000, 48_000), same);
    let ramp: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
    let up = resample(&ramp, 22_050, 44_100);
    assert_eq!(up.len(), 200);
    assert!(
        (up[1] - 0.005).abs() < 1e-6,
        "midpoint interpolated: {}",
        up[1]
    );
    assert!(
        up.windows(2).all(|w| w[1] >= w[0]),
        "a ramp stays monotonic"
    );
    let down = resample(&ramp, 48_000, 16_000);
    assert_eq!(down.len(), 33);
    assert!((down[10] - 0.30).abs() < 1e-6);
}

#[test]
fn profile_knob_reads_trino_sim() {
    // Read-only: tests never mutate the environment.
    let expected = std::env::var("TRINO_SIM")
        .ok()
        .and_then(|v| SimProfile::parse(&v))
        .unwrap_or(SimProfile::N64);
    assert_eq!(profile_from_env(), expected);
    let tier = std::env::var("TRINO_SIM")
        .ok()
        .and_then(|v| SimProfile::parse_tier(&v))
        .map_or(trino_core::HwTier::Base, |(_, t)| t);
    assert_eq!(trino_runner_pc::tier_from_env(), tier);
}

/// What a standalone host does at start: bake the project, upload every
/// asset kind (sounds resampled to the device rate), then draw with them.
#[test]
fn bake_upload_and_draw_like_the_runner() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("assets");
    let baked = dir.path().join("baked");
    common::write_project(&root);
    trino_asset_pipeline::bake_all(&root, trino_asset_pipeline::Platform::Pc, &baked)
        .unwrap_or_else(|e| panic!("{e}"));

    let mut audio = PcAudio::new();
    let mut platform = PcPlatform::new(&SimProfile::Native.caps());
    let Some(mut r) = common::headless(SimProfile::Native) else {
        return;
    };
    // A missing bake is an error, not a panic.
    let err = upload_assets(
        &dir.path().join("nope"),
        &mut r,
        &mut audio,
        &mut platform,
        None,
    )
    .unwrap_err();
    assert!(err.contains("index.toml"), "{err}");

    // Only the blob first (a live reload of one handle)...
    let blob = BlobId::from_path(common::BLOB);
    upload_assets(&baked, &mut r, &mut audio, &mut platform, Some(&[blob.0])).unwrap();
    assert_eq!(platform.blob(blob), Some(common::BLOB_BYTES));
    r.begin_frame(Color::BLACK);
    r.draw_sprite(
        common::sprite(common::RED),
        Vec2::ZERO,
        &SpriteParams::DEFAULT,
    );
    r.end_frame();
    assert_eq!(r.stats().sprites, 0, "sprites not uploaded yet");

    // ...then everything.
    upload_assets(&baked, &mut r, &mut audio, &mut platform, None).unwrap();
    // The 0.1 s, 22 050 Hz beep is resampled to the device rate.
    let v = audio
        .play_voice(SoundId::from_path(common::BEEP), &Default::default(), true)
        .expect("the sound was uploaded");
    assert!(audio.voice_playing(v));
    audio.stop_voice(v);

    r.begin_frame(Color::BLACK);
    r.draw_sprite(
        common::sprite(common::RED),
        Vec2::new(10.0, 10.0),
        &SpriteParams::DEFAULT,
    );
    r.fill_rect(Rect::new(100.0, 10.0, 8.0, 8.0), Color::rgb(0, 0, 255));
    r.draw_text(
        common::font(),
        Vec2::new(200.0, 10.0),
        "A",
        &TextParams::tinted(Color::rgb(0, 255, 0)),
    );
    r.set_camera(&Default::default());
    r.draw_model(
        common::model(common::BOX),
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams {
            shading: Shading::Unlit,
            ..ModelParams::tinted(Color::rgb(255, 255, 0))
        },
    );
    r.end_frame();
    let frame = r.read_offscreen();
    let at = |x, y| common::px(&frame, 640, x, y);
    common::assert_near(at(14, 14), [255, 0, 0], 8, "sprite");
    common::assert_near(at(104, 14), [0, 0, 255], 8, "rect");
    common::assert_near(at(201, 13), [0, 255, 0], 8, "text");
    common::assert_near(at(320, 240), [255, 255, 0], 8, "model");
    assert_eq!(r.stats().models, 1);
}
