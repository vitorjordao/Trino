//! Shadows on the PC renderer, per tier (ADR 0025): the N64 profile draws
//! blobs (and projects its two hero casters), the 3DS profile projects
//! every shape onto its receiver once per pixel, the native profile draws a
//! shadow map. Goldens per tier (`tests/golden/pc/shadows_*`, `cargo xtask
//! test --bless`), plus property checks that hold on any GPU.
//!
//! `TRINO_SHADOW_SHOTS=<dir>` also writes each tier's frames, for review.

use std::path::PathBuf;

use trino_asset_pipeline::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, Light};
use trino_core::{
    Color, Material, ModelId, ModelParams, QualitySettings, Renderer, ShadowCast, ShadowPlane,
    ShadowSetting, ShadowSettings, ShadowTier, Transform3, Vec3,
};
use trino_platform_pc::{PcRenderer, ShadowView, SimProfile};

const FLOOR: ModelId = ModelId(0x5701);
const STEP: ModelId = ModelId(0x5702);
const TOWER: ModelId = ModelId(0x5703);
const BALL: ModelId = ModelId(0x5704);
const PROP: ModelId = ModelId(0x5705);

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

fn setup(r: &mut PcRenderer, floor: f32) {
    r.upload_mesh(
        FLOOR,
        prim(
            Shape::Plane {
                size: [floor, floor],
            },
            [214, 202, 172, 255],
            Pivot::Center,
        ),
    );
    r.upload_mesh(
        STEP,
        prim(
            Shape::Box {
                size: [2.0, 0.6, 2.0],
            },
            [150, 172, 212, 255],
            Pivot::Bottom,
        ),
    );
    r.upload_mesh(
        TOWER,
        prim(
            Shape::Box {
                size: [0.6, 1.6, 0.6],
            },
            [235, 120, 90, 255],
            Pivot::Bottom,
        ),
    );
    r.upload_mesh(
        BALL,
        prim(
            Shape::Sphere { radius: 0.35 },
            [120, 210, 130, 255],
            Pivot::Center,
        ),
    );
    r.upload_mesh(
        PROP,
        prim(
            Shape::Box {
                size: [0.4, 0.4, 0.4],
            },
            [240, 220, 90, 255],
            Pivot::Bottom,
        ),
    );
}

const CAMERA: Camera3 = Camera3 {
    eye: Vec3::new(0.0, 4.2, -6.5),
    target: Vec3::new(0.3, 0.2, 0.4),
    fov_y: 60.0 * core::f32::consts::PI / 180.0,
};

const SUN: Light = Light {
    dir: Vec3::new(0.45, -0.8, 0.35),
    ambient: 0.45,
    color: Color::rgb(255, 246, 228),
    ambient_color: Color::rgb(190, 205, 240),
    fog: trino_core::Fog::NONE,
};

const TOWER_AT: Vec3 = Vec3::new(-1.2, 0.0, 0.6);
const BALL_AT: Vec3 = Vec3::new(0.3, 0.9, -0.9);
const PROP_AT: Vec3 = Vec3::new(2.2, 0.6, 1.0);

fn receivers() -> [ShadowPlane; 2] {
    [
        ShadowPlane::horizontal(Vec3::ZERO, 6.0, 6.0),
        ShadowPlane::top_of(Vec3::new(1.2, 0.0, 0.0), Vec3::new(3.2, 0.6, 2.0)),
    ]
}

/// The scene: the floor, the step, a tower (its shape), a ball in the air
/// (its shape, a blob on the N64) and a prop on the step (a blob
/// everywhere), with `shadows` (`None`: never call `set_shadows`).
fn frame(r: &mut PcRenderer, shadows: Option<&ShadowSettings>) -> Vec<u8> {
    r.begin_frame(Color::rgb(140, 175, 220));
    r.set_camera(&CAMERA);
    r.set_light(&SUN);
    if let Some(s) = shadows {
        r.set_shadows(Some(s));
    }
    r.set_shadow_receivers(&receivers());
    let at = |p: Vec3| Transform3 {
        position: p,
        ..Default::default()
    };
    let plain = ModelParams::DEFAULT;
    r.draw_model(FLOOR, &at(Vec3::ZERO), Material::VertexLit, &plain);
    r.draw_model(
        STEP,
        &at(Vec3::new(2.2, 0.0, 1.0)),
        Material::VertexLit,
        &plain,
    );
    let cast = |shadow| ModelParams {
        shadow,
        ..ModelParams::DEFAULT
    };
    r.draw_model(
        TOWER,
        &at(TOWER_AT),
        Material::VertexLit,
        &cast(ShadowCast::Mesh),
    );
    r.draw_model(
        BALL,
        &at(BALL_AT),
        Material::VertexLit,
        &cast(ShadowCast::auto(0.35)),
    );
    r.draw_model(
        PROP,
        &at(PROP_AT),
        Material::VertexLit,
        &cast(ShadowCast::Blob { radius: 0.5 }),
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

fn luma(c: [u8; 3]) -> i32 {
    (c[0] as i32 * 3 + c[1] as i32 * 6 + c[2] as i32) / 10
}

/// Where the light's ray through `p` meets the plane `y = h`.
fn shadow_of(p: Vec3, h: f32) -> Vec3 {
    let l = SUN.dir;
    let t = (h - p.y) / l.y;
    Vec3::new(p.x + l.x * t, h, p.z + l.z * t)
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
    if let Ok(dir) = std::env::var("TRINO_SHADOW_SHOTS") {
        let (w, h) = r.internal_size();
        write_png(&PathBuf::from(dir).join(format!("{name}.png")), w, h, img);
    }
}

const TIERS: [(SimProfile, &str, u8, ShadowTier); 3] = [
    (SimProfile::Native, "shadows_native", 4, ShadowTier::Map),
    (SimProfile::N3ds, "shadows_n3ds", 3, ShadowTier::Planar),
    (SimProfile::N64, "shadows_n64", 9, ShadowTier::Blob),
];

#[test]
fn each_tier_matches_its_golden() {
    let _gpu = gpu();
    for (profile, name, tol, tier) in TIERS {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r, 12.0);
        let img = frame(&mut r, Some(&ShadowSettings::DEFAULT));
        assert_eq!(r.shadow_tier(), tier, "{profile:?}");
        shots(&r, name, &img);
        let off = frame(&mut r, None);
        shots(&r, &format!("{name}_off"), &off);
        golden(&r, &img, name, tol, 1024);
    }
}

#[test]
fn shadows_darken_the_ground_under_each_caster_on_every_tier() {
    let _gpu = gpu();
    for (profile, name, _, _) in TIERS {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r, 12.0);
        let on = frame(&mut r, Some(&ShadowSettings::DEFAULT));
        let stats = r.stats().shadows;
        let off = frame(&mut r, None);
        let darker = |p: Vec3, what: &str| {
            let (a, b) = (luma(px(&r, &on, p)), luma(px(&r, &off, p)));
            assert!(
                a + 25 < b,
                "{name}: no shadow {what} ({a} vs {b} unshadowed)"
            );
        };
        // The tower's shadow, halfway along it (its shape, every tier: the
        // N64 projects its first two `Mesh` casters).
        darker(
            shadow_of(TOWER_AT + Vec3::new(0.0, 1.0, 0.0), 0.0),
            "of the tower",
        );
        // The prop's blob on the step, right under it.
        darker(PROP_AT + Vec3::new(0.0, 0.0, -0.3), "of the prop");
        // The ball's: its shape along the light, or a blob straight down.
        let ball = if profile == SimProfile::N64 {
            Vec3::new(BALL_AT.x, 0.0, BALL_AT.z - 0.1)
        } else {
            shadow_of(BALL_AT, 0.0)
        };
        darker(ball, "of the ball");
        // Lit ground away from every caster is untouched.
        let open = Vec3::new(-2.6, 0.0, -2.0);
        let (a, b) = (luma(px(&r, &on, open)), luma(px(&r, &off, open)));
        assert!(
            (a - b).abs() <= 2,
            "{name}: the open ground changed ({a} vs {b})"
        );
        // Counted.
        let s = stats;
        assert!(s.blobs + s.planar + s.mapped >= 3, "{name}: {s:?}");
    }
}

#[test]
fn shadows_are_cleared_at_begin_frame_and_follow_the_view() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::N3ds) else {
        return;
    };
    setup(&mut r, 12.0);
    let on = frame(&mut r, Some(&ShadowSettings::DEFAULT));
    let none = frame(&mut r, None);
    assert_ne!(on, none);
    assert_eq!(r.stats().shadows, Default::default(), "cleared");
    r.set_shadow_view(ShadowView::Off);
    assert_eq!(frame(&mut r, Some(&ShadowSettings::DEFAULT)), none);
    r.set_shadow_view(ShadowView::Debug);
    let debug = frame(&mut r, None);
    let p = shadow_of(TOWER_AT + Vec3::new(0.0, 1.0, 0.0), 0.0);
    let c = px(&r, &debug, p);
    assert!(c[0] > 150 && c[1] < 90, "the debug color: {c:?}");
    r.set_shadow_view(ShadowView::Game);
    assert_eq!(frame(&mut r, Some(&ShadowSettings::DEFAULT)), on);
    // A game asking for blobs gets blobs everywhere.
    let blobs = ShadowSettings {
        tier_max: ShadowTier::Blob,
        ..ShadowSettings::DEFAULT
    };
    frame(&mut r, Some(&blobs));
    assert_eq!(r.shadow_tier(), ShadowTier::Blob);
}

/// Two casters whose planar shadows overlap: the 3DS draws the overlap
/// once (its stencil; the PC's alpha mark), the N64 blends both.
#[test]
fn planar_shadows_draw_once_per_pixel_on_the_3ds_and_twice_on_the_n64() {
    let _gpu = gpu();
    for (profile, once) in [(SimProfile::N3ds, true), (SimProfile::N64, false)] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r, 12.0);
        let draw = |r: &mut PcRenderer, n: usize| {
            r.begin_frame(Color::rgb(140, 175, 220));
            r.set_camera(&CAMERA);
            r.set_light(&SUN);
            r.set_shadows(Some(&ShadowSettings::DEFAULT));
            r.set_shadow_receivers(&receivers());
            r.draw_model(
                FLOOR,
                &Transform3::default(),
                Material::VertexLit,
                &ModelParams::DEFAULT,
            );
            for k in 0..n {
                r.draw_model(
                    TOWER,
                    &Transform3 {
                        position: TOWER_AT + Vec3::new(0.25 * k as f32, 0.0, 0.0),
                        ..Default::default()
                    },
                    Material::VertexLit,
                    &ModelParams {
                        shadow: ShadowCast::Mesh,
                        ..ModelParams::DEFAULT
                    },
                );
            }
            r.end_frame();
            r.read_offscreen()
        };
        let one = draw(&mut r, 1);
        let two = draw(&mut r, 2);
        // Where both shadows fall (the first's, moved halfway).
        let p = shadow_of(TOWER_AT + Vec3::new(0.2, 1.2, 0.0), 0.0);
        let (a, b) = (luma(px(&r, &one, p)), luma(px(&r, &two, p)));
        if once {
            assert!(
                (a - b).abs() <= 2,
                "{profile:?}: the overlap darkened ({a} → {b})"
            );
        } else {
            assert!(
                b + 10 < a,
                "{profile:?}: the overlap should blend twice ({a} → {b})"
            );
        }
    }
}

/// A long floor seen low, blobs from 4 m to 120 m away: every blob shows
/// (the depth offset keeps it in front of its floor) on the N64 profile,
/// whose offset follows the RDP's 18-bit compressed z. The console's own
/// check is the emulator golden (`tests/golden/n64/shadows_far.png`).
#[test]
fn blobs_show_at_every_distance_on_a_large_floor() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::N64) else {
        return;
    };
    setup(&mut r, 400.0);
    let cam = Camera3 {
        eye: Vec3::new(0.0, 3.0, -4.0),
        target: Vec3::new(0.0, 0.6, 40.0),
        fov_y: 50.0 * core::f32::consts::PI / 180.0,
    };
    // Each a pixel or more tall on the 240-line picture.
    let at = [4.0f32, 9.0, 18.0, 35.0, 60.0, 100.0];
    let draw = |r: &mut PcRenderer, shadows: bool| {
        r.begin_frame(Color::rgb(140, 175, 220));
        r.set_camera(&cam);
        r.set_light(&SUN);
        if shadows {
            r.set_shadows(Some(&ShadowSettings {
                distance: 200.0,
                ..ShadowSettings::DEFAULT
            }));
            r.set_shadow_receivers(&[ShadowPlane::ground(0.0)]);
        }
        r.draw_model(
            FLOOR,
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        for (k, z) in at.iter().enumerate() {
            let x = if k % 2 == 0 { -0.3 } else { 0.3 } * z * 0.2;
            r.draw_model(
                BALL,
                // Small and low, on the ground: the blob is all it casts,
                // and the ball stays clear of the probe under its center.
                &Transform3 {
                    position: Vec3::new(x, 0.08, *z),
                    scale: Vec3::new(0.1, 0.1, 0.1),
                    ..Default::default()
                },
                Material::VertexLit,
                &ModelParams {
                    shadow: ShadowCast::Blob {
                        radius: 0.5 + z * 0.06,
                    },
                    ..ModelParams::DEFAULT
                },
            );
        }
        r.end_frame();
        r.read_offscreen()
    };
    let on = draw(&mut r, true);
    let off = draw(&mut r, false);
    shots(&r, "shadows_far_n64", &on);
    let (w, h) = r.internal_size();
    let view = cam.view();
    let half = cam.fov_y * 0.5;
    let focal = half.cos() / half.sin() * h as f32 * 0.5;
    for (k, z) in at.iter().enumerate() {
        let x = if k % 2 == 0 { -0.3 } else { 0.3 } * z * 0.2;
        let v = view.transform_point(Vec3::new(x, 0.0, *z));
        let sx = (w as f32 * 0.5 + v.x * focal / v.z) as i32;
        let sy = (h as f32 * 0.5 - v.y * focal / v.z) as i32;
        // Far away a blob is under a pixel tall: the pixel around where it
        // lands that it darkens most.
        let (mut a, mut b) = (0, 0);
        for y in sy - 1..=sy + 1 {
            for x in sx - 2..=sx + 2 {
                let i = ((y as u32 * w + x as u32) * 4) as usize;
                let (p, q) = (
                    luma([on[i], on[i + 1], on[i + 2]]),
                    luma([off[i], off[i + 1], off[i + 2]]),
                );
                if q - p > b - a {
                    (a, b) = (p, q);
                }
            }
        }
        assert!(a + 20 < b, "the blob {z} m away doesn't show ({a} vs {b})");
    }
}

/// The native profile's quality preset sizes and filters the map; `Off`
/// keeps the casters grounded with planar shadows, no map.
#[test]
fn the_quality_preset_caps_the_native_tier() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r, 12.0);
    r.apply_quality(&QualitySettings {
        shadows: ShadowSetting::Off,
        ..QualitySettings::HIGH
    });
    let off = frame(&mut r, Some(&ShadowSettings::DEFAULT));
    assert_eq!(r.shadow_tier(), ShadowTier::Planar);
    shots(&r, "shadows_native_quality_off", &off);
    let s = r.stats().shadows;
    assert!(s.planar >= 2 && s.mapped == 0, "{s:?}");
    r.apply_quality(&QualitySettings::HIGH);
    frame(&mut r, Some(&ShadowSettings::DEFAULT));
    assert_eq!(r.shadow_tier(), ShadowTier::Map);
}

/// The map's composite reads the scene's depth however it is drawn:
/// multisampled, at a render scale, both. The same shadows land, and the
/// lit faces of the casters themselves stay clear (no acne).
#[test]
fn the_map_shadows_with_msaa_and_a_render_scale() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r, 12.0);
    for (msaa, scale) in [(false, 1.0), (true, 1.0), (false, 0.6), (true, 0.6)] {
        r.set_msaa(msaa);
        r.set_render_scale(scale);
        let on = frame(&mut r, Some(&ShadowSettings::DEFAULT));
        assert_eq!(r.frame_times().shadow_maps, 1, "msaa {msaa} scale {scale}");
        assert_eq!(r.shadow_tier(), ShadowTier::Map);
        let off = frame(&mut r, None);
        assert_eq!(r.frame_times().shadow_maps, 0);
        let what = format!("msaa {msaa} scale {scale}");
        shots(&r, &format!("shadows_native_msaa{msaa}_scale{scale}"), &on);
        let tower = shadow_of(TOWER_AT + Vec3::new(0.0, 1.0, 0.0), 0.0);
        let (a, b) = (luma(px(&r, &on, tower)), luma(px(&r, &off, tower)));
        assert!(a + 25 < b, "{what}: the tower's shadow ({a} vs {b})");
        // The tower's face toward the camera and the light.
        let face = TOWER_AT + Vec3::new(0.0, 0.8, -0.3);
        let (a, b) = (luma(px(&r, &on, face)), luma(px(&r, &off, face)));
        assert!(
            (a - b).abs() <= 3,
            "{what}: the lit face changed ({a} vs {b})"
        );
    }
    r.set_msaa(false);
    r.set_render_scale(1.0);
}

/// Contact hardening and distance attenuation on the map (#86): one cube,
/// resting on the floor, then 0.5 and 2 units above it, under a slanted
/// sun and a camera looking down on the shadow's free side.
mod contact {
    use super::*;

    pub const CUBE: ModelId = ModelId(0x5706);

    pub const CAMERA: Camera3 = Camera3 {
        eye: Vec3::new(1.1, 4.6, -1.6),
        target: Vec3::new(1.1, 0.0, 0.25),
        fov_y: 55.0 * core::f32::consts::PI / 180.0,
    };

    /// Down at 37° from the vertical toward +x: a point `h` up casts
    /// `0.75·h` farther along x.
    pub const SUN: Light = Light {
        dir: Vec3::new(0.6, -0.8, 0.0),
        ambient: 0.45,
        color: Color::rgb(255, 246, 228),
        ambient_color: Color::rgb(190, 205, 240),
        fog: trino_core::Fog::NONE,
    };

    pub const ON: ShadowSettings = ShadowSettings {
        soft_growth: 0.1,
        fade_distance: 4.0,
        ..ShadowSettings::DEFAULT
    };

    pub const OFF: ShadowSettings = ShadowSettings {
        soft_growth: 0.0,
        fade_distance: 0.0,
        ..ON
    };

    pub fn setup(r: &mut PcRenderer) {
        super::setup(r, 12.0);
        r.upload_mesh(
            CUBE,
            prim(
                Shape::Box {
                    size: [0.6, 0.6, 0.6],
                },
                [235, 120, 90, 255],
                Pivot::Bottom,
            ),
        );
    }

    /// The floor and the cube `h` above it; `casts`: its shadow, or the
    /// same frame without (the baseline the probes compare with).
    pub fn frame(r: &mut PcRenderer, s: &ShadowSettings, h: f32, casts: bool) -> Vec<u8> {
        r.begin_frame(Color::rgb(140, 175, 220));
        r.set_camera(&CAMERA);
        r.set_light(&SUN);
        r.set_shadows(Some(s));
        r.set_shadow_receivers(&[ShadowPlane::ground(0.0)]);
        r.draw_model(
            FLOOR,
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        r.draw_model(
            CUBE,
            &Transform3 {
                position: Vec3::new(0.0, h, 0.0),
                ..Default::default()
            },
            Material::VertexLit,
            &ModelParams {
                shadow: if casts {
                    ShadowCast::Mesh
                } else {
                    ShadowCast::None
                },
                ..ModelParams::DEFAULT
            },
        );
        r.end_frame();
        r.read_offscreen()
    }

    /// The blocker distance along the light at the probe line.
    pub fn dist(h: f32) -> f32 {
        (h + 0.16) / 0.8
    }

    pub fn luma_at(r: &PcRenderer, img: &[u8], p: Vec3) -> f32 {
        let (w, h) = r.internal_size();
        let v = CAMERA.view().transform_point(p);
        let half = CAMERA.fov_y * 0.5;
        let focal = half.cos() / half.sin() * h as f32 * 0.5;
        let x = (w as f32 * 0.5 + v.x * focal / v.z) as u32;
        let y = (h as f32 * 0.5 - v.y * focal / v.z) as u32;
        let i = ((y * w + x) * 4) as usize;
        luma([img[i], img[i + 1], img[i + 2]]) as f32
    }

    /// The shadow of the cube `h` up: how much it darkens the floor in
    /// its middle (luma), and the width of its edge along z, 10% to 90%
    /// of that (meters). The probe line is 0.12 past the cube's foot, where
    /// the light grazes its lit face 0.16 above its bottom: `dist(h)` from
    /// the floor along the light.
    pub fn measure(r: &mut PcRenderer, s: &ShadowSettings, h: f32) -> (f32, f32) {
        let on = frame(r, s, h, true);
        let off = frame(r, s, h, false);
        let x = 0.75 * h + 0.42;
        let dark = |z: f32| {
            let p = Vec3::new(x, 0.0, z);
            luma_at(r, &off, p) - luma_at(r, &on, p)
        };
        // The middle, averaged over a few probes.
        let mid = (0..5).map(|k| dark(-0.1 + 0.05 * k as f32)).sum::<f32>() / 5.0;
        let (mut z90, mut z10) = (None, None);
        let mut z = 0.0f32;
        while z < 0.8 {
            let d = dark(z);
            if d >= 0.9 * mid {
                z90 = Some(z);
            }
            if z10.is_none() && d <= 0.1 * mid {
                z10 = Some(z);
            }
            z += 0.005;
        }
        let (z90, z10) = (z90.expect("a dark middle"), z10.expect("an edge"));
        (mid, z10 - z90)
    }
}

#[test]
fn contact_shadows_harden_at_contact_and_fade_with_height() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    contact::setup(&mut r);
    if let Ok(dir) = std::env::var("TRINO_SHADOW_SHOTS") {
        for h in [0.0, 0.5, 1.0, 2.0] {
            for (tag, s) in [("after", contact::ON), ("before", contact::OFF)] {
                let img = contact::frame(&mut r, &s, h, true);
                let (w, hh) = r.internal_size();
                write_png(
                    &PathBuf::from(&dir).join(format!("contact_h{h}_{tag}.png")),
                    w,
                    hh,
                    &img,
                );
            }
        }
    }
    let m: Vec<(f32, f32)> = [0.0, 0.5, 2.0]
        .iter()
        .map(|&h| contact::measure(&mut r, &contact::ON, h))
        .collect();
    eprintln!("contact on: (darkening, edge m) at 0, 0.5, 2: {m:?}");
    let (touch, low, high) = (m[0], m[1], m[2]);
    // Darkest at contact, fading with height (linearly in the distance
    // along the light, gone at `fade_distance`).
    assert!(touch.0 > low.0 + 3.0 && low.0 > high.0 + 6.0, "{m:?}");
    assert!(high.0 > 3.0, "still a shadow 2 up: {m:?}");
    let lift = |h: f32| 1.0 - contact::dist(h) / contact::ON.fade_distance;
    // At most the fade's ratio: the wider filter and the cube's higher
    // texels in the blocker mean only lighten it further.
    assert!(
        high.0 / touch.0 <= lift(2.0) / lift(0.0) + 0.05,
        "the fade's ratio: {m:?}"
    );
    // Sharpest at contact, softer above.
    assert!(touch.1 + 0.03 < low.1 && low.1 <= high.1 + 0.01, "{m:?}");
    assert!(touch.1 < 0.7 * high.1, "{m:?}");

    // Without contact hardening (ADR 0025's even filter, the default) the
    // same probes see the same shadow at every height: the test above
    // fails on it.
    let flat: Vec<(f32, f32)> = [0.0, 2.0]
        .iter()
        .map(|&h| contact::measure(&mut r, &contact::OFF, h))
        .collect();
    eprintln!("contact off: {flat:?}");
    assert!((flat[0].0 - flat[1].0).abs() <= 3.0, "{flat:?}");
    assert!((flat[0].1 - flat[1].1).abs() <= 0.02, "{flat:?}");
    // Near contact the shadow keeps most of the even filter's strength
    // and sharpness (the blocker mean also sees the cube's higher texels).
    assert!(touch.0 >= 0.75 * flat[0].0, "{m:?} vs {flat:?}");
    assert!(
        touch.1 <= 1.6 * flat[0].1 && high.1 > 2.0 * flat[1].1,
        "{m:?} vs {flat:?}"
    );
}

/// Contact hardening with MSAA and a render scale: the composite's
/// blocker search reads the same map; the shadow still fades with height.
#[test]
fn contact_shadows_hold_with_msaa_and_a_render_scale() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    contact::setup(&mut r);
    for (msaa, scale) in [(true, 1.0), (false, 0.6), (true, 0.6)] {
        r.set_msaa(msaa);
        r.set_render_scale(scale);
        let touch = contact::measure(&mut r, &contact::ON, 0.0);
        let high = contact::measure(&mut r, &contact::ON, 2.0);
        assert!(
            touch.0 > high.0 + 6.0 && touch.1 < high.1,
            "msaa {msaa} scale {scale}: {touch:?} vs {high:?}"
        );
    }
    r.set_msaa(false);
    r.set_render_scale(1.0);
}
