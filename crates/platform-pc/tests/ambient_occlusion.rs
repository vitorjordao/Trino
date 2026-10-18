//! Screen-space ambient occlusion on the PC renderer (ADR 0014): the
//! native profile darkens creases and contacts, after the 3D and before
//! the 2D on top of it; the console profiles never run it (their AO is
//! baked into the models). Properties that hold on any GPU, plus goldens
//! (`tests/golden/pc/ao_*`, `cargo xtask test --bless`).
//!
//! `TRINO_AO_SHOTS=<dir>` also writes every frame, for review.

use std::path::PathBuf;

use trino_asset_pipeline::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, Light};
use trino_core::{
    AmbientOcclusion, AoQuality, AoTier, Color, Fog, Material, ModelId, ModelParams, Renderer,
    SpriteId, SpriteParams, Transform3, Vec2, Vec3,
};
use trino_platform_pc::{AoView, PcRenderer, SimProfile};

const FLOOR: ModelId = ModelId(0x2001);
const CRATE: ModelId = ModelId(0x2002);
const BALL: ModelId = ModelId(0x2003);
const PATCH: SpriteId = SpriteId(0x2004);

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

fn prim(shape: Shape, color: [u8; 4], pivot: Pivot, segments: u32) -> Vec<u8> {
    Primitive {
        shape,
        color,
        pivot,
        segments,
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
            Shape::Box {
                size: [10.0, 0.2, 10.0],
            },
            [170, 170, 170, 255],
            Pivot::Bottom,
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
            Pivot::Bottom,
            1,
        ),
    );
    r.upload_mesh(
        BALL,
        prim(
            Shape::Sphere { radius: 0.6 },
            [220, 220, 230, 255],
            Pivot::Bottom,
            16,
        ),
    );
    r.upload_sprite(PATCH, 8, 8, &[255u8; 8 * 8 * 4]);
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
    quality: AoQuality::High,
};

/// Where the crate's front face meets the floor (the crate stands at z = 2,
/// 1 m wide: its front face is at z = 1.5).
const CONTACT: Vec3 = Vec3::new(0.0, 0.0, 1.42);
/// The 15 cm gap between the two crates on the right: a narrow crease.
const CREASE: Vec3 = Vec3::new(1.625, 0.0, 1.6);
/// Floor far from everything.
const OPEN: Vec3 = Vec3::new(-1.0, 0.0, -0.5);
/// The middle of the crate's top face.
const TOP: Vec3 = Vec3::new(0.0, 1.0, 2.0);

/// The scene with `ao` (`None`: never call `set_ambient_occlusion`), and a
/// white patch drawn over the contact after the 3D when `patch`.
fn frame(r: &mut PcRenderer, ao: Option<Option<AmbientOcclusion>>, patch: bool) -> Vec<u8> {
    r.begin_frame(Color::rgb(40, 60, 90));
    r.set_camera(&CAMERA);
    r.set_light(&SUN);
    if let Some(ao) = ao {
        r.set_ambient_occlusion(ao.as_ref());
    }
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
    if patch {
        let (x, y) = project(r, CONTACT);
        r.draw_sprite(
            PATCH,
            Vec2::new(x as f32 - 4.0, y as f32 - 4.0),
            &SpriteParams {
                tint: Color::rgb(90, 200, 120),
                ..SpriteParams::DEFAULT
            },
        );
    }
    r.end_frame();
    r.read_offscreen()
}

/// Screen pixel of world point `p` (the T&L's projection).
fn project(r: &PcRenderer, p: Vec3) -> (u32, u32) {
    let (w, h) = r.internal_size();
    let v = CAMERA.view().transform_point(p);
    let half = CAMERA.fov_y * 0.5;
    let focal = half.cos() / half.sin() * h as f32 * 0.5;
    let x = w as f32 * 0.5 + v.x * focal / v.z;
    let y = h as f32 * 0.5 - v.y * focal / v.z;
    (x as u32, y as u32)
}

fn px(r: &PcRenderer, img: &[u8], p: Vec3) -> [u8; 3] {
    let (x, y) = project(r, p);
    at(r, img, x, y)
}

fn at(r: &PcRenderer, img: &[u8], x: u32, y: u32) -> [u8; 3] {
    let (w, _) = r.internal_size();
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

fn luma(c: [u8; 3]) -> i32 {
    (c[0] as i32 * 3 + c[1] as i32 * 6 + c[2] as i32) / 10
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
    if let Ok(dir) = std::env::var("TRINO_AO_SHOTS") {
        let (w, h) = r.internal_size();
        write_png(&PathBuf::from(dir).join(format!("{name}.png")), w, h, img);
    }
}

/// The frame's GPU T&L draws against the CPU path (`tnl_oracle`, issue #51),
/// the side-by-side kept under `target/evidence/pc-gpu-tnl`. Returns the
/// GPU draws (0 on a console profile).
fn tnl_oracle(
    r: &mut PcRenderer,
    name: &str,
    frame: impl FnMut(&mut PcRenderer) -> Vec<u8>,
) -> u32 {
    let Some(d) = trino_platform_pc::tnl_oracle::compare(r, frame) else {
        return 0;
    };
    eprintln!("{name}: {d}");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/evidence/pc-gpu-tnl");
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h, img) = d.side_by_side();
    write_png(&dir.join(format!("{name}.png")), w, h, &img);
    d.assert_within(name);
    d.gpu_draws
}

#[test]
fn the_native_tier_darkens_contacts_and_creases_only() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    assert_eq!(r.caps().ao_tier, AoTier::ScreenSpace);
    setup(&mut r);
    let off = frame(&mut r, None, false);
    let on = frame(&mut r, Some(Some(AO)), false);
    assert_eq!(r.frame_times().ao_passes, 1);
    shots(&r, "ao_off", &off);
    shots(&r, "ao_on", &on);
    for (name, p, min) in [("contact", CONTACT, 18), ("crease", CREASE, 25)] {
        let (a, b) = (px(&r, &off, p), px(&r, &on, p));
        assert!(
            luma(a) - luma(b) >= min,
            "{name} at {p:?}: {a:?} -> {b:?} (at least {min} darker expected)"
        );
    }
    for (name, p) in [("open floor", OPEN), ("crate top", TOP)] {
        let (a, b) = (px(&r, &off, p), px(&r, &on, p));
        assert!(
            luma(a) - luma(b) <= 3,
            "{name} at {p:?}: {a:?} -> {b:?} (unoccluded: unchanged)"
        );
    }
    // The cleared background: nothing to occlude, exactly unchanged.
    assert_eq!(at(&r, &off, 4, 4), at(&r, &on, 4, 4));
    golden(&r, &on, "ao_native", 4, 2048);
    let gpu = tnl_oracle(&mut r, "ao_native", |r| frame(r, Some(Some(AO)), false));
    assert!(gpu > 0, "the occluded scene's draws go to the GPU");
}

#[test]
fn no_ao_draws_exactly_what_the_renderer_drew_before() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let never = frame(&mut r, None, false);
    assert_eq!(r.frame_times().ao_passes, 0);
    let none = frame(&mut r, Some(None), false);
    assert!(never == none, "set_ambient_occlusion(None) changed pixels");
    // Cleared at begin_frame: an AO frame doesn't leak into the next.
    let _ = frame(&mut r, Some(Some(AO)), false);
    let after = frame(&mut r, None, false);
    assert!(never == after, "last frame's AO leaked");
    // Forced off: the game's settings are ignored.
    r.set_ao_view(AoView::Off);
    assert!(never == frame(&mut r, Some(Some(AO)), false));
}

#[test]
fn the_2d_drawn_after_the_3d_is_not_darkened() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let off = frame(&mut r, None, true);
    let on = frame(&mut r, Some(Some(AO)), true);
    // The patch over the contact keeps its exact color.
    assert_eq!(px(&r, &off, CONTACT), px(&r, &on, CONTACT));
    assert_eq!(px(&r, &on, CONTACT), [90, 200, 120]);
}

/// With MSAA on (the PC's best look, `trino-runner-pc`'s default) the
/// occlusion reads the multisampled depth: the same contacts darken, open
/// floor doesn't, and the 2D after the 3D keeps its exact color.
#[test]
fn msaa_frames_get_the_same_occlusion() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    r.set_msaa(true);
    assert!(r.msaa() > 1);
    let off = frame(&mut r, None, true);
    let on = frame(&mut r, Some(Some(AO)), true);
    assert_eq!(r.frame_times().ao_passes, 1);
    shots(&r, "ao_msaa_on", &on);
    let (a, b) = (px(&r, &off, CREASE), px(&r, &on, CREASE));
    assert!(
        luma(a) - luma(b) >= 25,
        "crease: {a:?} -> {b:?} (at least 25 darker expected)"
    );
    for (name, p) in [("open floor", OPEN), ("crate top", TOP)] {
        let (a, b) = (px(&r, &off, p), px(&r, &on, p));
        assert!(
            luma(a) - luma(b) <= 3,
            "{name} at {p:?}: {a:?} -> {b:?} (unoccluded: unchanged)"
        );
    }
    assert_eq!(px(&r, &on, CONTACT), [90, 200, 120]);
    // Against the single-sampled occlusion: the same picture but the
    // triangles' edges.
    r.set_msaa(false);
    let single = frame(&mut r, Some(Some(AO)), true);
    let (w, h) = r.internal_size();
    let mut far = 0;
    for y in 0..h {
        for x in 0..w {
            if (luma(at(&r, &on, x, y)) - luma(at(&r, &single, x, y))).abs() > 24 {
                far += 1;
            }
        }
    }
    assert!(
        far * 100 < (w * h) as usize,
        "{far} pixels differ by more than 24 from the single-sampled AO frame"
    );
}

#[test]
fn views_force_it_on_or_show_the_mask() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let game = frame(&mut r, Some(Some(AO)), false);
    // On: the game's settings when it sets some, the defaults otherwise.
    r.set_ao_view(AoView::On);
    assert!(game == frame(&mut r, Some(Some(AO)), false));
    let forced = frame(&mut r, None, false);
    let off = {
        r.set_ao_view(AoView::Off);
        frame(&mut r, None, false)
    };
    assert!(luma(px(&r, &forced, CREASE)) < luma(px(&r, &off, CREASE)) - 10);
    // Mask: white where nothing occludes, dark in the crease, the colors
    // of the frame gone.
    r.set_ao_view(AoView::Mask);
    let mask = frame(&mut r, Some(Some(AO)), false);
    shots(&r, "ao_mask", &mask);
    assert_eq!(at(&r, &mask, 4, 4), [255, 255, 255], "background");
    let open = px(&r, &mask, OPEN);
    assert!(open[0] >= 250 && open[0] == open[1] && open[1] == open[2]);
    let crease = px(&r, &mask, CREASE);
    assert!(crease[0] < 200, "crease in the mask: {crease:?}");
    golden(&r, &mask, "ao_mask", 4, 2048);
    tnl_oracle(&mut r, "ao_mask", |r| frame(r, Some(Some(AO)), false));
}

#[test]
fn quality_presets_and_intensity_change_the_occlusion() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let img = frame(&mut r, None, false);
    let off = luma(px(&r, &img, CREASE));
    let with = |r: &mut PcRenderer, ao: AmbientOcclusion| {
        let img = frame(r, Some(Some(ao)), false);
        luma(px(r, &img, CREASE))
    };
    for quality in [AoQuality::Low, AoQuality::Medium, AoQuality::High] {
        let v = with(&mut r, AmbientOcclusion { quality, ..AO });
        assert!(v < off - 10, "{quality:?}: {off} -> {v}");
    }
    let weak = with(
        &mut r,
        AmbientOcclusion {
            intensity: 0.4,
            ..AO
        },
    );
    let strong = with(
        &mut r,
        AmbientOcclusion {
            intensity: 2.0,
            ..AO
        },
    );
    assert!(strong < weak && weak < off, "{off} {weak} {strong}");
    assert_eq!(
        with(
            &mut r,
            AmbientOcclusion {
                intensity: 0.0,
                ..AO
            }
        ),
        off
    );
}

#[test]
fn console_profiles_never_run_it() {
    // Their AO is baked into the vertex colors: the sim profiles show what
    // the consoles show, whatever the game sets.
    let _gpu = gpu();
    for profile in [SimProfile::N64, SimProfile::N3ds] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        assert_eq!(r.caps().ao_tier, AoTier::Baked);
        setup(&mut r);
        let off = frame(&mut r, None, false);
        r.set_ao_view(AoView::On);
        let on = frame(&mut r, Some(Some(AO)), false);
        assert_eq!(r.frame_times().ao_passes, 0);
        assert!(off == on, "{profile:?} ran screen-space AO");
    }
}

#[test]
fn fog_fades_the_occlusion() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    // Fog full well before the crates: the AO fades with it.
    let foggy = Light {
        fog: Fog {
            color: Color::rgb(40, 60, 90),
            near: 0.5,
            far: 2.5,
        },
        ..SUN
    };
    let draw = |r: &mut PcRenderer, ao: Option<&AmbientOcclusion>| {
        r.begin_frame(Color::rgb(40, 60, 90));
        r.set_camera(&CAMERA);
        r.set_light(&foggy);
        r.set_ambient_occlusion(ao);
        for (m, p) in [
            (FLOOR, Vec3::new(0.0, -0.2, 0.0)),
            (CRATE, Vec3::new(1.05, 0.0, 2.0)),
            (CRATE, Vec3::new(2.2, 0.0, 2.0)),
        ] {
            r.draw_model(
                m,
                &Transform3 {
                    position: p,
                    ..Default::default()
                },
                Material::VertexLit,
                &ModelParams::DEFAULT,
            );
        }
        r.end_frame();
        r.read_offscreen()
    };
    let off = draw(&mut r, None);
    let on = draw(&mut r, Some(&AO));
    assert_eq!(px(&r, &off, CREASE), px(&r, &on, CREASE));
}

#[test]
fn tall_pictures_compute_it_at_half_resolution_with_the_same_result() {
    // Above 540 lines the occlusion is computed at half resolution and
    // upsampled by depth: the same creases darken, open floor and the
    // background stay, the silhouettes don't bleed onto the sky.
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    r.set_internal_size(1280, 960);
    let off = frame(&mut r, None, false);
    let on = frame(&mut r, Some(Some(AO)), false);
    shots(&r, "ao_half_res", &on);
    for (name, p, min) in [("contact", CONTACT, 18), ("crease", CREASE, 25)] {
        let (a, b) = (px(&r, &off, p), px(&r, &on, p));
        assert!(
            luma(a) - luma(b) >= min,
            "{name}: {a:?} -> {b:?} (at least {min} darker expected)"
        );
    }
    for p in [OPEN, TOP] {
        let (a, b) = (px(&r, &off, p), px(&r, &on, p));
        assert!(luma(a) - luma(b) <= 3, "{p:?}: {a:?} -> {b:?}");
    }
    // Every background pixel is exactly unchanged.
    let bg = at(&r, &off, 4, 4);
    let changed_sky = off
        .chunks(4)
        .zip(on.chunks(4))
        .filter(|(a, b)| a[..3] == bg && a != b)
        .count();
    assert_eq!(changed_sky, 0, "occlusion bled onto the background");
}
