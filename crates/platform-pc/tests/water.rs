//! Water surfaces on the PC renderer, per tier (ADR 0015): the native
//! profile shades them per pixel (normals, Fresnel, glint, depth foam), the
//! N64 and 3DS profiles per vertex with the consoles' layer blend. Goldens
//! per tier (`tests/golden/pc/water_*`, `cargo xtask test --bless`), plus
//! property checks that hold on any GPU.
//!
//! `TRINO_WATER_SHOTS=<dir>` also writes each tier's frames, for review.

use std::path::PathBuf;

use trino_asset_pipeline::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, Light};
use trino_core::{
    Color, DetailTexture, Material, ModelId, ModelParams, Renderer, Ripple, Shading, SpriteId,
    Transform3, Vec2, Vec3, Water,
};
use trino_platform_pc::{PcRenderer, SimProfile};

const FLOOR: ModelId = ModelId(0x2001);
const SHEET: ModelId = ModelId(0x2002);
const POST: ModelId = ModelId(0x2003);
const LAYER: SpriteId = SpriteId(0x2101);
const GRAIN: SpriteId = SpriteId(0x2102);

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/pc")
}

/// One GPU test at a time in this binary (see `local_lights.rs`).
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

/// A `cells`×`cells` grid at height `y`, `size` wide, facing up, in one
/// color, with planar UVs (one repeat per 2 m) when `uv`.
fn grid(size: f32, cells: u32, y: f32, color: [u8; 4], uv: bool) -> Vec<u8> {
    let n = cells + 1;
    let mut blob = Vec::new();
    blob.extend_from_slice(b"TMDL");
    blob.extend_from_slice(&(n * n).to_le_bytes());
    blob.extend_from_slice(&(cells * cells * 6).to_le_bytes());
    let step = size / cells as f32;
    let at = |i: u32| i as f32 * step - size / 2.0;
    for z in 0..n {
        for x in 0..n {
            for v in [at(x), y, at(z)] {
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
            for k in [a, c, b, a, d, c] {
                blob.extend_from_slice(&k.to_le_bytes());
            }
        }
    }
    if uv {
        let mut body = Vec::new();
        for z in 0..n {
            for x in 0..n {
                for v in [at(x) * 0.5, at(z) * 0.5] {
                    body.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        blob.extend_from_slice(b"TXUV");
        blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
        blob.extend_from_slice(&body);
    }
    blob.extend_from_slice(b"BNDS");
    blob.extend_from_slice(&16u32.to_le_bytes());
    for v in [0.0f32, y, 0.0, size * 0.75] {
        blob.extend_from_slice(&v.to_le_bytes());
    }
    blob
}

/// A 32×32 tiling grayscale layer.
fn layer(f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    let mut px = Vec::new();
    for y in 0..32 {
        for x in 0..32 {
            let g = f(x, y);
            px.extend_from_slice(&[g, g, g, 255]);
        }
    }
    px
}

fn setup(r: &mut PcRenderer) {
    r.upload_mesh(FLOOR, grid(12.0, 12, -0.8, [220, 40, 30, 255], false));
    r.upload_mesh(SHEET, grid(8.0, 16, 0.0, [40, 120, 230, 150], true));
    // A post standing in the water: a tall, narrow box (prim) from -0.8 up.
    r.upload_mesh(
        POST,
        Primitive {
            shape: Shape::Box {
                size: [0.6, 1.6, 0.6],
            },
            color: [240, 240, 240, 255],
            pivot: Pivot::Bottom,
            ..Primitive::default()
        }
        .build()
        .encode()
        .unwrap(),
    );
    r.upload_sprite(
        LAYER,
        32,
        32,
        &layer(|x, y| 190 + ((x * 7 + y * 3) % 32) as u8 * 2),
    );
    r.upload_sprite(
        GRAIN,
        32,
        32,
        &layer(|x, y| if (x / 4 + y / 4) % 2 == 0 { 255 } else { 150 }),
    );
}

const CAMERA: Camera3 = Camera3 {
    eye: Vec3::new(0.0, 3.5, -6.0),
    target: Vec3::new(0.0, -0.3, 0.0),
    fov_y: 60.0 * core::f32::consts::PI / 180.0,
};

const SUN: Light = Light {
    dir: Vec3::new(0.2, -0.8, 0.55),
    ambient: 0.45,
    color: Color::rgb(255, 246, 228),
    ambient_color: Color::rgb(190, 210, 255),
    fog: trino_core::Fog::NONE,
};

const WATER: Water = Water {
    flow_speed: Vec2::new(0.0, 0.3),
    detail_flow: Vec2::new(0.1, -0.4),
    wave_amplitude: 0.06,
    wave_frequency: 0.6,
    ripple_responsiveness: 1.0,
};

fn water_params() -> ModelParams {
    ModelParams {
        texture: Some(LAYER),
        detail: Some(DetailTexture::new(GRAIN, 1)),
        gloss: 170,
        water: Some(WATER),
        ..ModelParams::DEFAULT
    }
}

const RIPPLE: Ripple = Ripple {
    pos: Vec2::new(-1.6, 1.0),
    age: 0.5,
    strength: 0.12,
};

/// The scene: the floor, the post, the water (drawn first: the batch puts
/// it last), at `time` with `ripples` (`None`: never call `set_water`).
fn frame(r: &mut PcRenderer, water: Option<(f32, &[Ripple])>, params: &ModelParams) -> Vec<u8> {
    r.begin_frame(Color::rgb(130, 170, 220));
    r.set_camera(&CAMERA);
    r.set_light(&SUN);
    if let Some((time, ripples)) = water {
        r.set_water(time, ripples);
    }
    let at = |p: Vec3| Transform3 {
        position: p,
        ..Default::default()
    };
    r.draw_model(SHEET, &at(Vec3::ZERO), Material::VertexLit, params);
    r.draw_model(
        FLOOR,
        &at(Vec3::ZERO),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    r.draw_model(
        POST,
        &at(Vec3::new(1.2, -0.8, 0.5)),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
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
    if let Ok(dir) = std::env::var("TRINO_WATER_SHOTS") {
        let (w, h) = r.internal_size();
        write_png(&PathBuf::from(dir).join(format!("{name}.png")), w, h, img);
    }
}

const TIERS: [(SimProfile, &str, u8); 3] = [
    (SimProfile::Native, "water_native", 3),
    (SimProfile::N3ds, "water_n3ds", 3),
    (SimProfile::N64, "water_n64", 9),
];

#[test]
fn each_tier_matches_its_golden() {
    let _gpu = gpu();
    for (profile, name, tol) in TIERS {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        let img = frame(&mut r, Some((1.25, &[RIPPLE])), &water_params());
        shots(&r, name, &img);
        golden(&r, &img, name, tol, 1024);
    }
}

#[test]
fn the_floor_shows_through_on_every_tier() {
    let _gpu = gpu();
    for (profile, _, _) in TIERS {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        let img = frame(&mut r, Some((0.0, &[])), &water_params());
        // Open water over the red floor: red and blue together.
        let c = px(&r, &img, Vec3::new(-0.5, 0.0, -0.5));
        assert!(
            c[0] > 50 && c[0] < 230 && c[2] > 60,
            "{profile:?}: {c:?} (floor through water expected)"
        );
        // Without the water, the floor is plain red.
        let dry = frame(
            &mut r,
            Some((0.0, &[])),
            &ModelParams {
                water: None,
                tint: Color::rgba(255, 255, 255, 0),
                ..water_params()
            },
        );
        let d = px(&r, &dry, Vec3::new(-0.5, -0.8, -0.5));
        assert!(d[0] > 120 && d[2] < 80, "{profile:?}: floor {d:?}");
    }
}

#[test]
fn ripples_ring_and_the_swell_moves_on_every_tier() {
    let _gpu = gpu();
    for (profile, name, _) in TIERS {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        let still = frame(&mut r, Some((0.0, &[])), &water_params());
        let rippled = frame(&mut r, Some((0.0, &[RIPPLE])), &water_params());
        shots(&r, &format!("{name}_ripple"), &rippled);
        // Probes on the ring (radius 0.8 m at age 0.5 s).
        let ring: Vec<Vec3> = (0..12)
            .map(|k| {
                let a = k as f32 / 12.0 * core::f32::consts::TAU;
                Vec3::new(
                    RIPPLE.pos.x + 0.8 * a.cos(),
                    0.0,
                    RIPPLE.pos.y + 0.8 * a.sin(),
                )
            })
            .collect();
        let differ = |r: &PcRenderer, a: &[u8], b: &[u8], probes: &[Vec3]| {
            probes
                .iter()
                .filter(|&&p| {
                    let (x, y) = (px(r, a, p), px(r, b, p));
                    (0..3).map(|k| x[k].abs_diff(y[k]) as u32).sum::<u32>() > 18
                })
                .count()
        };
        assert!(
            differ(&r, &still, &rippled, &ring) >= 4,
            "{profile:?}: the ripple left no ring"
        );
        let later = frame(&mut r, Some((0.8, &[])), &water_params());
        let open: Vec<Vec3> = (0..12)
            .map(|k| Vec3::new(-2.5 + k as f32 * 0.4, 0.0, -1.0))
            .collect();
        assert!(
            differ(&r, &still, &later, &open) >= 4,
            "{profile:?}: nothing moved in 0.8 s"
        );
    }
}

#[test]
fn foam_and_glint_are_the_per_pixel_tier_only() {
    let _gpu = gpu();
    // Where the water meets the post's front face: foam per pixel (the
    // scene's depth under the surface is a few cm), plain water per vertex.
    let edge = Vec3::new(1.2, 0.0, 0.5 - 0.33);
    let open = Vec3::new(1.2, 0.0, -0.9);
    let luma = |c: [u8; 3]| (c[0] as i32 * 3 + c[1] as i32 * 6 + c[2] as i32) / 10;
    for (profile, foamy) in [
        (SimProfile::Native, true),
        (SimProfile::N3ds, false),
        (SimProfile::N64, false),
    ] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        let calm = ModelParams {
            water: Some(Water {
                wave_amplitude: 0.0,
                ..WATER
            }),
            ..water_params()
        };
        let img = frame(&mut r, Some((0.0, &[])), &calm);
        let (e, o) = (luma(px(&r, &img, edge)), luma(px(&r, &img, open)));
        if foamy {
            assert!(e > o + 30, "{profile:?}: no foam at the post ({e} vs {o})");
        } else {
            assert!(
                e < o + 30,
                "{profile:?}: foam where the tier has none ({e} vs {o})"
            );
        }
    }
}

#[test]
fn water_state_is_cleared_every_frame() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    let never = frame(&mut r, None, &water_params());
    let zero = frame(&mut r, Some((0.0, &[])), &water_params());
    assert!(never == zero, "no set_water = time 0, no ripples");
    let _ = frame(&mut r, Some((2.0, &[RIPPLE])), &water_params());
    let after = frame(&mut r, None, &water_params());
    assert!(never == after, "last frame's clock and ripples leaked");
}

#[test]
fn water_is_drawn_after_the_batch_and_the_2d_stays_on_top() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    // The post above the water: unblended (the water is drawn after it and
    // tests against its depth); a HUD rectangle over the water stays solid.
    r.begin_frame(Color::BLACK);
    r.set_camera(&CAMERA);
    r.set_light(&SUN);
    let at = |p: Vec3| Transform3 {
        position: p,
        ..Default::default()
    };
    r.draw_model(SHEET, &at(Vec3::ZERO), Material::VertexLit, &water_params());
    r.draw_model(
        POST,
        &at(Vec3::new(1.2, -0.8, 0.5)),
        Material::VertexLit,
        &ModelParams {
            shading: Shading::Unlit,
            ..ModelParams::DEFAULT
        },
    );
    let (hx, hy) = project(&r, Vec3::new(-1.0, 0.0, -1.0));
    r.fill_rect(
        trino_core::Rect::new(hx as f32 - 4.0, hy as f32 - 4.0, 8.0, 8.0),
        Color::rgb(0, 255, 0),
    );
    r.end_frame();
    let img = r.read_offscreen();
    let top = px(&r, &img, Vec3::new(1.2, 0.6, 0.5 - 0.31));
    assert!(
        top.iter().all(|&c| c > 200),
        "post above the water: {top:?}"
    );
    let hud = px(&r, &img, Vec3::new(-1.0, 0.0, -1.0));
    assert_eq!(hud, [0, 255, 0], "the HUD over the water");
}

#[test]
#[should_panic(expected = "live ripples")]
fn strict_mode_refuses_more_ripples_than_the_cap() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::N3ds) else {
        panic!("live ripples (no GPU: skipped)");
    };
    r.set_strict(true);
    r.begin_frame(Color::BLACK);
    r.set_water(0.0, &[RIPPLE; 5]);
}
