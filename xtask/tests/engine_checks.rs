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
    // The 3DS profile samples each sprite's 3DS format, like Azahar.
    let target = match profile {
        SimProfile::N3ds => trino_asset_pipeline::Platform::N3ds,
        _ => trino_asset_pipeline::Platform::Pc,
    };
    for s in assets.sprites {
        let px = s.pixels_for(target);
        renderer.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, px);
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

/// The `instancing` step on the native profile, where the PC counts the
/// copies and the GPU T&L split (issue #51, Phase 1: decided and reported,
/// drawn by the CPU path): one call is pixel- and work-identical to the
/// separate draws, every copy is counted as instanced, and every copy is
/// one the GPU path takes. The console profiles never take it.
#[test]
fn instancing_matches_separate_draws_and_counts_the_pc_gpu_split() {
    use castle64::selftest::{INSTANCES, instancing};
    use trino_core::Renderer;
    let save_dir = tempfile::tempdir().unwrap();
    for profile in [SimProfile::Native, SimProfile::N64, SimProfile::N3ds] {
        let Some((mut renderer, _)) = pc_target(profile, save_dir.path()) else {
            return;
        };
        assert_eq!(instancing(&mut renderer), Ok(true), "{profile:?}");
        let tnl = renderer.stats().tnl;
        assert_eq!(tnl.instanced, INSTANCES as u32, "{profile:?}");
        let off = renderer.pc_gpu_rejects();
        if profile == SimProfile::Native {
            assert_eq!(tnl.pc_gpu_draws, INSTANCES as u32);
            assert!(tnl.pc_gpu_tris > 0);
            assert_eq!(off, [0; trino_core::PcGpuReject::COUNT]);
        } else {
            assert_eq!((tnl.pc_gpu_draws, tnl.pc_gpu_tris), (0, 0), "{profile:?}");
            assert_eq!(off, [0; trino_core::PcGpuReject::COUNT]);
        }
    }
}

/// Evidence for issue #51: the `instancing` step's frame on the native
/// profile as separate draws, as one call, and their difference (black =
/// identical), side by side in `target/evidence/instancing/instancing.png`.
#[test]
#[ignore = "writes evidence PNGs: run with --ignored"]
fn instancing_evidence() {
    use castle64::selftest::{SLOTS_CARD, instancing_scene};
    use trino_core::{Camera3, Color, DEFAULT_LIGHT, Material, Renderer, Vec3};
    let save_dir = tempfile::tempdir().unwrap();
    let Some((mut r, _)) = pc_target(SimProfile::Native, save_dir.path()) else {
        return;
    };
    let (instances, palettes, params) = instancing_scene();
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -2.0),
        target: Vec3::ZERO,
        fov_y: 60.0 * std::f32::consts::PI / 180.0,
    };
    let mut frames = Vec::new();
    for one_call in [false, true] {
        r.begin_frame(Color::BLACK);
        r.set_camera(&camera);
        r.set_light(&DEFAULT_LIGHT);
        if one_call {
            r.draw_model_instances_with(SLOTS_CARD, &instances, &palettes, &params);
        } else {
            for i in &instances {
                let p = i.params(&params, &palettes);
                r.draw_model_matrix(SLOTS_CARD, &i.matrix, Material::VertexLit, &p);
            }
        }
        r.end_frame();
        frames.push(r.read_offscreen());
    }
    let (w, h) = r.internal_size();
    let diff: Vec<u8> = frames[0]
        .chunks(4)
        .zip(frames[1].chunks(4))
        .flat_map(|(a, b)| {
            let d = |k: usize| (a[k] as i32 - b[k] as i32).unsigned_abs().min(255) as u8;
            [d(0), d(1), d(2), 255]
        })
        .collect();
    assert!(
        diff.chunks(4).all(|p| p[..3] == [0, 0, 0]),
        "one call = separate draws"
    );
    let gap = 8u32;
    let total_w = w * 3 + gap * 2;
    let mut out = vec![40u8; (total_w * h * 4) as usize];
    for (k, img) in [&frames[0], &frames[1], &diff].into_iter().enumerate() {
        let x0 = k as u32 * (w + gap);
        for y in 0..h {
            let src = &img[(y * w * 4) as usize..((y + 1) * w * 4) as usize];
            let at = ((y * total_w + x0) * 4) as usize;
            out[at..at + src.len()].copy_from_slice(src);
        }
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/evidence/instancing");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join("instancing.png")).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), total_w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&out).unwrap();
}
