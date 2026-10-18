//! Quality presets on the PC renderer (issue #55, ADR 0022): the render
//! scale draws the 3D smaller and upscales it under a HUD that stays at the
//! picture's resolution, dynamic resolution follows the measured frame
//! time, the preset's AO caps the game's and its LOD bias scales the
//! game's LOD distances, the High preset
//! is the frame the native profile always drew, the console profiles
//! ignore all of it, and the auto-detect bench picks within 3 s. Goldens
//! for Low and High (`tests/golden/pc/quality_*`, `cargo xtask test
//! --bless`).
//!
//! `TRINO_QUALITY_SHOTS=<dir>` also writes every preset's frame, for
//! review.

use std::path::PathBuf;
use std::time::Duration;

use trino_asset_pipeline::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, Light};
use trino_core::{
    AmbientOcclusion, AoQuality, AoSetting, Color, DynRes, Fog, GpuClass, Material, ModelId,
    ModelParams, PresetTable, QualityPreset, QualitySettings, Rect, Renderer, Transform3, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

const FLOOR: ModelId = ModelId(0x2101);
const CRATE: ModelId = ModelId(0x2102);
const BALL: ModelId = ModelId(0x2103);

/// The pictures: 16:9, small enough for lavapipe.
const SIZE: (u32, u32) = (480, 272);
/// The HUD panel (opaque, top left) and its 1-pixel stripes.
const HUD: Rect = Rect::new(8.0, 8.0, 96.0, 32.0);

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/pc")
}

/// One GPU test at a time in this binary (RADV and parallel devices, see
/// `local_lights.rs`).
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn headless(profile: SimProfile) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(mut r) => {
            setup(&mut r);
            Some(r)
        }
        Err(e) => {
            if std::env::var("TRINO_REQUIRE_GPU").is_ok() {
                panic!("TRINO_REQUIRE_GPU set but no adapter: {e}");
            }
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

fn prim(shape: Shape, color: [u8; 4], segments: u32) -> Vec<u8> {
    Primitive {
        shape,
        color,
        pivot: Pivot::Bottom,
        segments,
        ..Primitive::default()
    }
    .build()
    .encode()
    .unwrap()
}

fn setup(r: &mut PcRenderer) {
    if r.profile() == SimProfile::Native {
        r.set_internal_size(SIZE.0, SIZE.1);
    }
    r.upload_mesh(
        FLOOR,
        prim(
            Shape::Box {
                size: [10.0, 0.2, 10.0],
            },
            [170, 170, 170, 255],
            1,
        ),
    );
    r.upload_mesh(
        CRATE,
        prim(
            Shape::Box {
                size: [1.0, 1.0, 1.0],
            },
            [200, 150, 100, 255],
            1,
        ),
    );
    r.upload_mesh(
        BALL,
        prim(Shape::Sphere { radius: 0.6 }, [220, 220, 230, 255], 16),
    );
}

const CAMERA: Camera3 = Camera3 {
    eye: Vec3::new(0.5, 2.6, -2.8),
    target: Vec3::new(0.5, 0.3, 1.8),
    fov_y: 60.0 * core::f32::consts::PI / 180.0,
};

const SUN: Light = Light {
    dir: Vec3::new(0.3, -0.8, 0.5),
    ambient: 0.45,
    color: Color::WHITE,
    ambient_color: Color::WHITE,
    fog: Fog::NONE,
};

const AO: AmbientOcclusion = AmbientOcclusion {
    radius: 0.6,
    intensity: 1.2,
    bias: 0.025,
    quality: AoQuality::Medium,
};

/// The scene (crates and a ball on a floor, the game asking for AO) with
/// a HUD panel of 1-pixel stripes over it.
fn frame(r: &mut PcRenderer) -> Vec<u8> {
    r.begin_frame(Color::rgb(40, 60, 90));
    r.set_camera(&CAMERA);
    r.set_light(&SUN);
    r.set_ambient_occlusion(Some(&AO));
    let mut draw = |m, p: Vec3| {
        r.draw_model(
            m,
            &Transform3 {
                position: p,
                ..Default::default()
            },
            Material::VertexLit,
            &ModelParams::DEFAULT,
        )
    };
    draw(FLOOR, Vec3::new(0.0, -0.2, 0.0));
    draw(CRATE, Vec3::new(0.0, 0.0, 2.0));
    draw(CRATE, Vec3::new(1.05, 0.0, 2.0));
    draw(CRATE, Vec3::new(2.2, 0.0, 2.0));
    draw(BALL, Vec3::new(-1.3, 0.0, 1.4));
    r.fill_rect(HUD, Color::rgb(10, 10, 20));
    for i in 0..24 {
        let x = HUD.pos.x + 4.0 + i as f32 * 3.0;
        r.fill_rect(
            Rect::new(x, HUD.pos.y + 4.0, 1.0, 24.0),
            Color::rgb(240, 220, 60),
        );
    }
    r.end_frame();
    r.read_offscreen()
}

/// The HUD panel's pixels.
fn hud(r: &PcRenderer, img: &[u8]) -> Vec<u8> {
    let (w, _) = r.internal_size();
    let (x0, y0) = (HUD.pos.x as u32, HUD.pos.y as u32);
    let mut out = Vec::new();
    for y in y0..y0 + HUD.size.y as u32 {
        let i = ((y * w + x0) * 4) as usize;
        out.extend_from_slice(&img[i..i + HUD.size.x as usize * 4]);
    }
    out
}

/// Channel values that differ by more than `tol`.
fn diff(a: &[u8], b: &[u8], tol: u8) -> usize {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .filter(|(x, y)| x.abs_diff(**y) > tol)
        .count()
}

fn shot(r: &PcRenderer, name: &str, img: &[u8]) {
    if let Ok(dir) = std::env::var("TRINO_QUALITY_SHOTS") {
        let (w, h) = r.internal_size();
        write_png(&PathBuf::from(dir).join(format!("{name}.png")), w, h, img);
    }
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = d.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// Compare with `tests/golden/pc/<name>.png` (or bless it): channel
/// values may differ by `tol`, and up to `allowed` of them by more (GPU
/// float precision and edge rules per driver, lavapipe in CI).
fn golden(r: &PcRenderer, img: &[u8], name: &str, tol: u8, allowed: usize) {
    let (w, h) = r.internal_size();
    let path = golden_dir().join(format!("{name}.png"));
    if std::env::var("TRINO_BLESS").is_ok() {
        write_png(&path, w, h, img);
        eprintln!("blessed golden: {}", path.display());
        return;
    }
    assert!(path.exists(), "missing {} — bless it", path.display());
    let (gw, gh, want) = read_png(&path);
    assert_eq!((gw, gh), (w, h));
    let bad = diff(img, &want, tol);
    if bad > allowed {
        write_png(&golden_dir().join(format!("actual/{name}.png")), w, h, img);
        panic!("{name}: {bad} channel values differ by more than {tol} (allowed {allowed})");
    }
}

/// A preset without dynamic resolution: the same picture every run.
fn fixed(p: QualityPreset) -> QualitySettings {
    QualitySettings {
        dynamic_resolution: None,
        ..QualitySettings::preset(p)
    }
}

#[test]
fn each_preset_draws_its_own_frame_and_low_and_high_match_their_goldens() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let mut frames = Vec::new();
    for p in QualityPreset::NAMED {
        r.apply_quality(&fixed(p));
        let img = frame(&mut r);
        shot(&r, &format!("quality_{}", p.key()), &img);
        let want = QualitySettings::preset(p);
        let scene = r.scene_size();
        assert_eq!(
            scene,
            (
                ((SIZE.0 as f32 * want.render_scale).round() as u32) & !1,
                ((SIZE.1 as f32 * want.render_scale).round() as u32) & !1
            ),
            "{p:?}: the 3D's size"
        );
        assert_eq!(r.msaa(), if want.msaa > 1 { 4 } else { 1 }, "{p:?}: MSAA");
        assert_eq!(r.lod_bias(), want.lod_bias, "{p:?}: LOD bias");
        assert_eq!(
            r.frame_times().upscales,
            u32::from(want.render_scale != 1.0),
            "{p:?}: an upscale pass only below a scale of 1"
        );
        frames.push((p, img));
    }
    // Each step up changes the picture (AO, MSAA, scale): presets are
    // never the same frame twice.
    for w in frames.windows(2) {
        assert!(
            diff(&w[0].1, &w[1].1, 2) > 0,
            "{:?} and {:?} drew the same frame",
            w[0].0,
            w[1].0
        );
    }
    // The HUD is drawn at the picture's resolution at every preset.
    let huds: Vec<_> = frames.iter().map(|(_, img)| hud(&r, img)).collect();
    for (h, (p, _)) in huds.iter().zip(&frames) {
        assert_eq!(diff(h, &huds[2], 0), 0, "{p:?}: the HUD changed");
    }
    r.apply_quality(&fixed(QualityPreset::Low));
    let low = frame(&mut r);
    golden(&r, &low, "quality_low", 4, 2048);
    r.apply_quality(&fixed(QualityPreset::High));
    let high = frame(&mut r);
    golden(&r, &high, "quality_high", 4, 2048);
}

#[test]
fn the_high_preset_draws_the_frame_the_native_profile_always_drew() {
    let _g = gpu();
    let Some(mut old) = headless(SimProfile::Native) else {
        return;
    };
    // The runner's start-up before presets: MSAA on, the game's AO.
    old.set_msaa(true);
    let before = frame(&mut old);
    drop(old);
    let mut r = headless(SimProfile::Native).unwrap();
    r.apply_quality(&QualitySettings::HIGH);
    assert_eq!(r.render_scale(), 1.0);
    assert_eq!(r.scene_size(), SIZE);
    let after = frame(&mut r);
    assert_eq!(
        diff(&before, &after, 0),
        0,
        "High must be the old native frame"
    );
    assert_eq!(r.frame_times().upscales, 0);
}

#[test]
fn a_scaled_scene_is_upscaled_and_sharpened_under_a_crisp_hud() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let base = QualitySettings {
        msaa: 1,
        ao: AoSetting::Off,
        ..fixed(QualityPreset::High)
    };
    r.apply_quality(&base);
    let full = frame(&mut r);
    r.apply_quality(&QualitySettings {
        render_scale: 0.5,
        sharpen: 0.0,
        ..base
    });
    assert_eq!(r.scene_size(), (SIZE.0 / 2, SIZE.1 / 2));
    let half = frame(&mut r);
    shot(&r, "scale_half", &half);
    r.apply_quality(&QualitySettings {
        render_scale: 0.5,
        sharpen: 1.0,
        ..base
    });
    let sharp = frame(&mut r);
    shot(&r, "scale_half_sharpened", &sharp);
    assert_eq!(r.frame_times().upscales, 1);
    // The HUD: the same pixels at every scale, every stripe still one
    // pixel wide.
    assert_eq!(diff(&hud(&r, &full), &hud(&r, &half), 0), 0);
    assert_eq!(diff(&hud(&r, &full), &hud(&r, &sharp), 0), 0);
    // The 3D: the same scene, softer (close to the full frame, not equal),
    // and the sharpening changes it.
    let changed = diff(&full, &half, 8);
    assert!(changed > 0, "a half-scale 3D can't match the full one");
    assert!(
        changed < full.len() / 4,
        "the half-scale frame is the same scene ({changed} values differ)"
    );
    assert!(diff(&half, &sharp, 1) > 0, "sharpening changed nothing");
    // Back to 1: no scaled targets, the full frame again.
    r.apply_quality(&base);
    assert_eq!(r.scene_size(), SIZE);
    assert_eq!(diff(&frame(&mut r), &full, 0), 0);
}

#[test]
fn the_preset_ao_caps_the_games() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let base = QualitySettings {
        msaa: 1,
        ..fixed(QualityPreset::High)
    };
    let mut at = |ao| {
        r.apply_quality(&QualitySettings { ao, ..base });
        frame(&mut r)
    };
    // The game asks for Medium AO (16 samples).
    let off = at(AoSetting::Off);
    let low = at(AoSetting::Low);
    let medium = at(AoSetting::Medium);
    let high = at(AoSetting::High);
    assert!(diff(&off, &medium, 4) > 100, "Off drops the pass");
    assert!(diff(&low, &medium, 1) > 0, "Low lowers the game's Medium");
    assert_eq!(
        diff(&medium, &high, 0),
        0,
        "High keeps the game's Medium: a cap never raises it"
    );
    assert_eq!(diff(&hud(&r, &off), &hud(&r, &low), 0), 0);
}

#[test]
fn dynamic_resolution_follows_the_frame_time_within_its_range() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let dynres = |target_us| QualitySettings {
        dynamic_resolution: Some(DynRes {
            target_us,
            min_scale: 0.5,
            max_scale: 1.0,
        }),
        ..fixed(QualityPreset::Medium)
    };
    // A target no frame can hold: down to the floor.
    r.apply_quality(&dynres(1));
    let start = r.render_scale();
    for _ in 0..80 {
        frame(&mut r);
        r.wait_gpu();
        r.report_frame_time(20_000);
    }
    assert!(r.gpu_frame_us() > 0, "a frame time was measured");
    assert_eq!(r.render_scale(), 0.5, "from {start}");
    assert_eq!(r.scene_size(), (SIZE.0 / 2, SIZE.1 / 2));
    // A target every frame holds: up to the ceiling.
    r.apply_quality(&dynres(10_000_000));
    for _ in 0..240 {
        frame(&mut r);
        r.wait_gpu();
        r.report_frame_time(1_000);
    }
    assert_eq!(r.render_scale(), 1.0);
    assert_eq!(r.scene_size(), SIZE);
}

#[test]
fn dynamic_resolution_steps_reuse_one_allocation_and_draw_what_a_fixed_scale_draws() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    // High: 4x MSAA and the game's AO, every target the 3D owns.
    let ranged = QualitySettings {
        dynamic_resolution: Some(DynRes {
            target_us: 10_000_000,
            min_scale: 0.5,
            max_scale: 1.0,
        }),
        ..fixed(QualityPreset::High)
    };
    for scale in [0.5, 0.7, 0.85, 1.0, 0.6] {
        r.apply_quality(&ranged);
        assert_eq!(r.scene_extent(), SIZE, "allocated for the range's top");
        r.set_render_scale(scale);
        assert_eq!(r.scene_extent(), SIZE, "a step allocates nothing");
        let stepped = frame(&mut r);
        let size = r.scene_size();
        // The same scale in targets of its own size (none at 1).
        r.apply_quality(&QualitySettings {
            render_scale: scale,
            ..fixed(QualityPreset::High)
        });
        assert_eq!((r.scene_size(), r.scene_extent()), (size, size));
        let own = frame(&mut r);
        shot(&r, &format!("dynres_corner_{scale}"), &stepped);
        assert_eq!(diff(&stepped, &own, 1), 0, "scale {scale}");
    }
}

#[test]
fn the_console_profiles_ignore_quality() {
    let _g = gpu();
    for profile in [SimProfile::N64, SimProfile::N3ds] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        let before = frame(&mut r);
        r.apply_quality(&QualitySettings::ULTRA);
        r.set_render_scale(0.5);
        assert_eq!(r.quality(), None, "{profile:?}");
        assert_eq!(r.render_scale(), 1.0, "{profile:?}");
        assert_eq!(r.lod_bias(), 1.0, "{profile:?}");
        assert_eq!(r.msaa(), 1, "{profile:?}");
        assert_eq!(diff(&before, &frame(&mut r), 0), 0, "{profile:?}");
    }
}

#[test]
fn the_bench_picks_a_preset_within_three_seconds_and_restores_the_renderer() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    r.set_internal_size(1280, 720);
    r.set_msaa(true);
    let cal = r.calibrate(&PresetTable::BUILT_IN, 15_000, Duration::from_secs(2));
    eprintln!("bench on {}: {}", r.adapter_info(), cal.summary());
    assert!(cal.elapsed <= Duration::from_secs(3), "{:?}", cal.elapsed);
    assert!(!cal.results.is_empty());
    assert_eq!(
        cal.results[0].0,
        QualityPreset::Low,
        "the walk starts at Low"
    );
    assert!(cal.gpu_us() > 0);
    if cal.gpu == GpuClass::Software {
        assert_eq!(cal.pick, QualityPreset::Low, "software adapters get Low");
    }
    // Only presets up to the first miss are walked, and the pick held the
    // target.
    for (i, (p, us)) in cal.results.iter().enumerate() {
        assert_eq!(*p, QualityPreset::NAMED[i]);
        if p.index() < cal.pick.index() {
            assert!(*us <= cal.target_us);
        }
    }
    // The renderer is back as it was: no quality applied, MSAA on, full
    // scale, and the bench's meshes gone (the scene still draws).
    assert_eq!(r.quality(), None);
    assert_eq!(r.msaa(), 4);
    assert_eq!(r.render_scale(), 1.0);
    assert_eq!(r.internal_size(), (1280, 720));
    r.set_internal_size(SIZE.0, SIZE.1);
    let img = frame(&mut r);
    assert!(img.iter().any(|v| *v != 0));
}
