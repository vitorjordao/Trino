//! Additive draws (`ModelParams::blend = Blend::Add`, Bongo-Kart #179,
//! ADR 0043 §2) on the PC renderer: a spark adds its light to what is
//! behind it instead of covering it; two sparks give the same pixel in
//! either depth order; an additive draw writes no depth; the GPU T&L leaves
//! it to the CPU path and the frame is the same either way; the console
//! profiles add too. Evidence PNGs (sparks with and without additive, per
//! profile) in `target/evidence/additive`. GPU-gated like the goldens.

use std::path::{Path, PathBuf};

use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::{
    Blend, Color, DynVertex, DynamicMesh, Mat34, Material, ModelId, ModelParams, Renderer, Screen,
    Shading, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

const WALL: ModelId = ModelId(51);
const SPARK: ModelId = ModelId(52);
const BURST: ModelId = ModelId(53);
const BLOCK: ModelId = ModelId(54);
const W: u32 = 320;
const H: u32 = 240;
const WALL_RGB: [u8; 3] = [30, 30, 50];
const SPARK_RGB: [u8; 3] = [200, 100, 20];
const BLOCK_RGB: [u8; 3] = [60, 160, 60];

/// One GPU test at a time (parallel headless devices crash some drivers).
fn gpu() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn headless(profile: SimProfile) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(mut r) => {
            if profile == SimProfile::Native {
                r.set_internal_size(W, H);
            }
            r.set_hw_tnl(true);
            upload(&mut r);
            Some(r)
        }
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn evidence(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/evidence/additive")
        .join(name)
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn blob(v: &[DynVertex], i: &[u16]) -> Vec<u8> {
    let mesh = DynamicMesh::new(v, i);
    let mut b = vec![0; mesh.tmdl_len().unwrap()];
    assert_eq!(mesh.write_tmdl(&mut b), Ok(b.len()));
    b
}

/// A `side` square facing the camera (−z), one color.
fn quad(side: f32, rgb: [u8; 3]) -> Vec<u8> {
    let n = Vec3::new(0.0, 0.0, -1.0);
    let c = Color::rgb(rgb[0], rgb[1], rgb[2]);
    let h = side / 2.0;
    let v = [
        DynVertex::new(Vec3::new(-h, -h, 0.0), n, c),
        DynVertex::new(Vec3::new(h, -h, 0.0), n, c),
        DynVertex::new(Vec3::new(h, h, 0.0), n, c),
        DynVertex::new(Vec3::new(-h, h, 0.0), n, c),
    ];
    blob(&v, &[0, 2, 1, 0, 3, 2])
}

/// A spark: twelve thin rays from a hot center to transparent tips.
fn burst() -> Vec<u8> {
    let n = Vec3::new(0.0, 0.0, -1.0);
    let mut v = vec![DynVertex::new(
        Vec3::ZERO,
        n,
        Color::rgba(255, 220, 120, 255),
    )];
    let mut i = Vec::new();
    for k in 0..12u16 {
        let a = k as f32 * std::f32::consts::TAU / 12.0;
        let len = if k % 2 == 0 { 1.0 } else { 0.6 };
        let tip = |da: f32| {
            Vec3::new(
                (a + da).cos() * len * if da == 0.0 { 1.0 } else { 0.25 },
                (a + da).sin() * len * if da == 0.0 { 1.0 } else { 0.25 },
                0.0,
            )
        };
        let base = v.len() as u16;
        v.push(DynVertex::new(tip(0.0), n, Color::rgba(255, 90, 20, 0)));
        v.push(DynVertex::new(tip(0.35), n, Color::rgba(255, 160, 60, 160)));
        v.push(DynVertex::new(
            tip(-0.35),
            n,
            Color::rgba(255, 160, 60, 160),
        ));
        i.extend_from_slice(&[0, base + 1, base, 0, base, base + 2]);
    }
    blob(&v, &i)
}

fn upload(r: &mut PcRenderer) {
    r.upload_mesh(WALL, quad(20.0, WALL_RGB));
    r.upload_mesh(SPARK, quad(1.0, SPARK_RGB));
    r.upload_mesh(BURST, burst());
    r.upload_mesh(BLOCK, quad(0.6, BLOCK_RGB));
}

fn at(x: f32, y: f32, z: f32) -> Mat34 {
    scaled(x, y, z, 1.0)
}

fn scaled(x: f32, y: f32, z: f32, s: f32) -> Mat34 {
    Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::new(s, s, s), Vec3::new(x, y, z))
}

fn params(blend: Blend) -> ModelParams {
    ModelParams {
        shading: Shading::Unlit,
        double_sided: true,
        blend,
        ..ModelParams::DEFAULT
    }
}

fn begin(r: &mut PcRenderer) {
    r.begin_frame(Color::BLACK);
    r.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 0.0, -4.0),
        target: Vec3::ZERO,
        fov_y: 60f32.to_radians(),
    });
    r.set_light(&DEFAULT_LIGHT);
    r.draw_model_matrix(
        WALL,
        &at(0.0, 0.0, 3.0),
        Material::VertexLit,
        &params(Blend::Alpha),
    );
}

fn finish(r: &mut PcRenderer) -> (Vec<u8>, u32, u32) {
    r.end_frame();
    let size = r.screen_size(Screen::Top).unwrap();
    (r.read_offscreen(), size.x as u32, size.y as u32)
}

/// Two sparks side by side, overlapping in the middle; `near_left` puts
/// the left one in front.
fn pair(r: &mut PcRenderer, blend: Blend, near_left: bool) -> (Vec<u8>, u32, u32) {
    begin(r);
    let (zl, zr) = if near_left { (-0.1, 0.0) } else { (0.0, -0.1) };
    r.draw_model_matrix(
        SPARK,
        &at(-0.3, 0.0, zl),
        Material::VertexLit,
        &params(blend),
    );
    r.draw_model_matrix(
        SPARK,
        &at(0.3, 0.0, zr),
        Material::VertexLit,
        &params(blend),
    );
    finish(r)
}

fn px(img: &(Vec<u8>, u32, u32), fx: f32, fy: f32) -> [u8; 3] {
    let (x, y) = ((fx * img.1 as f32) as usize, (fy * img.2 as f32) as usize);
    let i = (y * img.1 as usize + x) * 4;
    [img.0[i], img.0[i + 1], img.0[i + 2]]
}

fn near(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    a.iter()
        .zip(b)
        .all(|(&x, y)| (x as i32 - y as i32).abs() <= tol)
}

fn add(a: [u8; 3], b: [u8; 3]) -> [u8; 3] {
    std::array::from_fn(|i| a[i].saturating_add(b[i]))
}

/// The left spark alone (x = −0.6 m), the overlap (center).
const LEFT: (f32, f32) = (0.40, 0.5);
const MIDDLE: (f32, f32) = (0.5, 0.5);

/// A spark shower over the wall, for the eye.
fn shower(r: &mut PcRenderer, blend: Blend) -> (Vec<u8>, u32, u32) {
    begin(r);
    let spots = [
        (-1.4, 0.6, 0.0, 1.0),
        (-0.4, 0.2, -0.2, 1.3),
        (0.2, 0.5, 0.1, 0.9),
        (1.1, -0.2, 0.0, 1.2),
        (-0.9, -0.6, -0.1, 0.8),
        (0.6, -0.7, 0.2, 1.0),
        (0.0, -0.1, -0.3, 0.7),
    ];
    for (x, y, z, s) in spots {
        let m = scaled(x, y, z, s);
        r.draw_model_matrix(BURST, &m, Material::VertexLit, &params(blend));
    }
    finish(r)
}

#[test]
fn additive_sparks_add_their_light_and_alpha_ones_cover() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let alpha = pair(&mut r, Blend::Alpha, true);
    let added = pair(&mut r, Blend::Add, true);
    assert!(near(px(&alpha, LEFT.0, LEFT.1), SPARK_RGB, 1));
    assert!(near(px(&alpha, MIDDLE.0, MIDDLE.1), SPARK_RGB, 1));
    let one = add(WALL_RGB, SPARK_RGB);
    let two = add(one, SPARK_RGB);
    assert!(
        near(px(&added, LEFT.0, LEFT.1), one, 2),
        "{:?} vs {one:?}",
        px(&added, LEFT.0, LEFT.1)
    );
    assert!(
        near(px(&added, MIDDLE.0, MIDDLE.1), two, 2),
        "{:?} vs {two:?} (saturated)",
        px(&added, MIDDLE.0, MIDDLE.1)
    );
    // Away from the sparks, the wall as it was.
    assert!(near(px(&added, 0.1, 0.1), WALL_RGB, 1));
    // Evidence: the pair, and a shower of sparks with and without.
    write_png(&evidence("pair_alpha.png"), alpha.1, alpha.2, &alpha.0);
    write_png(&evidence("pair_add.png"), added.1, added.2, &added.0);
    for (name, blend) in [
        ("sparks_alpha.png", Blend::Alpha),
        ("sparks_add.png", Blend::Add),
    ] {
        let img = shower(&mut r, blend);
        write_png(&evidence(name), img.1, img.2, &img.0);
    }
    let a = shower(&mut r, Blend::Alpha);
    let b = shower(&mut r, Blend::Add);
    let light = |img: &(Vec<u8>, u32, u32)| img.0.iter().map(|&v| v as u64).sum::<u64>();
    assert!(light(&b) > light(&a), "adding never darkens the frame");
}

#[test]
fn additive_sparks_ignore_their_depth_order() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    // (The nearer spark is a little bigger on screen, so compare inside.)
    let a = pair(&mut r, Blend::Add, true);
    let b = pair(&mut r, Blend::Add, false);
    for (x, y) in [MIDDLE, LEFT, (0.55, 0.45), (0.47, 0.52)] {
        assert_eq!(px(&a, x, y), px(&b, x, y), "adding commutes at {x}, {y}");
    }
    // Translucent alpha sparks do depend on which is in front.
    let half = |r: &mut PcRenderer, near_left: bool| {
        begin(r);
        let (zl, zr) = if near_left { (-0.1, 0.0) } else { (0.0, -0.1) };
        let p = |c: Color| ModelParams {
            tint: c,
            ..params(Blend::Alpha)
        };
        let red = Color::rgba(255, 40, 40, 160);
        let blue = Color::rgba(40, 40, 255, 160);
        r.draw_model_matrix(SPARK, &at(-0.3, 0.0, zl), Material::VertexLit, &p(red));
        r.draw_model_matrix(SPARK, &at(0.3, 0.0, zr), Material::VertexLit, &p(blue));
        finish(r)
    };
    let c = half(&mut r, true);
    let d = half(&mut r, false);
    assert_ne!(px(&c, MIDDLE.0, MIDDLE.1), px(&d, MIDDLE.0, MIDDLE.1));
}

#[test]
fn additive_draws_write_no_depth_and_hide_behind_walls() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let frame = |r: &mut PcRenderer, blend: Blend, block_z: f32| {
        begin(r);
        r.draw_model_matrix(
            SPARK,
            &at(-0.3, 0.0, 0.0),
            Material::VertexLit,
            &params(blend),
        );
        // A camera change flushes the batch: the block comes after the
        // spark, whatever its depth.
        r.set_camera(&Camera3 {
            eye: Vec3::new(0.0, 0.0, -4.0),
            target: Vec3::ZERO,
            fov_y: 60f32.to_radians(),
        });
        r.draw_model_matrix(
            BLOCK,
            &at(-0.6, 0.0, block_z),
            Material::VertexLit,
            &params(Blend::Alpha),
        );
        finish(r)
    };
    // Behind the spark: hidden by an opaque spark, drawn over an additive
    // one (it wrote no depth).
    let opaque = frame(&mut r, Blend::Alpha, 1.0);
    assert!(near(px(&opaque, LEFT.0, LEFT.1), SPARK_RGB, 1));
    let added = frame(&mut r, Blend::Add, 1.0);
    assert!(near(px(&added, LEFT.0, LEFT.1), BLOCK_RGB, 1));
    // In front of it: the block covers either.
    for blend in [Blend::Alpha, Blend::Add] {
        let img = frame(&mut r, blend, -1.0);
        assert!(near(px(&img, LEFT.0, LEFT.1), BLOCK_RGB, 1), "{blend:?}");
    }
    // An additive spark behind an opaque block drawn first adds nothing
    // there (depth tested).
    begin(&mut r);
    r.draw_model_matrix(
        BLOCK,
        &at(-0.6, 0.0, -1.0),
        Material::VertexLit,
        &params(Blend::Alpha),
    );
    r.draw_model_matrix(
        SPARK,
        &at(-0.3, 0.0, 0.0),
        Material::VertexLit,
        &params(Blend::Add),
    );
    let img = finish(&mut r);
    assert!(near(px(&img, LEFT.0, LEFT.1), BLOCK_RGB, 1));
}

#[test]
fn the_gpu_path_leaves_additive_draws_to_the_cpu_path() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let gpu = shower(&mut r, Blend::Add);
    let s = r.stats();
    assert_eq!(s.tnl.pc_gpu_draws, 1, "the wall only");
    assert!(
        s.tnl.emitted > 0,
        "the sparks' triangles came from the CPU T&L"
    );
    r.set_hw_tnl(false);
    let cpu = shower(&mut r, Blend::Add);
    assert_eq!(r.stats().tnl.pc_gpu_draws, 0);
    let far = gpu
        .0
        .iter()
        .zip(&cpu.0)
        .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 1)
        .count();
    assert_eq!(far, 0, "the same frame on both paths");
}

#[test]
fn the_console_profiles_add_too() {
    let _g = gpu();
    for (profile, name) in [(SimProfile::N64, "n64"), (SimProfile::N3ds, "3ds")] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        let alpha = pair(&mut r, Blend::Alpha, true);
        let added = pair(&mut r, Blend::Add, true);
        let one = add(WALL_RGB, SPARK_RGB);
        // The N64 look quantizes to 5 bits a channel and dithers.
        assert!(
            near(px(&added, LEFT.0, LEFT.1), one, 12),
            "{name}: {:?} vs {one:?}",
            px(&added, LEFT.0, LEFT.1)
        );
        assert!(near(px(&alpha, LEFT.0, LEFT.1), SPARK_RGB, 12), "{name}");
        let img = shower(&mut r, Blend::Add);
        write_png(
            &evidence(&format!("sparks_add_{name}.png")),
            img.1,
            img.2,
            &img.0,
        );
    }
}
