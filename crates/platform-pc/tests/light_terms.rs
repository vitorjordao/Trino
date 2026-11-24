//! A two-color ambient and a rim light (`Light::ground`, `Light::rim`,
//! Bongo-Kart #181, ADR 0043 §3) on the PC renderer: the GPU T&L draws
//! what the CPU path draws (the oracle), a ground of the sky's own color
//! draws the old light, the hemisphere and the rim land where they should
//! on every profile, and the console profiles draw them on the CPU T&L.
//!
//! Evidence (`target/evidence/light-terms/`): a dusk scene with the old
//! light, the sky and ground, and both plus the rim, per profile.

use std::path::PathBuf;

use trino_asset_pipeline::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, Light};
use trino_core::{
    Color, DEFAULT_LIGHT, DynVertex, DynamicMesh, Material, ModelId, ModelParams, Renderer, Rim,
    Shading, Transform3, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

const BALL: ModelId = ModelId(0x2001);
const CRATE: ModelId = ModelId(0x2002);
const FLOOR: ModelId = ModelId(0x2003);
const POST: ModelId = ModelId(0x2004);

/// One GPU test at a time in this binary (RADV, see `local_lights.rs`).
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

fn prim(shape: Shape, color: [u8; 4], segments: u32, pivot: Pivot) -> Vec<u8> {
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

/// A `cells`×`cells` grid floor, `size` meters wide, facing up. Small
/// triangles: a big one crossing the guard band is clipped into a polygon
/// whose new vertices the CPU path colors in 3D, while the GPU path
/// interpolates over the whole triangle, so a view-dependent rim (a color
/// that changes across one triangle) differs by up to ~11/255 there.
fn grid_floor(size: f32, cells: u16, color: Color) -> Vec<u8> {
    let n = cells + 1;
    let step = size / cells as f32;
    let up = Vec3::new(0.0, 1.0, 0.0);
    let mut verts = Vec::new();
    for z in 0..n {
        for x in 0..n {
            let p = Vec3::new(
                x as f32 * step - size / 2.0,
                0.0,
                z as f32 * step - size / 2.0,
            );
            verts.push(DynVertex::new(p, up, color));
        }
    }
    let mut idx = Vec::new();
    for z in 0..cells {
        for x in 0..cells {
            let i = z * n + x;
            let (a, b, c, d) = (i, i + 1, i + n + 1, i + n);
            // Front faces CCW seen from above.
            idx.extend_from_slice(&[a, c, b, a, d, c]);
        }
    }
    let mesh = DynamicMesh::new(&verts, &idx);
    let mut blob = vec![0; mesh.tmdl_len().unwrap()];
    mesh.write_tmdl(&mut blob).unwrap();
    blob
}

fn setup(r: &mut PcRenderer) {
    r.upload_mesh(
        BALL,
        prim(
            Shape::Sphere { radius: 0.5 },
            [200, 200, 200, 255],
            24,
            Pivot::Center,
        ),
    );
    r.upload_mesh(
        CRATE,
        prim(
            Shape::Box {
                size: [0.9, 0.9, 0.9],
            },
            [190, 140, 90, 255],
            12,
            Pivot::Bottom,
        ),
    );
    r.upload_mesh(FLOOR, grid_floor(9.0, 18, Color::rgb(110, 150, 90)));
    r.upload_mesh(
        POST,
        prim(
            Shape::Capsule {
                radius: 0.25,
                height: 1.6,
            },
            [90, 110, 200, 255],
            16,
            Pivot::Bottom,
        ),
    );
}

const CAMERA: Camera3 = Camera3 {
    eye: Vec3::new(0.0, 1.6, -4.2),
    target: Vec3::new(0.0, 0.6, 0.0),
    fov_y: 60.0 * core::f32::consts::PI / 180.0,
};

/// Dusk: a low orange sun from the right, a dim blue ambient.
fn dusk() -> Light {
    Light {
        dir: Vec3::new(-0.8, -0.35, 0.3),
        ambient: 0.45,
        color: Color::rgb(255, 170, 110),
        ambient_color: Color::rgb(120, 140, 210),
        ..DEFAULT_LIGHT
    }
}

/// The sky stays the ambient color, the ground is a warm dark bounce.
fn hemisphere(l: Light) -> Light {
    Light {
        ground: Some(Color::rgb(70, 45, 25)),
        ..l
    }
}

fn rimmed(l: Light) -> Light {
    Light {
        rim: Rim {
            color: Color::rgb(200, 220, 255),
            strength: 0.8,
        },
        ..l
    }
}

fn frame(r: &mut PcRenderer, light: &Light) -> Vec<u8> {
    frame_split(r, light, light)
}

/// The floor under `level`, the objects under `objects`: how a game keeps
/// a rim on its characters (a light per group of draws, `set_light`
/// between them) without lighting the ground it sees at a grazing angle.
fn frame_split(r: &mut PcRenderer, level: &Light, objects: &Light) -> Vec<u8> {
    let lit = ModelParams {
        shading: Shading::Gouraud,
        ..ModelParams::DEFAULT
    };
    let at = |x: f32, y: f32, z: f32| Transform3 {
        position: Vec3::new(x, y, z),
        ..Transform3::default()
    };
    r.begin_frame(Color::rgb(30, 30, 50));
    r.set_camera(&CAMERA);
    r.set_light(level);
    r.draw_model(FLOOR, &at(0.0, 0.0, 1.5), Material::VertexLit, &lit);
    r.set_light(objects);
    r.draw_model(BALL, &at(-1.3, 0.9, 0.0), Material::VertexLit, &lit);
    r.draw_model(CRATE, &at(1.2, 0.0, 0.2), Material::VertexLit, &lit);
    r.draw_model(POST, &at(0.0, 0.0, 1.0), Material::VertexLit, &lit);
    r.end_frame();
    r.read_offscreen()
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn evidence(r: &PcRenderer, name: &str, img: &[u8]) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/evidence/light-terms");
    let (w, h) = r.internal_size();
    write_png(&dir.join(format!("{name}.png")), w, h, img);
}

fn px(r: &PcRenderer, img: &[u8], x: u32, y: u32) -> [i32; 3] {
    let (w, _) = r.internal_size();
    let i = ((y * w + x) * 4) as usize;
    [img[i] as i32, img[i + 1] as i32, img[i + 2] as i32]
}

fn sum(c: [i32; 3]) -> i32 {
    c[0] + c[1] + c[2]
}

/// Where a world point lands on the picture.
fn screen_of(r: &PcRenderer, p: Vec3) -> (u32, u32) {
    let (w, h) = r.internal_size();
    let view = CAMERA.view();
    let v = view.transform_point(p);
    let half = CAMERA.fov_y * 0.5;
    let focal = half.cos() / half.sin() * h as f32 * 0.5;
    (
        (w as f32 * 0.5 + v.x * focal / v.z) as u32,
        (h as f32 * 0.5 - v.y * focal / v.z) as u32,
    )
}

/// The GPU T&L evaluates the same sky, ground and rim as the CPU path
/// (`tnl_oracle`: ≤ 1/255 mean, ≤ 0.5 % of pixels past 8/255), and keeps
/// every draw (the terms are no reason to leave it).
#[test]
fn the_gpu_path_draws_the_cpu_paths_sky_ground_and_rim() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    setup(&mut r);
    for (name, light) in [
        ("hemisphere", hemisphere(dusk())),
        ("rim", rimmed(dusk())),
        ("both", rimmed(hemisphere(dusk()))),
    ] {
        // The floor under the plain dusk, the objects under the light.
        let Some(d) =
            trino_platform_pc::tnl_oracle::compare(&mut r, |r| frame_split(r, &dusk(), &light))
        else {
            return;
        };
        eprintln!("{name}: {d}");
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/evidence/pc-gpu-tnl");
        let (w, h, img) = d.side_by_side();
        write_png(&dir.join(format!("light_terms_{name}.png")), w, h, &img);
        d.assert_within(name);
        assert_eq!(d.gpu_draws, 4, "{name}: every draw on the GPU path");
    }
}

/// A ground of the sky's own color is the one-color ambient: the lit copy
/// of the T&L draws the old frame (within the rounding of its world-space
/// normals), on the CPU path and the GPU path.
#[test]
fn a_ground_the_color_of_the_sky_draws_the_old_light() {
    let _gpu = gpu();
    for gpu_path in [false, true] {
        let Some(mut r) = headless(SimProfile::Native) else {
            return;
        };
        setup(&mut r);
        r.set_hw_tnl(gpu_path);
        let old = frame(&mut r, &dusk());
        let same = frame(
            &mut r,
            &Light {
                ground: Some(dusk().ambient_color),
                ..dusk()
            },
        );
        let worst = old
            .iter()
            .zip(&same)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 1, "GPU path {gpu_path}: {worst}/255 apart");
    }
}

/// The ball under a white sky and a black ground (full ambient, no sun):
/// its top reads brighter than its middle, its middle than its bottom; a
/// rim alone (no ambient, the sun behind it) leaves its middle dark and
/// lights its edge. On the native profile (both T&L paths) and the
/// console profiles (the CPU T&L, like the consoles).
#[test]
fn the_sky_lights_tops_the_ground_bottoms_and_the_rim_edges() {
    let _gpu = gpu();
    for profile in [SimProfile::Native, SimProfile::N64, SimProfile::N3ds] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        let ball = Vec3::new(-1.3, 0.9, 0.0);
        let (bx, by) = screen_of(&r, ball);
        let (_, top) = screen_of(&r, ball + Vec3::new(0.0, 0.3, -0.35));
        let (_, bottom) = screen_of(&r, ball + Vec3::new(0.0, -0.3, -0.35));
        // Near the silhouette: a point of the sphere whose normal is 0.2
        // toward the eye (n·v ≈ 0.2, the rim's (1 - n·v)² ≈ 0.64).
        let d = (CAMERA.eye - ball).normalized();
        let side = Vec3::new(0.0, 1.0, 0.0).cross(d).normalized();
        let (edge, edge_y) = screen_of(&r, ball + (side * 0.98 + d * 0.2) * 0.5);
        let flat = Light {
            ambient: 1.0,
            ..DEFAULT_LIGHT
        };
        let sky = Light {
            ground: Some(Color::BLACK),
            ..flat
        };
        let dark = Light {
            ambient: 0.0,
            dir: (CAMERA.eye - CAMERA.target).normalized(),
            ..DEFAULT_LIGHT
        };
        let gpu_paths: &[bool] = if profile == SimProfile::Native {
            &[true, false]
        } else {
            &[false]
        };
        for &gpu_path in gpu_paths {
            r.set_hw_tnl(gpu_path);
            let tag = format!("{profile:?}, GPU path {gpu_path}");
            let img = frame(&mut r, &sky);
            let (t, m, b) = (
                sum(px(&r, &img, bx, top)),
                sum(px(&r, &img, bx, by)),
                sum(px(&r, &img, bx, bottom)),
            );
            assert!(
                t > m + 60 && m > b + 60,
                "{tag}: top {t}, middle {m}, bottom {b}"
            );
            // At the ball's middle the normal points at the eye, which looks
            // down on it: the sky's share is n.y * 0.5 + 0.5 of the plain
            // ambient's light.
            let plain = frame(&mut r, &flat);
            let base = sum(px(&r, &plain, bx, by));
            let share = (CAMERA.eye - ball).normalized().y * 0.5 + 0.5;
            let want = base as f32 * share;
            assert!(
                (m as f32 - want).abs() < 24.0,
                "{tag}: the middle takes {share:.3} of the sky ({m} of {base})"
            );
            let unlit = frame(&mut r, &dark);
            let lit = frame(&mut r, &rimmed(dark));
            let (c0, e0) = (
                sum(px(&r, &unlit, bx, by)),
                sum(px(&r, &unlit, edge, edge_y)),
            );
            let (c1, e1) = (sum(px(&r, &lit, bx, by)), sum(px(&r, &lit, edge, edge_y)));
            assert!(e0 < 40 && c0 < 40, "{tag}: no light without the rim");
            assert!(c1 < 60, "{tag}: the rim spares the middle ({c1})");
            assert!(
                e1 > c1 + 80,
                "{tag}: the rim lights the edge ({e1} vs {c1})"
            );
        }
        if profile == SimProfile::Native {
            r.set_hw_tnl(true);
        }
        let tag = format!("{profile:?}").to_lowercase();
        for (name, level, objects) in [
            ("dusk", dusk(), dusk()),
            ("dusk_hemisphere", hemisphere(dusk()), hemisphere(dusk())),
            (
                "dusk_hemisphere_rim",
                rimmed(hemisphere(dusk())),
                rimmed(hemisphere(dusk())),
            ),
            (
                "dusk_hemisphere_rim_on_objects",
                hemisphere(dusk()),
                rimmed(hemisphere(dusk())),
            ),
        ] {
            let img = frame_split(&mut r, &level, &objects);
            evidence(&r, &format!("{tag}_{name}"), &img);
        }
    }
}
