//! Stereoscopic 3D on the PC's 3DS profile (issue #63, ADR 0032): the top
//! screen recorded and replayed once per eye like the 3DS does, the eyes
//! kept for read-back, the preview composed. Goldens of the left and right
//! eyes and the anaglyph (`tests/golden/pc/stereo_*`, `cargo xtask test
//! --bless`), and checks that hold on any GPU: the slider down is the mono
//! frame byte for byte, each preview is exactly its eyes, the second screen
//! still draws, and a warm-up leaves the composites nothing to compile.
//!
//! `TRINO_STEREO_SHOTS=<dir>` also writes every view, for review.

use std::path::PathBuf;

use trino_asset_pipeline::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::{
    Color, Eye, LayerDepth, Material, ModelId, ModelParams, PrecompileRequest, Rect, Renderer,
    Screen, StereoSettings, Transform3, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile, StereoView};

const FLOOR: ModelId = ModelId(0x5e01);
const PILLAR: ModelId = ModelId(0x5e02);
const BALL: ModelId = ModelId(0x5e03);

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/pc")
}

/// One GPU test at a time in this binary (see `local_lights.rs`).
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn headless() -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(SimProfile::N3ds)) {
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
        segments: 12,
        ..Primitive::default()
    }
    .build()
    .encode()
    .unwrap()
}

fn setup(r: &mut PcRenderer) {
    r.upload_mesh(
        FLOOR,
        prim(
            Shape::Plane { size: [14.0, 14.0] },
            [200, 190, 160, 255],
            Pivot::Center,
        ),
    );
    r.upload_mesh(
        PILLAR,
        prim(
            Shape::Box {
                size: [0.6, 2.0, 0.6],
            },
            [90, 120, 220, 255],
            Pivot::Bottom,
        ),
    );
    r.upload_mesh(
        BALL,
        prim(
            Shape::Sphere { radius: 0.4 },
            [230, 90, 70, 255],
            Pivot::Center,
        ),
    );
}

const CAMERA: Camera3 = Camera3 {
    eye: Vec3::new(0.0, 2.2, -5.0),
    target: Vec3::new(0.0, 0.8, 0.0),
    fov_y: 60.0 * core::f32::consts::PI / 180.0,
};

/// Zero parallax on the middle pillar; the ball in front pops out, the far
/// pillars sink in.
const STEREO: StereoSettings = StereoSettings::new(0.35, 5.2);

/// The scene with `stereo` (`None`: never call `set_stereo`), a HUD
/// rectangle floating 12 px out and a strip on the touch screen. Returns
/// the main target (the preview).
fn frame(r: &mut PcRenderer, stereo: Option<&StereoSettings>) -> Vec<u8> {
    r.begin_frame(Color::rgb(30, 34, 52));
    if let Some(s) = stereo {
        r.set_stereo(Some(s));
    }
    r.set_camera(&CAMERA);
    r.set_light(&DEFAULT_LIGHT);
    let at = |p: Vec3| Transform3 {
        position: p,
        ..Default::default()
    };
    let plain = ModelParams::DEFAULT;
    r.draw_model(FLOOR, &at(Vec3::ZERO), Material::VertexLit, &plain);
    for p in [
        Vec3::new(0.0, 0.0, 0.2),
        Vec3::new(-2.2, 0.0, 3.5),
        Vec3::new(2.4, 0.0, 5.0),
    ] {
        r.draw_model(PILLAR, &at(p), Material::VertexLit, &plain);
    }
    r.draw_model(
        BALL,
        &at(Vec3::new(0.9, 0.9, -2.2)),
        Material::VertexLit,
        &plain,
    );
    r.set_layer_depth(LayerDepth(12));
    r.fill_rect(Rect::new(16.0, 16.0, 60.0, 14.0), Color::rgb(250, 220, 60));
    r.set_layer_depth(LayerDepth::SCREEN);
    if r.on_screen(Screen::Bottom) {
        r.fill_rect(Rect::new(0.0, 0.0, 320.0, 30.0), Color::rgb(40, 200, 90));
    }
    r.end_frame();
    r.read_offscreen()
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

/// Compare with `tests/golden/pc/<name>.png` (or bless it), within `tol`
/// per channel and `allowed` channel values beyond it (drivers).
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
    if let Ok(dir) = std::env::var("TRINO_STEREO_SHOTS") {
        let (w, h) = r.internal_size();
        write_png(&PathBuf::from(dir).join(format!("{name}.png")), w, h, img);
    }
}

#[test]
fn the_eyes_and_the_anaglyph_match_their_goldens() {
    let _gpu = gpu();
    let Some(mut r) = headless() else { return };
    setup(&mut r);
    r.set_stereo_view(StereoView::Anaglyph);
    let anaglyph = frame(&mut r, Some(&STEREO));
    assert_eq!(r.stats().stereo.eyes, 2);
    let left = r.read_eye(Eye::Left).unwrap();
    let right = r.read_eye(Eye::Right).unwrap();
    shots(&r, "stereo_left", &left);
    shots(&r, "stereo_right", &right);
    shots(&r, "stereo_anaglyph", &anaglyph);
    // The anaglyph is exactly its eyes: red from the left, the rest from
    // the right.
    for ((a, l), rt) in anaglyph
        .as_chunks::<4>()
        .0
        .iter()
        .zip(left.as_chunks::<4>().0)
        .zip(right.as_chunks::<4>().0)
    {
        assert_eq!([a[0], a[1], a[2]], [l[0], rt[1], rt[2]]);
    }
    golden(&r, &left, "stereo_left", 3, 1024);
    golden(&r, &right, "stereo_right", 3, 1024);
    golden(&r, &anaglyph, "stereo_anaglyph", 3, 1024);
    for view in StereoView::ALL {
        r.set_stereo_view(view);
        let img = frame(&mut r, Some(&STEREO));
        shots(&r, &format!("stereo_view_{}", view.key()), &img);
    }
}

#[test]
fn the_slider_down_is_the_mono_frame_byte_for_byte() {
    let _gpu = gpu();
    let Some(mut r) = headless() else { return };
    setup(&mut r);
    let mono = frame(&mut r, None);
    let bottom = r.read_second_screen();
    // A game that asks for stereo, the preview off (the slider down).
    let asked = frame(&mut r, Some(&STEREO));
    assert_eq!(r.stats().stereo.eyes, 1);
    assert_eq!(asked, mono);
    assert_eq!(r.read_second_screen(), bottom);
    assert!(r.read_eye(Eye::Right).is_none());
    assert_eq!(r.read_eye_pixel(Eye::Right, 10, 10), None);
    // A preview with its slider at 0 is still mono.
    r.set_stereo_view(StereoView::Anaglyph);
    r.set_stereo_slider(0.0);
    assert_eq!(frame(&mut r, Some(&STEREO)), mono);
    // The slider up: two different eyes, the left shown under `Left`, the
    // touch screen drawn as before.
    r.set_stereo_slider(1.0);
    r.set_stereo_view(StereoView::Left);
    let shown = frame(&mut r, Some(&STEREO));
    let (left, right) = (
        r.read_eye(Eye::Left).unwrap(),
        r.read_eye(Eye::Right).unwrap(),
    );
    assert_eq!(shown, left);
    assert_ne!(left, right);
    assert_ne!(left, mono);
    assert_eq!(r.read_second_screen(), bottom);
    // The native profile has no stereo screen: always mono.
    let Ok(mut native) = pollster::block_on(PcRenderer::new_headless(SimProfile::Native)) else {
        return;
    };
    setup(&mut native);
    native.set_stereo_view(StereoView::Anaglyph);
    let a = frame(&mut native, None);
    let b = frame(&mut native, Some(&STEREO));
    assert_eq!(a, b);
    assert_eq!(native.stats().stereo.eyes, 1);
}

#[test]
fn each_preview_is_exactly_its_eyes() {
    let _gpu = gpu();
    let Some(mut r) = headless() else { return };
    setup(&mut r);
    let (w, _) = r.internal_size();
    let half = (w / 2) as usize;
    r.set_stereo_view(StereoView::Right);
    let shown = frame(&mut r, Some(&STEREO));
    let (left, right) = (
        r.read_eye(Eye::Left).unwrap(),
        r.read_eye(Eye::Right).unwrap(),
    );
    assert_eq!(shown, right);
    // Side by side: each half the average of its eye's two texels.
    r.set_stereo_view(StereoView::SideBySide);
    let sbs = frame(&mut r, Some(&STEREO));
    let w = w as usize;
    for y in [20usize, 120, 200] {
        for x in [3usize, 77, half - 1, half, half + 50, w - 1] {
            let (eye, sx) = if x < half {
                (&left, 2 * x)
            } else {
                (&right, 2 * (x - half))
            };
            let at = |x: usize| &eye[(y * w + x.min(w - 1)) * 4..][..3];
            let want: Vec<i32> = (0..3)
                .map(|c| (at(sx)[c] as i32 + at(sx + 1)[c] as i32) / 2)
                .collect();
            let got = &sbs[(y * w + x) * 4..][..3];
            for c in 0..3 {
                assert!(
                    (got[c] as i32 - want[c]).abs() <= 1,
                    "({x}, {y}): {got:?} vs {want:?}"
                );
            }
        }
    }
    // Flip: the left eye for a few frames, then the right.
    r.set_stereo_view(StereoView::Flip);
    let mut seen = Vec::new();
    for _ in 0..(2 * trino_platform_pc::renderer::STEREO_FLIP_FRAMES) {
        let img = frame(&mut r, Some(&STEREO));
        seen.push(img == r.read_eye(Eye::Left).unwrap());
    }
    assert!(
        seen.iter().any(|l| *l) && seen.iter().any(|l| !*l),
        "{seen:?}"
    );
}

#[test]
fn a_warm_up_leaves_the_composites_nothing_to_compile() {
    let _gpu = gpu();
    let Some(mut r) = headless() else { return };
    setup(&mut r);
    let status = r.precompile(&PrecompileRequest::EVERYTHING.blocking());
    assert!(status.is_done(), "{status:?}");
    let before = r.pipeline_counts().render_thread;
    for view in [StereoView::Anaglyph, StereoView::SideBySide] {
        r.set_stereo_view(view);
        frame(&mut r, Some(&STEREO));
        assert_eq!(r.frame_times().pipelines, 0, "{view:?} compiled in a frame");
    }
    assert_eq!(r.pipeline_counts().render_thread, before);
}
