//! Local lights on the PC renderer, per tier: the native profile shades
//! them per pixel (with specular and wetness), the N64 and 3DS profiles per
//! vertex like the consoles. Goldens per tier (`tests/golden/pc/lights_*`,
//! `cargo xtask test --bless`), plus property checks that hold on any GPU.
//!
//! `TRINO_LIGHTS_SHOTS=<dir>` also writes each tier with and without
//! lights (and wet), for review.

use std::path::PathBuf;

use trino_asset_pipeline::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, Light};
use trino_core::{
    Color, LocalLight, Material, ModelId, ModelParams, Renderer, Transform3, Vec2, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

const FLOOR: ModelId = ModelId(0x1001);
const BALL: ModelId = ModelId(0x1002);
const CRATE: ModelId = ModelId(0x1003);

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/pc")
}

/// One GPU test at a time in this binary: RADV crashed (SIGSEGV, ~1 run in
/// 10–40) creating and dropping several devices from parallel test threads
/// — `render_e2e` too, before these tests existed.
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

/// A `cells`×`cells` grid floor, `size` meters wide, facing up (TMDL).
fn floor_tmdl(size: f32, cells: u32, color: [u8; 4]) -> Vec<u8> {
    let n = cells + 1;
    let mut blob = Vec::new();
    blob.extend_from_slice(b"TMDL");
    blob.extend_from_slice(&(n * n).to_le_bytes());
    blob.extend_from_slice(&(cells * cells * 6).to_le_bytes());
    let step = size / cells as f32;
    for z in 0..n {
        for x in 0..n {
            for v in [
                x as f32 * step - size / 2.0,
                0.0,
                z as f32 * step - size / 2.0,
            ] {
                blob.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    for _ in 0..n * n {
        for v in [0.0f32, 1.0, 0.0] {
            blob.extend_from_slice(&v.to_le_bytes());
        }
    }
    for _ in 0..n * n {
        blob.extend_from_slice(&color);
    }
    for z in 0..cells {
        for x in 0..cells {
            let i = (z * n + x) as u16;
            let (a, b, c, d) = (i, i + 1, i + n as u16 + 1, i + n as u16);
            // Front faces CCW seen from above.
            for k in [a, c, b, a, d, c] {
                blob.extend_from_slice(&k.to_le_bytes());
            }
        }
    }
    blob.extend_from_slice(b"BNDS");
    blob.extend_from_slice(&16u32.to_le_bytes());
    for v in [0.0f32, 0.0, 0.0, size * 0.75] {
        blob.extend_from_slice(&v.to_le_bytes());
    }
    blob
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
    r.upload_mesh(FLOOR, floor_tmdl(10.0, 20, [150, 150, 150, 255]));
    r.upload_mesh(
        BALL,
        prim(Shape::Sphere { radius: 0.6 }, [230, 230, 230, 255], 16),
    );
    r.upload_mesh(
        CRATE,
        prim(
            Shape::Box {
                size: [1.0, 1.0, 1.0],
            },
            [190, 140, 90, 255],
            12,
        ),
    );
}

const CAMERA: Camera3 = Camera3 {
    eye: Vec3::new(0.0, 4.0, -7.0),
    target: Vec3::new(0.0, 0.0, 0.0),
    fov_y: 60.0 * core::f32::consts::PI / 180.0,
};

/// A dim blue night: the local lights do most of the work.
const NIGHT: Light = Light {
    dir: Vec3::new(0.3, -0.8, 0.5),
    ambient: 0.25,
    color: Color::rgb(120, 130, 170),
    ambient_color: Color::rgb(150, 160, 200),
    fog: trino_core::Fog::NONE,
};

fn lamps() -> [LocalLight; 3] {
    [
        LocalLight::point(Vec3::new(-1.5, 1.2, -0.5), Color::rgb(255, 170, 80), 4.0),
        LocalLight::point(Vec3::new(2.5, 1.0, -1.0), Color::rgb(255, 60, 200), 3.5)
            .with_intensity(1.2),
        LocalLight::spot(
            Vec3::new(0.0, 4.0, 2.0),
            Vec3::new(0.0, -1.0, -0.3),
            0.25,
            0.45,
            Color::rgb(80, 220, 255),
            7.0,
        ),
    ]
}

/// The scene with `lights` (`None`: never call `set_local_lights`).
fn frame(r: &mut PcRenderer, lights: Option<&[LocalLight]>, wet: f32) -> Vec<u8> {
    r.begin_frame(Color::rgb(10, 12, 24));
    r.set_camera(&CAMERA);
    r.set_light(&NIGHT);
    if let Some(l) = lights {
        r.set_local_lights(l);
    }
    if wet > 0.0 {
        r.set_wetness(wet);
    }
    let draw = |r: &mut PcRenderer, m, p: Vec3, gloss| {
        r.draw_model(
            m,
            &Transform3 {
                position: p,
                ..Default::default()
            },
            Material::VertexLit,
            &ModelParams {
                gloss,
                ..ModelParams::DEFAULT
            },
        )
    };
    draw(r, FLOOR, Vec3::ZERO, 0);
    draw(r, BALL, Vec3::new(-2.0, 0.0, 0.0), 0);
    draw(r, BALL, Vec3::new(2.0, 0.0, 0.0), 160);
    draw(r, CRATE, Vec3::new(0.0, 0.0, 2.0), 40);
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
    let (w, _) = r.internal_size();
    let (x, y) = project(r, p);
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
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
/// edge rules and float precision per driver).
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
    if let Ok(dir) = std::env::var("TRINO_LIGHTS_SHOTS") {
        let (w, h) = r.internal_size();
        write_png(&PathBuf::from(dir).join(format!("{name}.png")), w, h, img);
    }
}

#[test]
fn each_tier_matches_its_golden() {
    let _gpu = gpu();
    for (profile, name, tol) in [
        (SimProfile::Native, "lights_native", 3),
        (SimProfile::N3ds, "lights_n3ds", 3),
        (SimProfile::N64, "lights_n64", 9),
    ] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        let off = frame(&mut r, None, 0.0);
        let on = frame(&mut r, Some(&lamps()), 0.0);
        shots(&r, &format!("{name}_off"), &off);
        shots(&r, &format!("{name}_on"), &on);
        if profile == SimProfile::Native {
            let wet = frame(&mut r, Some(&lamps()), 0.8);
            shots(&r, &format!("{name}_wet"), &wet);
            golden(&r, &wet, "lights_native_wet", tol, 1024);
        }
        golden(&r, &on, name, tol, 1024);
    }
}

#[test]
fn no_lights_draw_exactly_what_the_renderer_drew_before() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let never = frame(&mut r, None, 0.0);
    let empty = frame(&mut r, Some(&[]), 0.0);
    assert!(
        never == empty,
        "an empty light list must not change a pixel"
    );
    // Lights are cleared at begin_frame.
    let _ = frame(&mut r, Some(&lamps()), 0.6);
    let after = frame(&mut r, None, 0.0);
    assert!(never == after, "last frame's lights and wetness leaked");
}

#[test]
fn lights_add_their_color_where_they_reach_on_every_tier() {
    let _gpu = gpu();
    for profile in [SimProfile::Native, SimProfile::N3ds, SimProfile::N64] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        let off = frame(&mut r, None, 0.0);
        let on = frame(&mut r, Some(&lamps()), 0.0);
        // The warm lamp's side of the left ball, and the floor under it.
        for p in [Vec3::new(-1.6, 0.0, -0.4), Vec3::new(-1.68, 0.99, -0.33)] {
            let (a, b) = (px(&r, &off, p), px(&r, &on, p));
            assert!(
                b[0] as i32 >= a[0] as i32 + 25 && b[0] >= b[2],
                "{profile:?} at {p:?}: {a:?} -> {b:?} (warm light expected)"
            );
        }
        // Far from every light: unchanged (within the N64 dither).
        let far = Vec3::new(4.5, 0.0, 4.5);
        let (a, b) = (px(&r, &off, far), px(&r, &on, far));
        for k in 0..3 {
            assert!(
                a[k].abs_diff(b[k]) <= 9,
                "{profile:?} far floor: {a:?} vs {b:?}"
            );
        }
    }
}

#[test]
fn per_pixel_lights_are_round_on_a_coarse_mesh() {
    let _gpu = gpu();
    // One big quad: per vertex a light in its middle can't show (no vertex
    // near it), per pixel it makes a pool.
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    r.upload_mesh(FLOOR, floor_tmdl(10.0, 1, [150, 150, 150, 255]));
    let lamp = [LocalLight::point(
        Vec3::new(0.0, 1.0, 0.0),
        Color::WHITE,
        2.5,
    )];
    let mut draw = |lights: &[LocalLight]| {
        r.begin_frame(Color::BLACK);
        r.set_camera(&CAMERA);
        r.set_light(&NIGHT);
        r.set_local_lights(lights);
        r.draw_model(
            FLOOR,
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        r.end_frame();
        r.read_offscreen()
    };
    let off = draw(&[]);
    let on = draw(&lamp);
    let center = px(&r, &on, Vec3::ZERO);
    let base = px(&r, &off, Vec3::ZERO);
    assert!(
        center[1] as i32 > base[1] as i32 + 60,
        "{base:?} -> {center:?}"
    );
    let edge = Vec3::new(3.5, 0.0, 0.0);
    assert_eq!(px(&r, &on, edge), px(&r, &off, edge), "outside the radius");
}

#[test]
fn gloss_and_wetness_add_highlights_per_pixel_only() {
    let _gpu = gpu();
    let count_bright = |img: &[u8]| {
        img.chunks(4)
            .filter(|p| p[0].max(p[1]).max(p[2]) > 220)
            .count()
    };
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let dry = frame(&mut r, Some(&lamps()), 0.0);
    let wet = frame(&mut r, Some(&lamps()), 0.9);
    assert!(
        count_bright(&wet) > count_bright(&dry) + 50,
        "wet floors mirror the lights: {} vs {} bright pixels",
        count_bright(&wet),
        count_bright(&dry)
    );
    // Per vertex (3DS) there is no wetness.
    let Some(mut r) = headless(SimProfile::N3ds) else {
        return;
    };
    setup(&mut r);
    assert!(frame(&mut r, Some(&lamps()), 0.0) == frame(&mut r, Some(&lamps()), 0.9));
}

#[test]
fn per_vertex_tiers_use_at_most_their_cap() {
    let _gpu = gpu();
    // The N64 profile evaluates 2 lights: a third changes nothing.
    let Some(mut r) = headless(SimProfile::N64) else {
        return;
    };
    assert_eq!(r.caps().max_local_lights, 2);
    setup(&mut r);
    let l = lamps();
    let two = frame(&mut r, Some(&l[..2]), 0.0);
    let three = frame(&mut r, Some(&l), 0.0);
    assert!(two == three);
    let _ = Vec2::ZERO;
}
