//! `castle64::selftest::engine_checks` on the PC backend — the same checks
//! the N64/3DS self-tests run inside ares/Azahar. GPU-gated like the
//! goldens (`TRINO_REQUIRE_GPU=1` turns a missing adapter into a failure).

use std::path::PathBuf;

use trino_platform_pc::{PcPlatform, PcRenderer, SimProfile};

/// A strict PC renderer with every baked asset, and a platform serving the
/// blobs and a save file. `None` (skip) without a GPU.
fn pc_target(profile: SimProfile, save_dir: &std::path::Path) -> Option<(PcRenderer, PcPlatform)> {
    pc_target_on(profile, trino_core::HwTier::Base, save_dir)
}

/// [`pc_target`] simulating a console's hardware tier (`TRINO_SIM=n64x`,
/// `3dsnew`; issue #54).
fn pc_target_on(
    profile: SimProfile,
    tier: trino_core::HwTier,
    save_dir: &std::path::Path,
) -> Option<(PcRenderer, PcPlatform)> {
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
    renderer.set_hw_tier(tier);
    let mut platform = PcPlatform::new(&profile.caps().with_tier(tier))
        .with_save_file(save_dir.join("selftest.sav"));
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
    // The console profiles draw what the consoles' bake grades (issue #64).
    for m in assets.models {
        let tmdl = match (profile, m.graded) {
            (SimProfile::N64 | SimProfile::N3ds, Some(g)) => g,
            _ => m.tmdl,
        };
        renderer.upload_mesh(trino_core::ModelId(m.id), tmdl);
    }
    for b in assets.blobs {
        renderer.take_lut_blob(b.id, &b.bytes);
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
    assert_eq!(
        castle64::selftest::ui_layout(&mut renderer, &mut platform),
        Ok(true),
        "the UI probed on one screen (the touch button on the main one)"
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
    assert_eq!(
        castle64::selftest::ui_layout(&mut renderer, &mut platform),
        Ok(true),
        "the UI probed on both screens (a tap on the touch screen)"
    );
}

/// Every check on the simulated expanded tiers (issue #54): the N64 with
/// the Expansion Pak and the New 3DS. The `hw tier` step sees the tier's
/// caps and, the PC's one bake serving every tier, the base tier's data.
#[test]
fn engine_checks_pass_on_the_simulated_expanded_tiers() {
    use castle64::selftest::{TierReport, hw_tier};
    use trino_core::HwTier;
    let save_dir = tempfile::tempdir().unwrap();
    for (key, ram, cores, base_ram) in [
        ("n64x", 8u32 << 20, 1, 4u32 << 20),
        ("3dsnew", 64 << 20, 2, 64 << 20),
    ] {
        let (profile, tier) = SimProfile::parse_tier(key).unwrap();
        assert_eq!(tier, HwTier::Expanded);
        let Some((mut renderer, mut platform)) = pc_target_on(profile, tier, save_dir.path())
        else {
            return;
        };
        castle64::selftest::engine_checks(&mut renderer, &mut platform).unwrap();
        assert_eq!(
            hw_tier(&mut renderer, &mut platform),
            Ok(TierReport {
                tier: HwTier::Expanded,
                data: HwTier::Base,
                ram_bytes: ram,
                cpu_cores: cores,
                sparks: 4,
            }),
            "{key}"
        );
        // The base tier of the same profile, for contrast.
        let Some((mut renderer, mut platform)) = pc_target(profile, save_dir.path()) else {
            return;
        };
        let base = hw_tier(&mut renderer, &mut platform).unwrap();
        assert_eq!(
            (base.tier, base.ram_bytes),
            (HwTier::Base, base_ram),
            "{key}"
        );
        // A platform claiming a tier its renderer does not simulate fails.
        platform.set_hw_tier(HwTier::Expanded);
        assert!(hw_tier(&mut renderer, &mut platform).is_err(), "{key}");
    }
}

/// The `shader precompile` step (issue #101) on the PC's profiles: each
/// warms its lazy passes on workers while frames keep coming (the console
/// profiles still draw with wgpu pipelines: shadows, color-vision views),
/// and a second request finds nothing. The real N64 and 3DS answer 0
/// passes (their shaders are assembled at build time) in ares and Azahar.
#[test]
fn the_shader_precompile_step_warms_the_pc_and_skips_the_consoles() {
    use castle64::selftest::shader_precompile;
    let save_dir = tempfile::tempdir().unwrap();
    for profile in [SimProfile::Native, SimProfile::N64, SimProfile::N3ds] {
        let Some((mut renderer, mut platform)) = pc_target(profile, save_dir.path()) else {
            return;
        };
        let r = shader_precompile(&mut renderer, &mut platform).unwrap();
        // The PC's console profiles still draw with wgpu pipelines (their
        // shadow and color-vision passes): they warm too, just fewer.
        assert!(r.passes > 0, "{profile:?}: {r:?}");
        assert!(r.threads >= 1, "{profile:?}: {r:?}");
    }
}

/// The `post` step (issue #64) on the native profile and each console
/// profile: the card is graded alike (the PC's LUT at run time, the
/// consoles' graded bake) and blooms or glows. Breaking each tier's half
/// fails it: no LUT on the native profile, the ungraded mesh or no halo
/// sprite on a console profile.
#[test]
fn the_post_step_grades_and_glows_on_every_profile_and_catches_its_breakages() {
    use castle64::selftest::{POST_CARD, POST_LUT, post};
    let save_dir = tempfile::tempdir().unwrap();
    for profile in [SimProfile::Native, SimProfile::N64, SimProfile::N3ds] {
        let Some((mut renderer, _)) = pc_target(profile, save_dir.path()) else {
            return;
        };
        assert_eq!(post(&mut renderer), Ok(true), "{profile:?}");
        if profile == SimProfile::Native {
            renderer.remove_lut(POST_LUT);
            assert!(post(&mut renderer).unwrap_err().contains("not graded"));
        } else {
            // The ungraded mesh: what a console bake that ignored
            // `grading` would draw.
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
            let raw = trino_asset_pipeline::bake_model(
                &root.join("assets/shared/models/proxies/selftest-post.prim.ron"),
                &Default::default(),
            )
            .unwrap();
            renderer.upload_mesh(POST_CARD, raw);
            assert!(
                post(&mut renderer).unwrap_err().contains("not graded"),
                "{profile:?}"
            );
            let Some((mut renderer, _)) = pc_target(profile, save_dir.path()) else {
                return;
            };
            renderer.remove_sprite(trino_core::Glow::SPRITE);
            assert_eq!(
                post(&mut renderer),
                Err("post: no glow beside the card"),
                "{profile:?}"
            );
        }
    }
}

/// The `quality` step (issue #55) on each simulated console: a fixed
/// preset (Low on the N64 with or without the Pak and on the 3DS, Medium on
/// the New 3DS), LOD bias 1, the baked particle caps.
#[test]
fn the_quality_step_reports_each_consoles_fixed_preset() {
    use castle64::selftest::quality;
    use trino_core::{HwTier, QualityPreset};
    let save_dir = tempfile::tempdir().unwrap();
    for (key, want) in [
        ("n64", QualityPreset::Low),
        ("n64x", QualityPreset::Low),
        ("3ds", QualityPreset::Low),
        ("3dsnew", QualityPreset::Medium),
    ] {
        let (profile, tier) = SimProfile::parse_tier(key).unwrap();
        let target = if tier == HwTier::Expanded {
            pc_target_on(profile, tier, save_dir.path())
        } else {
            pc_target(profile, save_dir.path())
        };
        let Some((mut renderer, mut platform)) = target else {
            return;
        };
        let q = quality(&mut renderer, &mut platform).unwrap();
        assert_eq!((q.preset, q.fixed, q.lod_pct), (want, true, 100), "{key}");
        assert!(q.particles > 0, "{key}");
    }
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
        audio
            .upload_sound_blob(trino_core::SoundId(s.id), &s.blob)
            .unwrap();
    }
    for m in assets.music {
        audio
            .upload_music_blob(trino_core::MusicId(m.id), &m.blob)
            .unwrap();
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

/// The `shadows` step at every PC profile's tier (ADR 0025): the map on
/// native, projected shadows on the 3DS, a blob and a projected post on
/// the N64; each counted under its own tier.
#[test]
fn the_shadows_step_passes_at_every_tier() {
    use trino_core::{Renderer, ShadowTier};
    let save_dir = tempfile::tempdir().unwrap();
    for (profile, tier) in [
        (SimProfile::Native, ShadowTier::Map),
        (SimProfile::N64, ShadowTier::Blob),
        (SimProfile::N3ds, ShadowTier::Planar),
    ] {
        let Some((mut renderer, mut platform)) = pc_target(profile, save_dir.path()) else {
            return;
        };
        assert_eq!(
            castle64::selftest::shadows(&mut renderer, &mut platform),
            Ok(true),
            "{profile:?}"
        );
        assert_eq!(renderer.shadow_tier(), tier, "{profile:?}");
        castle64::selftest::shadow_scene(&mut renderer, Some(&trino_core::ShadowSettings::DEFAULT));
        let s = renderer.stats().shadows;
        match tier {
            ShadowTier::Map => assert_eq!((s.mapped, s.blobs, s.planar), (2, 0, 0)),
            ShadowTier::Planar => assert_eq!((s.mapped, s.blobs, s.planar), (0, 0, 2)),
            _ => assert_eq!((s.mapped, s.blobs, s.planar), (0, 1, 1)),
        }
    }
}

/// The `stereo` step (issue #63) reads both eyes on the PC's 3DS profile
/// and finds one picture, the mono one, on the profiles without a stereo
/// screen. The stats name each frame's eyes and why.
#[test]
fn the_stereo_step_reads_both_eyes_on_the_3ds_profile() {
    use castle64::selftest::StereoProbe;
    use trino_core::{Renderer, StereoMono};
    let save_dir = tempfile::tempdir().unwrap();
    for (profile, want) in [
        (SimProfile::N3ds, StereoProbe::Eyes { gpu_draws: 0 }),
        (SimProfile::N64, StereoProbe::Mono),
        (SimProfile::Native, StereoProbe::Mono),
    ] {
        let Some((mut renderer, _)) = pc_target(profile, save_dir.path()) else {
            return;
        };
        assert_eq!(
            castle64::selftest::stereo(&mut renderer),
            Ok(want),
            "{profile:?}"
        );
        // The step's last frame is the slider-at-0 one.
        let s = renderer.stats().stereo;
        let why = match want {
            StereoProbe::Eyes { .. } => StereoMono::Slider,
            _ => StereoMono::NoScreen,
        };
        assert_eq!((s.eyes, s.mono), (1, why), "{profile:?}");
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

/// Step `audio codecs` (issue #53) on the PC backend, offline: each frame
/// renders 1/60 s, so voices finish, streams drain and duck ramps move as
/// on a device. The output is audible (RMS) while the codec sounds play.
#[test]
fn audio_codecs_pass_on_pc() {
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
    let mut audio = PcAudio::new_offline(48_000);
    for s in assets.sounds {
        audio
            .upload_sound_blob(trino_core::SoundId(s.id), &s.blob)
            .unwrap();
    }
    for m in assets.music {
        audio
            .upload_music_blob(trino_core::MusicId(m.id), &m.blob)
            .unwrap();
    }
    let mix = assets
        .mixer
        .expect("assets/audio.toml bakes to audio/mixer");
    trino_core::MixerConfig::parse(&mix)
        .unwrap()
        .apply(&mut audio);
    let music = trino_core::Audio::bus_volume(&audio, trino_core::Bus::Music);
    assert!((music - 0.8).abs() < 1e-3, "the project's mix: {music}");

    let mut buf = vec![0.0f32; 800 * 2];
    let (mut energy, mut samples) = (0.0f64, 0usize);
    let stats = castle64::selftest::audio_codecs(&mut audio, |a| {
        a.render(&mut buf);
        energy += buf.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>();
        samples += buf.len();
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let rms = (energy / samples as f64).sqrt();
    assert!(rms > 0.01, "the step was audible: rms {rms}");
    assert_eq!(stats.underruns, 0);
    assert!(stats.streamed_frames >= castle64::selftest::AUDIO_STREAM_MIN_SAMPLES);
    let music = trino_core::Audio::bus_volume(&audio, trino_core::Bus::Music);
    assert!((music - 0.8).abs() < 1e-3, "the step restores the buses");
}

/// The `ui layout` step (issue #58) on the native profile at 1080p, 4K and
/// 21:9: whole-pixel scaling, the stack centered, the same probes.
#[test]
fn the_ui_layout_step_passes_at_pc_resolutions() {
    let save_dir = tempfile::tempdir().unwrap();
    let Some((mut renderer, mut platform)) = pc_target(SimProfile::Native, save_dir.path()) else {
        return;
    };
    for (w, h) in [(1920, 1080), (3840, 2160), (2560, 1080)] {
        renderer.set_internal_size(w, h);
        assert_eq!(
            castle64::selftest::ui_layout(&mut renderer, &mut platform),
            Ok(true),
            "{w}x{h}"
        );
    }
}

/// The `accessibility` step (issue #60) probes on every PC profile: the
/// palette swap, the caption box (on the 3DS profile's touch screen too),
/// the high-contrast focus and the settings round trip.
#[test]
fn the_accessibility_step_passes_on_every_profile() {
    let save_dir = tempfile::tempdir().unwrap();
    for profile in [
        SimProfile::N64,
        SimProfile::N3ds,
        SimProfile::Native,
        SimProfile::Web,
    ] {
        let Some((mut renderer, mut platform)) = pc_target(profile, save_dir.path()) else {
            return;
        };
        assert_eq!(
            castle64::selftest::accessibility(&mut renderer, &mut platform),
            Ok(true),
            "{profile:?}"
        );
    }
}

/// The `trace` step's lines (issue #62) read on the host as the emulator
/// harnesses read them: a platform that records `log` around the PC's, the
/// lines collected by `trino-trace`, the step's names and nesting there.
#[test]
fn the_trace_step_lines_collect_on_the_host() {
    use std::cell::RefCell;
    struct Capture {
        inner: PcPlatform,
        log: RefCell<String>,
    }
    impl trino_core::Platform for Capture {
        fn now_micros(&self) -> u64 {
            self.inner.now_micros()
        }
        fn log(&self, message: &str) {
            self.log.borrow_mut().push_str(message);
        }
        fn trace_platform(&self) -> trino_core::trace::TracePlatform {
            self.inner.trace_platform()
        }
    }
    let save_dir = tempfile::tempdir().unwrap();
    let Some((mut renderer, platform)) = pc_target(SimProfile::N64, save_dir.path()) else {
        return;
    };
    let mut p = Capture {
        inner: platform,
        log: RefCell::new(String::new()),
    };
    let events = castle64::selftest::trace(&mut renderer, &mut p).unwrap();
    assert_eq!(events, 7);
    let t = trino_trace::collect(&p.log.borrow()).unwrap();
    let s = t.session(castle64::selftest::TRACE_SESSION).unwrap();
    assert_eq!(s.platform, "pc");
    for n in castle64::selftest::TRACE_NAMES {
        assert!(s.names.iter().any(|x| x == n), "{n}");
    }
    let depth = |n: &str| {
        s.zones
            .iter()
            .find(|z| s.name_of(z.name) == n)
            .map(|z| z.depth)
    };
    assert_eq!((depth("update"), depth("physics")), (Some(0), Some(1)));
    assert_eq!((depth("render"), depth("present")), (Some(0), Some(1)));
}

/// Evidence for issue #63: castle64's hub on the PC's 3DS profile in every
/// stereo view (the game's own `set_stereo`: convergence on the player, the
/// HUD 6 px out), each eye, and the touch screen under the anaglyph, in
/// `target/evidence/stereo/`.
#[test]
#[ignore = "writes evidence PNGs: run with --ignored"]
fn stereo_evidence() {
    use trino_core::{Eye, Game, Renderer};
    use trino_platform_pc::StereoView;
    let save_dir = tempfile::tempdir().unwrap();
    let Some((mut r, _)) = pc_target(SimProfile::N3ds, save_dir.path()) else {
        return;
    };
    struct Mute;
    impl trino_core::Audio for Mute {
        fn play_sound(&mut self, _: trino_core::SoundId) {}
        fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/evidence/stereo");
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = r.internal_size();
    let save = |name: &str, w: u32, h: u32, rgba: &[u8]| {
        let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
        let mut e = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(rgba).unwrap();
    };
    for scene in castle64::perf::SCENES {
        let mut game =
            castle64::perf::scene_game(scene, trino_core::Vec2::new(w as f32, h as f32), &mut Mute);
        for view in StereoView::ALL {
            r.set_stereo_view(view);
            for _ in 0..2 {
                game.render(&mut r);
            }
            save(
                &format!("{}_{}", scene.name, view.key()),
                w,
                h,
                &r.read_offscreen(),
            );
            if view == StereoView::Anaglyph {
                assert_eq!(r.stats().stereo.eyes, 2, "{}", scene.name);
                save(
                    &format!("{}_eye_left", scene.name),
                    w,
                    h,
                    &r.read_eye(Eye::Left).unwrap(),
                );
                save(
                    &format!("{}_eye_right", scene.name),
                    w,
                    h,
                    &r.read_eye(Eye::Right).unwrap(),
                );
                let (bw, bh) = r.second_screen().map(|s| (s.x as u32, s.y as u32)).unwrap();
                save(
                    &format!("{}_bottom", scene.name),
                    bw,
                    bh,
                    &r.read_second_screen(),
                );
            }
        }
    }
}
