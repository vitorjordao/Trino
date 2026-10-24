//! The post stack on the PC renderer (issue #64, ADR 0033): off by default
//! and byte-identical then, neutral settings within a step of the plain
//! frame, the shader held to `trino_core::post`'s CPU math (every tonemap,
//! exposure, the LUT), bloom around an emissive, the HUD untouched, the
//! range past 1 kept in HDR, every pass combination (MSAA, render scale,
//! AO, the shadow map, water, both T&L paths), the console profiles
//! unaffected, and the daltonization filter per mode against its CPU
//! reference. Goldens `tests/golden/pc/post_*` and `cvd_filter_*`.
//!
//! `TRINO_POST_SHOTS=<dir>` also writes every frame, for review.

use std::path::PathBuf;

use trino_asset_pipeline::{Pivot, Primitive, Shape};
use trino_core::post::{Lut, tone};
use trino_core::render3d::{Camera3, Light};
use trino_core::{
    AmbientOcclusion, Bloom, Color, CvdMode, Fog, LocalLight, LutId, Material, ModelId,
    ModelParams, PostSettings, Renderer, Shading, ShadowCast, ShadowSettings, Tonemap, Transform3,
    Vec3,
};
use trino_platform_pc::{PcRenderer, PostView, SimProfile};

const WALL: ModelId = ModelId(0x6401);
const SWATCH: ModelId = ModelId(0x6402);
const LAMP: ModelId = ModelId(0x6403);
const FLOOR: ModelId = ModelId(0x6404);
const LUT: LutId = LutId::of("luts/test-rotate");

/// The swatches' colors, left to right, top row then bottom row.
const COLORS: [[u8; 3]; 8] = [
    [255, 255, 255],
    [200, 60, 20],
    [30, 120, 220],
    [128, 128, 128],
    [10, 10, 10],
    [250, 220, 90],
    [90, 200, 120],
    [60, 30, 90],
];

static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn headless(profile: SimProfile) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => Some(r),
        Err(e) => {
            if std::env::var("TRINO_REQUIRE_GPU").is_ok() {
                panic!("TRINO_REQUIRE_GPU set but no adapter: {e}");
            }
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

fn prim(shape: Shape, color: [u8; 4], pivot: Pivot) -> Vec<u8> {
    Primitive {
        shape,
        color,
        pivot,
        segments: 1,
        ..Primitive::default()
    }
    .build()
    .encode()
    .unwrap()
}

/// A 17³ LUT that rotates the channels: (r, g, b) → (g, b, r), exact under
/// trilinear filtering (each output is linear in one input).
fn rotate_lut() -> Vec<u8> {
    let n = 17u8;
    let mut v = Lut::header(n).to_vec();
    for b in 0..n as u32 {
        for g in 0..n as u32 {
            for r in 0..n as u32 {
                let _ = r;
                v.extend([Lut::neutral(n, g), Lut::neutral(n, b), Lut::neutral(n, r)]);
            }
        }
    }
    v
}

fn setup(r: &mut PcRenderer) {
    r.upload_mesh(
        WALL,
        prim(
            Shape::Box {
                size: [12.0, 8.0, 0.2],
            },
            [40, 44, 60, 255],
            Pivot::Center,
        ),
    );
    r.upload_mesh(
        SWATCH,
        prim(
            Shape::Box {
                size: [0.8, 0.8, 0.1],
            },
            [255, 255, 255, 255],
            Pivot::Center,
        ),
    );
    r.upload_mesh(
        LAMP,
        prim(
            Shape::Box {
                size: [0.3, 0.3, 0.3],
            },
            [255, 255, 255, 255],
            Pivot::Center,
        ),
    );
    r.upload_mesh(
        FLOOR,
        prim(
            Shape::Box {
                size: [3.0, 0.2, 3.0],
            },
            [120, 120, 130, 255],
            Pivot::Bottom,
        ),
    );
    r.upload_lut(LUT, &rotate_lut()).unwrap();
}

const CAMERA: Camera3 = Camera3 {
    eye: Vec3::new(0.0, 0.0, -4.0),
    target: Vec3::new(0.0, 0.0, 5.0),
    fov_y: 60.0 * core::f32::consts::PI / 180.0,
};

const SUN: Light = Light {
    dir: Vec3::new(0.3, -0.8, 0.5),
    ambient: 0.5,
    color: Color::WHITE,
    ambient_color: Color::WHITE,
    fog: Fog::NONE,
};

fn swatch_pos(i: usize) -> Vec3 {
    let col = (i % 4) as f32;
    let row = (i / 4) as f32;
    Vec3::new(-1.65 + col * 1.1, 0.6 - row * 1.1, 0.0)
}

const LAMP_POS: Vec3 = Vec3::new(0.0, -1.7, 0.0);

#[derive(Clone, Copy, Default)]
struct Extra {
    ao: bool,
    shadows: bool,
    lights: bool,
    hud: bool,
}

/// The test frame: the unlit wall and swatches, the emissive lamp, then
/// whatever `extra` adds; `post` given to `set_post` (`None`: not called).
fn frame(r: &mut PcRenderer, post: Option<Option<PostSettings>>, extra: Extra) -> Vec<u8> {
    r.begin_frame(Color::rgb(20, 22, 30));
    r.set_camera(&CAMERA);
    r.set_light(&SUN);
    if let Some(p) = post {
        r.set_post(p.as_ref());
    }
    if extra.ao {
        r.set_ambient_occlusion(Some(&AmbientOcclusion::DEFAULT));
    }
    if extra.shadows {
        r.set_shadows(Some(&ShadowSettings::DEFAULT));
    }
    if extra.lights {
        r.set_local_lights(&[LocalLight {
            intensity: 4.0,
            ..LocalLight::point(Vec3::new(1.6, -1.0, -1.2), Color::rgb(255, 230, 200), 1.2)
        }]);
    }
    let unlit = |c: [u8; 3]| ModelParams {
        shading: Shading::Unlit,
        tint: Color::rgb(c[0], c[1], c[2]),
        ..ModelParams::DEFAULT
    };
    let at = |p: Vec3| Transform3 {
        position: p,
        ..Default::default()
    };
    r.draw_model(
        WALL,
        &at(Vec3::new(0.0, 0.0, 0.3)),
        Material::VertexLit,
        &unlit([255; 3]),
    );
    for (i, c) in COLORS.iter().enumerate() {
        r.draw_model(SWATCH, &at(swatch_pos(i)), Material::VertexLit, &unlit(*c));
    }
    r.draw_model(
        LAMP,
        &at(LAMP_POS),
        Material::VertexLit,
        &ModelParams {
            shading: Shading::Unlit,
            shadow: if extra.shadows {
                ShadowCast::Mesh
            } else {
                ShadowCast::None
            },
            ..ModelParams::DEFAULT
        },
    );
    if extra.lights || extra.shadows {
        r.draw_model(
            FLOOR,
            &at(Vec3::new(2.2, -1.7, -1.0)),
            Material::VertexLit,
            &ModelParams {
                gloss: 255,
                ..ModelParams::DEFAULT
            },
        );
    }
    if extra.hud {
        r.fill_rect(
            trino_core::Rect::new(4.0, 4.0, 24.0, 16.0),
            Color::rgb(100, 150, 200),
        );
    }
    r.end_frame();
    r.read_offscreen()
}

fn project(r: &PcRenderer, p: Vec3) -> (u32, u32) {
    let (w, h) = r.internal_size();
    let v = CAMERA.view().transform_point(p);
    let half = CAMERA.fov_y * 0.5;
    let focal = half.cos() / half.sin() * h as f32 * 0.5;
    let x = w as f32 * 0.5 + v.x * focal / v.z;
    let y = h as f32 * 0.5 - v.y * focal / v.z;
    (x as u32, y as u32)
}

fn at(r: &PcRenderer, img: &[u8], x: u32, y: u32) -> [u8; 3] {
    let (w, _) = r.internal_size();
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

fn px(r: &PcRenderer, img: &[u8], p: Vec3) -> [u8; 3] {
    let (x, y) = project(r, p);
    at(r, img, x, y)
}

fn max_diff(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
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

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/pc")
}

/// Compare with `tests/golden/pc/<name>.png` (or bless it with
/// `TRINO_BLESS=1`): channels within `tol`, up to `allowed` beyond it.
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
    let bad = img
        .iter()
        .zip(&want)
        .filter(|(a, b)| a.abs_diff(**b) > tol)
        .count();
    if bad > allowed {
        write_png(&golden_dir().join(format!("actual/{name}.png")), w, h, img);
        panic!("{name}: {bad} channel values differ by more than {tol} (allowed {allowed})");
    }
}

fn shots(r: &PcRenderer, name: &str, img: &[u8]) {
    if let Ok(dir) = std::env::var("TRINO_POST_SHOTS") {
        let (w, h) = r.internal_size();
        write_png(&PathBuf::from(dir).join(format!("{name}.png")), w, h, img);
    }
}

fn expect_tone(post: &PostSettings, c: [u8; 3]) -> [u8; 3] {
    let t = tone(post, c.map(|v| v as f32 / 255.0));
    t.map(|v| (v * 255.0 + 0.5) as u8)
}

#[test]
fn post_off_is_the_old_frame_byte_for_byte() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let plain = frame(&mut r, None, Extra::default());
    let none = frame(&mut r, Some(None), Extra::default());
    assert_eq!(plain, none, "set_post(None) changes nothing");
    // A frame with post, then without: the stack leaves nothing behind.
    let _ = frame(&mut r, Some(Some(PostSettings::FILMIC)), Extra::default());
    let after = frame(&mut r, None, Extra::default());
    assert_eq!(plain, after, "turning post off restores the plain frame");
    assert_eq!(r.frame_times().post_passes, 0);
    // The view forces it off whatever the game sets.
    r.set_post_view(PostView::Off);
    let forced = frame(&mut r, Some(Some(PostSettings::FILMIC)), Extra::default());
    assert_eq!(plain, forced);
}

#[test]
fn neutral_settings_change_nothing_but_rounding() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let plain = frame(&mut r, None, Extra::default());
    let neutral = frame(&mut r, Some(Some(PostSettings::DEFAULT)), Extra::default());
    assert_eq!(r.frame_times().post_passes, 1);
    assert_eq!(r.frame_times().bloom_levels, 0);
    let d = max_diff(&plain, &neutral);
    assert!(d <= 1, "neutral post moved a channel by {d}");
}

#[test]
fn every_tonemap_and_exposure_matches_the_cpu_math() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    for (hw, exposure) in [(true, 1.6), (false, 0.7)] {
        r.set_hw_tnl(hw);
        for tonemap in Tonemap::ALL {
            let post = PostSettings {
                exposure,
                tonemap,
                ..PostSettings::DEFAULT
            };
            let img = frame(&mut r, Some(Some(post)), Extra::default());
            shots(&r, &format!("tone_{}_{hw}", tonemap.key()), &img);
            for (i, c) in COLORS.iter().enumerate() {
                let got = px(&r, &img, swatch_pos(i));
                let want = expect_tone(&post, *c);
                for k in 0..3 {
                    assert!(
                        got[k].abs_diff(want[k]) <= 2,
                        "{tonemap:?} ×{exposure} hw={hw} swatch {i} {c:?}: {got:?} vs {want:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_lut_grades_like_the_cpu_and_a_missing_one_is_skipped() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let bytes = rotate_lut();
    let lut = Lut::parse(&bytes).unwrap();
    let post = PostSettings {
        tonemap: Tonemap::Aces,
        grading: Some(LUT),
        ..PostSettings::DEFAULT
    };
    let img = frame(&mut r, Some(Some(post)), Extra::default());
    shots(&r, "lut_rotate", &img);
    for (i, c) in COLORS.iter().enumerate() {
        let toned = tone(&post, c.map(|v| v as f32 / 255.0));
        let want = lut.apply(toned).map(|v| (v * 255.0 + 0.5) as u8);
        let got = px(&r, &img, swatch_pos(i));
        for k in 0..3 {
            assert!(
                got[k].abs_diff(want[k]) <= 3,
                "swatch {i}: {got:?} vs {want:?}"
            );
        }
    }
    // A LUT nobody uploaded grades nothing (the tonemap still runs).
    let missing = PostSettings {
        grading: Some(LutId::of("luts/missing")),
        ..post
    };
    let img = frame(&mut r, Some(Some(missing)), Extra::default());
    let want = expect_tone(&post, COLORS[1]);
    let got = px(&r, &img, swatch_pos(1));
    assert!(
        got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2),
        "{got:?} vs {want:?}"
    );
    assert_eq!(r.remove_lut(LUT), 17 * 17 * 17 * 4);
    assert!(!r.has_lut(LUT));
    assert!(r.upload_lut(LUT, b"TLUT nope").is_err());
}

#[test]
fn bloom_glows_around_the_emissive_lamp_and_the_hud_stays_untouched() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let hud = Extra {
        hud: true,
        ..Extra::default()
    };
    let plain = frame(&mut r, Some(Some(PostSettings::DEFAULT)), hud);
    let bloom = PostSettings {
        bloom: Some(Bloom {
            threshold: 0.8,
            intensity: 1.5,
            radius: 0.8,
        }),
        exposure: 2.0,
        vignette: 0.5,
        ..PostSettings::DEFAULT
    };
    let img = frame(&mut r, Some(Some(bloom)), hud);
    shots(&r, "bloom", &img);
    assert!(r.frame_times().bloom_levels >= 4);
    // Just beside the lamp (0.15 m half-side): brighter with the bloom.
    let beside = LAMP_POS + Vec3::new(0.32, 0.0, -0.2);
    let (a, b) = (px(&r, &plain, beside), px(&r, &img, beside));
    let exposed = expect_tone(
        &PostSettings {
            exposure: 2.0,
            ..PostSettings::DEFAULT
        },
        a,
    );
    assert!(
        b[0] as i32 > exposed[0] as i32 + 12,
        "bloom beside the lamp: {a:?} -> {b:?} (exposure alone {exposed:?})"
    );
    // The HUD rectangle was drawn after the last 3D draw: no exposure,
    // vignette or bloom reaches it.
    assert_eq!(at(&r, &img, 10, 10), [100, 150, 200]);
    // The vignette darkens the corners of the 3D.
    let (w, h) = r.internal_size();
    let corner = at(&r, &img, w - 3, h - 3);
    let mid = at(&r, &img, w / 2 - 70, h / 2 - 20);
    assert!(
        corner[2] < mid[2],
        "vignette: corner {corner:?} mid {mid:?}"
    );
    // The bloom view shows the bloom alone: black far from the lamp, lit
    // around it.
    r.set_post_view(PostView::Bloom);
    let mask = frame(&mut r, Some(Some(bloom)), Extra::default());
    shots(&r, "bloom_mask", &mask);
    assert!(px(&r, &mask, LAMP_POS)[0] > 40);
    assert!(at(&r, &mask, 3, 3)[0] < 4);
    r.set_post_view(PostView::Game);
}

#[test]
fn the_hdr_target_keeps_highlights_past_white() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    if !r.post_hdr() {
        eprintln!("skipping: no float render targets");
        return;
    }
    setup(&mut r);
    let lit = Extra {
        lights: true,
        ..Extra::default()
    };
    let plain = frame(&mut r, None, lit);
    // The floor right under the lamp light: clipped white without post.
    let spot = Vec3::new(1.6, -1.5, -1.2);
    let clipped = px(&r, &plain, spot);
    assert!(
        clipped[0] >= 250,
        "the light saturates the floor: {clipped:?}"
    );
    let dim = PostSettings {
        exposure: 0.25,
        ..PostSettings::DEFAULT
    };
    let img = frame(&mut r, Some(Some(dim)), lit);
    shots(&r, "hdr_highlight", &img);
    // Two stops down: a clipped 1.0 would read encode(0.25) = 137.
    let got = px(&r, &img, spot);
    assert!(got[0] > 150, "the highlight kept its range: {got:?}");
}

#[test]
fn post_runs_with_msaa_render_scale_ao_shadows_and_both_tnl_paths() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let all = Extra {
        ao: true,
        shadows: true,
        lights: true,
        hud: true,
    };
    let post = PostSettings {
        grading: Some(LUT),
        ..PostSettings::FILMIC
    };
    let base = frame(&mut r, Some(Some(post)), all);
    let check = |r: &PcRenderer, img: &[u8], what: &str, tol: u8| {
        for i in [0, 1, 2, 5] {
            let (a, b) = (px(r, &base, swatch_pos(i)), px(r, img, swatch_pos(i)));
            for k in 0..3 {
                assert!(
                    a[k].abs_diff(b[k]) <= tol,
                    "{what} swatch {i}: {a:?} vs {b:?}"
                );
            }
        }
        assert_eq!(at(r, img, 10, 10), [100, 150, 200], "{what}: the HUD");
    };
    r.set_msaa(true);
    let ms = frame(&mut r, Some(Some(post)), all);
    shots(&r, "all_msaa", &ms);
    check(&r, &ms, "msaa", 3);
    r.set_render_scale(0.5);
    let half = frame(&mut r, Some(Some(post)), all);
    shots(&r, "all_half", &half);
    check(&r, &half, "half scale + msaa", 6);
    r.set_msaa(false);
    r.set_hw_tnl(false);
    let cpu = frame(&mut r, Some(Some(post)), all);
    check(&r, &cpu, "half scale, CPU T&L", 6);
    r.set_render_scale(1.0);
    let cpu1 = frame(&mut r, Some(Some(post)), all);
    check(&r, &cpu1, "CPU T&L", 3);
    // And off again: the plain frame, no stray format left behind.
    let plain = frame(&mut r, None, all);
    r.set_hw_tnl(true);
    let plain_gpu = frame(&mut r, None, all);
    assert!(max_diff(&plain, &plain_gpu) <= 8);
}

#[test]
fn console_profiles_ignore_the_post_stack() {
    let _g = gpu();
    for profile in [SimProfile::N64, SimProfile::N3ds] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        let plain = frame(&mut r, None, Extra::default());
        let post = PostSettings {
            grading: Some(LUT),
            exposure: 3.0,
            ..PostSettings::FILMIC
        };
        let with = frame(&mut r, Some(Some(post)), Extra::default());
        assert_eq!(plain, with, "{profile:?}");
        assert_eq!(r.frame_times().post_passes, 0);
        r.set_cvd_filter(CvdMode::Deuteranopia);
        let fixed = frame(&mut r, None, Extra::default());
        assert_eq!(
            plain, fixed,
            "{profile:?}: no correction filter on a console profile"
        );
    }
}

#[test]
fn the_native_golden_with_bloom_grading_and_vignette() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let post = PostSettings {
        exposure: 1.2,
        tonemap: Tonemap::AgX,
        bloom: Some(Bloom::DEFAULT),
        grading: Some(LUT),
        vignette: 0.3,
        grain: 0.0,
    };
    let img = frame(
        &mut r,
        Some(Some(post)),
        Extra {
            lights: true,
            hud: true,
            ..Extra::default()
        },
    );
    shots(&r, "post_native", &img);
    golden(&r, &img, "post_native", 3, 400);
}

#[test]
fn grain_moves_pixels_a_little_and_changes_every_frame() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let clean = frame(&mut r, Some(Some(PostSettings::DEFAULT)), Extra::default());
    let grain = PostSettings {
        grain: 0.5,
        ..PostSettings::DEFAULT
    };
    let a = frame(&mut r, Some(Some(grain)), Extra::default());
    let b = frame(&mut r, Some(Some(grain)), Extra::default());
    let d = max_diff(&clean, &a);
    assert!(d > 2 && d <= 12, "grain amplitude {d}");
    assert_ne!(a, b, "a new pattern each frame");
}

/// Solid colors → each mode's correction, held to the CPU reference.
#[test]
fn the_correction_filter_matches_its_cpu_reference_per_mode() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    assert_eq!(r.cvd_filter(), CvdMode::Off, "off by default");
    let plain = frame(&mut r, None, Extra::default());
    r.set_cvd_filter(CvdMode::Grayscale);
    assert_eq!(
        frame(&mut r, None, Extra::default()),
        plain,
        "nothing to correct"
    );
    for mode in [
        CvdMode::Protanopia,
        CvdMode::Deuteranopia,
        CvdMode::Tritanopia,
    ] {
        r.set_cvd_filter(mode);
        let img = frame(
            &mut r,
            None,
            Extra {
                hud: true,
                ..Extra::default()
            },
        );
        shots(&r, &format!("cvd_filter_{}", mode.key()), &img);
        for (i, c) in COLORS.iter().enumerate() {
            let want = trino_core::access::color::correct(mode, Color::rgb(c[0], c[1], c[2]));
            let got = px(&r, &img, swatch_pos(i));
            for (k, w) in [want.r, want.g, want.b].into_iter().enumerate() {
                assert!(
                    got[k].abs_diff(w) <= 2,
                    "{mode:?} swatch {i}: {got:?} vs {want:?}"
                );
            }
        }
        // The HUD too: the filter is the player's, over the whole picture.
        let hud = trino_core::access::color::correct(mode, Color::rgb(100, 150, 200));
        let got = at(&r, &img, 10, 10);
        assert!(got[0].abs_diff(hud.r) <= 2 && got[2].abs_diff(hud.b) <= 2);
        golden(&r, &img, &format!("cvd_filter_{}", mode.key()), 2, 200);
    }
    r.set_cvd_filter(CvdMode::Off);
    assert_eq!(frame(&mut r, None, Extra::default()), plain);
}
