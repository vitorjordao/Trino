//! The PC native profile's GPU T&L path (issue #51, ADR 0016) through the
//! public API: the draws `pc_gpu_check` accepts — static, animated, smooth
//! skinned, instanced — drawn by the WGSL vertex stage within the oracle's
//! tolerance of the CPU T&L (`tnl_oracle`), the draws it rejects kept on the
//! CPU with their reason, `set_hw_tnl(false)` (`TRINO_PC_TNL=cpu`) bringing
//! the old path back, and the parallel CPU fallback drawing the same pixels
//! and counters for any worker count. GPU-gated like the goldens.

use std::path::{Path, PathBuf};

use trino_asset_pipeline::primitive::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Fog, PcGpuReject};
use trino_core::{
    AnimRef, ClipId, Color, Instance, Mat34, Material, ModelId, ModelParams, Renderer, Shading,
    Transform3, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile, tnl_oracle};

const CRATE: ModelId = ModelId(1);
const BALL: ModelId = ModelId(2);
const FLOOR: ModelId = ModelId(3);
const ROBOT: ModelId = ModelId(4);
const STRIP: ModelId = ModelId(5);

/// One GPU test at a time (parallel headless devices crash some drivers).
fn gpu() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn headless() -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(SimProfile::Native)) {
        Ok(mut r) => {
            r.set_internal_size(480, 270);
            // These tests are about the GPU path: on even under
            // `TRINO_PC_TNL=cpu` (CI's run of the old path).
            r.set_hw_tnl(true);
            Some(r)
        }
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
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

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bake(file: &str, smooth: bool, anim_fps: f32) -> Vec<u8> {
    let opts = trino_asset_pipeline::model::ModelOptions {
        smooth,
        anim_fps,
        ..Default::default()
    };
    trino_asset_pipeline::model::bake_model(&root().join("assets/shared").join(file), &opts)
        .unwrap_or_else(|e| panic!("{e}"))
}

fn upload(r: &mut PcRenderer) {
    r.upload_mesh(
        CRATE,
        prim(
            Shape::Box {
                size: [0.6, 0.6, 0.6],
            },
            [200, 140, 80, 255],
            1,
        ),
    );
    r.upload_mesh(
        BALL,
        prim(Shape::Sphere { radius: 0.3 }, [90, 170, 230, 255], 10),
    );
    r.upload_mesh(
        FLOOR,
        prim(
            Shape::Box {
                size: [12.0, 0.2, 12.0],
            },
            [120, 150, 90, 255],
            1,
        ),
    );
    r.upload_mesh(ROBOT, bake("models/showcase/robot.glb", false, 20.0));
    r.upload_mesh(
        STRIP,
        bake("models/proxies/selftest-strip.gltf", true, 20.0),
    );
}

fn camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(0.0, 3.0, -7.0),
        target: Vec3::new(0.0, 0.6, 0.0),
        fov_y: 60f32.to_radians(),
    }
}

fn begin(r: &mut PcRenderer, fog: bool) {
    r.begin_frame(Color::rgb(40, 60, 90));
    r.set_camera(&camera());
    let mut light = DEFAULT_LIGHT;
    if fog {
        light.fog = Fog {
            color: Color::rgb(140, 160, 190),
            near: 4.0,
            far: 14.0,
        };
    }
    r.set_light(&light);
}

fn at(x: f32, y: f32, z: f32, yaw: f32) -> Mat34 {
    Transform3 {
        position: Vec3::new(x, y, z),
        rotation: Vec3::new(0.0, yaw, 0.0),
        ..Transform3::default()
    }
    .matrix()
}

/// A field of `n` copies of a model on a grid, each with its own tint.
fn field(n: usize) -> Vec<Instance> {
    let side = (n as f32).sqrt().ceil() as usize;
    (0..n)
        .map(|i| {
            let (gx, gz) = ((i % side) as f32, (i / side) as f32);
            let s = 5.0 / side as f32;
            Instance {
                tint: Color::rgb(255, 160 + (i * 37 % 96) as u8, 200),
                ..Instance::new(at(gx * s - 2.5, 0.0, gz * s - 1.0, i as f32 * 0.4))
            }
        })
        .collect()
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// The oracle, with its side-by-side kept as evidence.
fn oracle(
    r: &mut PcRenderer,
    name: &str,
    frame: impl FnMut(&mut PcRenderer) -> Vec<u8>,
) -> tnl_oracle::TnlDiff {
    let d = tnl_oracle::compare(r, frame).expect("the native profile has the GPU path");
    eprintln!("{name}: {d}");
    let (w, h, img) = d.side_by_side();
    write_png(
        &root().join(format!("target/evidence/pc-gpu-tnl/{name}.png")),
        w,
        h,
        &img,
    );
    d.assert_within(name);
    d
}

/// Every shading, a tint, an emissive, a palette, a double-sided draw and
/// fog: what the vertex stage does per vertex, against the CPU path.
#[test]
fn static_draws_match_the_cpu_path() {
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    let frame = |r: &mut PcRenderer| {
        begin(r, true);
        r.draw_model_matrix(
            FLOOR,
            &at(0.0, -0.2, 2.0, 0.0),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        for (k, shading) in [Shading::Gouraud, Shading::Flat, Shading::Unlit]
            .into_iter()
            .enumerate()
        {
            let x = k as f32 * 1.4 - 1.4;
            r.draw_model_matrix(
                CRATE,
                &at(x, 0.0, k as f32 * 2.0, 0.7),
                Material::VertexLit,
                &ModelParams {
                    shading,
                    tint: Color::rgb(255, 210, 190),
                    emissive: Color::rgb(12, 0, 24),
                    ..ModelParams::DEFAULT
                },
            );
            r.draw_model_matrix(
                BALL,
                &at(x, 0.9, k as f32 * 2.0, 0.0),
                Material::VertexLit,
                &ModelParams {
                    shading,
                    double_sided: k == 2,
                    palette: Some([Color::rgb(255, 80, 80); 4]),
                    ..ModelParams::DEFAULT
                },
            );
        }
        r.end_frame();
        r.read_offscreen()
    };
    let d = oracle(&mut r, "static_draws", frame);
    assert_eq!(d.gpu_draws, 7);
    // GPU draws do no CPU vertex work.
    frame(&mut r);
    let s = r.stats();
    assert_eq!((s.tnl.pc_gpu_draws, s.tnl.verts, s.tnl.emitted), (7, 0, 0));
    assert!(s.tris >= s.tnl.pc_gpu_tris && s.tnl.pc_gpu_tris > 0);
}

/// A rigid skeleton playing a clip (the robot's walk, a crossfade into its
/// wave) and a smooth-skinned strip (`SKW2`, the 50/50 vertex): the joint
/// matrices go to the GPU, the mesh stays resident.
#[test]
fn animated_and_smooth_skinned_meshes_draw_on_the_gpu() {
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    let resident = r.resident_mesh_bytes();
    assert!(resident > 0, "meshes are resident from upload_mesh");
    let walk = ClipId::from_name("walk");
    let wave = ClipId::from_name("wave");
    let frame = |r: &mut PcRenderer| {
        begin(r, false);
        for (k, t) in [0.1f32, 0.35, 0.6].into_iter().enumerate() {
            let mut anim = AnimRef::new(walk, t);
            if k == 2 {
                anim = anim.blend_to(wave, 0.4, 0.5);
            }
            r.draw_model_matrix(
                ROBOT,
                &at(k as f32 * 1.8 - 1.8, 0.0, 0.0, 2.6 + k as f32 * 0.3),
                Material::VertexLit,
                &ModelParams {
                    anim: Some(anim),
                    ..ModelParams::DEFAULT
                },
            );
        }
        r.draw_model_matrix(
            STRIP,
            &at(0.0, 1.8, -1.0, 0.0),
            Material::VertexLit,
            &ModelParams {
                double_sided: true,
                ..ModelParams::DEFAULT
            },
        );
        r.end_frame();
        r.read_offscreen()
    };
    let d = oracle(&mut r, "skinned_draws", frame);
    assert_eq!(d.gpu_draws, 4);
    assert_eq!(r.resident_mesh_bytes(), resident, "no re-upload per frame");
    // The pose moves the pixels: a later time is a different frame.
    let a = frame(&mut r);
    let b = {
        begin(&mut r, false);
        r.draw_model_matrix(
            ROBOT,
            &at(-1.8, 0.0, 0.0, 2.6),
            Material::VertexLit,
            &ModelParams {
                anim: Some(AnimRef::new(walk, 0.3)),
                ..ModelParams::DEFAULT
            },
        );
        r.end_frame();
        r.read_offscreen()
    };
    assert!(a != b);
}

/// 400 copies in one call: one GPU instanced draw per (model, texture,
/// sidedness) group, the copies' tints applied, pixels within the oracle;
/// 400 separate `draw_model` calls are merged into the same few draw calls.
#[test]
fn instanced_copies_are_a_few_draw_calls_and_the_same_pixels() {
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    let copies = field(400);
    let call = |r: &mut PcRenderer| {
        begin(r, false);
        r.draw_model_instances(BALL, &copies, &ModelParams::DEFAULT);
        r.end_frame();
        r.read_offscreen()
    };
    let separate = |r: &mut PcRenderer| {
        begin(r, false);
        let params = ModelParams::DEFAULT;
        for c in &copies {
            r.draw_model_matrix(
                BALL,
                &c.matrix,
                Material::VertexLit,
                &c.params(&params, &[]),
            );
        }
        r.end_frame();
        r.read_offscreen()
    };
    let d = oracle(&mut r, "instancing_400", call);
    assert_eq!(d.gpu_draws, 400);
    let one = call(&mut r);
    let s = r.stats();
    assert_eq!(
        (s.tnl.instanced, s.tnl.pc_gpu_draws, s.tnl.verts),
        (400, 400, 0)
    );
    let calls = r.frame_times().draw_calls;
    assert!(calls <= 4, "400 copies took {calls} draw calls");
    let many = separate(&mut r);
    assert_eq!(
        r.frame_times().draw_calls,
        calls,
        "separate draws merge too"
    );
    assert!(one == many, "one call draws what the separate draws draw");
}

/// What the GPU path can't draw stays on the CPU path with its reason,
/// in the same frame as the GPU draws.
#[test]
fn rejected_draws_stay_on_the_cpu_with_their_reason() {
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    let frame = |r: &mut PcRenderer| {
        begin(r, false);
        r.draw_model_matrix(
            CRATE,
            &at(-1.5, 0.0, 0.0, 0.5),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        r.draw_model_matrix(
            CRATE,
            &at(0.0, 0.0, 0.0, 0.5),
            Material::VertexLit,
            &ModelParams::tinted(Color::rgba(255, 255, 255, 128)),
        );
        r.draw_model_matrix(
            ROBOT,
            &at(1.5, 0.0, 0.0, 2.6),
            Material::VertexLit,
            &ModelParams {
                shading: Shading::Flat,
                anim: Some(AnimRef::new(ClipId::from_name("walk"), 0.2)),
                ..ModelParams::DEFAULT
            },
        );
        r.end_frame();
        r.read_offscreen()
    };
    let d = oracle(&mut r, "rejected_draws", frame);
    assert_eq!(d.gpu_draws, 1);
    let off = r.pc_gpu_rejects();
    assert_eq!(off[PcGpuReject::Translucent as usize], 1);
    assert_eq!(off[PcGpuReject::FlatSkinned as usize], 1);
    assert!(
        r.stats().tnl.verts > 0,
        "the rejected draws ran the CPU T&L"
    );
}

/// `set_hw_tnl(false)` (`TRINO_PC_TNL=cpu`) is the old path: every draw on
/// the CPU T&L, counted as "GPU T&L off", and the setting holds across
/// frames until turned back on.
#[test]
fn turning_the_gpu_path_off_restores_the_cpu_path() {
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    let copies = field(16);
    let frame = |r: &mut PcRenderer| {
        begin(r, false);
        r.draw_model_instances(CRATE, &copies, &ModelParams::DEFAULT);
        r.end_frame();
    };
    // On by default on the native profile (`TRINO_PC_TNL=cpu` starts it off).
    let fresh = pollster::block_on(PcRenderer::new_headless(SimProfile::Native)).unwrap();
    let env_cpu = matches!(std::env::var("TRINO_PC_TNL").as_deref(), Ok("cpu"));
    assert_eq!(fresh.hw_tnl(), !env_cpu);
    drop(fresh);
    r.set_hw_tnl(false);
    assert!(!r.hw_tnl());
    for _ in 0..2 {
        frame(&mut r);
        let s = r.stats();
        assert_eq!(s.tnl.pc_gpu_draws, 0);
        assert!(s.tnl.verts > 0 && s.tnl.instanced == 16);
        assert_eq!(r.pc_gpu_rejects()[PcGpuReject::Disabled as usize], 16);
    }
    r.set_hw_tnl(true);
    frame(&mut r);
    assert_eq!(r.stats().tnl.pc_gpu_draws, 16);
    // The console profiles never take the GPU path.
    let Ok(mut n64) = pollster::block_on(PcRenderer::new_headless(SimProfile::N64)) else {
        return;
    };
    n64.set_hw_tnl(true);
    assert!(!n64.hw_tnl());
}

/// The CPU path on many threads: byte-identical pixels and identical work
/// counters for 1, 2 and 8 workers (the chunks are concatenated in draw
/// order, so the batch's depth sort sees the serial stream).
#[test]
fn the_parallel_cpu_fallback_draws_the_same_frame_on_any_worker_count() {
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    r.set_hw_tnl(false);
    let copies = field(300);
    let frame = |r: &mut PcRenderer, threads: usize| {
        r.set_tnl_threads(threads);
        begin(r, true);
        r.draw_model_matrix(
            FLOOR,
            &at(0.0, -0.2, 2.0, 0.0),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        r.draw_model_instances(
            BALL,
            &copies,
            &ModelParams::tinted(Color::rgba(255, 255, 255, 200)),
        );
        r.end_frame();
        (r.read_offscreen(), r.stats())
    };
    let (serial, stats) = frame(&mut r, 1);
    assert!(
        stats.tnl.tris_in > 2048 * 4,
        "enough triangles to go parallel"
    );
    for threads in [2, 8] {
        let (img, s) = frame(&mut r, threads);
        assert!(img == serial, "{threads} workers drew a different frame");
        assert_eq!(s, stats, "{threads} workers counted different work");
    }
}
