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
    AnimRef, BillboardParams, ClipId, Color, Instance, LocalLight, Mat34, Material, ModelId,
    ModelParams, Renderer, Shading, SpriteId, Transform3, Vec2, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile, tnl_oracle};

const CRATE: ModelId = ModelId(1);
const BALL: ModelId = ModelId(2);
const FLOOR: ModelId = ModelId(3);
const ROBOT: ModelId = ModelId(4);
const STRIP: ModelId = ModelId(5);
/// A box whose vertices are translucent (`Mesh::opaque` false).
const GLASS: ModelId = ModelId(6);
const PUFF: SpriteId = SpriteId(7);

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
    r.upload_mesh(
        GLASS,
        prim(
            Shape::Box {
                size: [0.7, 0.9, 0.7],
            },
            [140, 230, 160, 110],
            1,
        ),
    );
    // A soft white puff for billboards (their tint blends them).
    r.upload_sprite(PUFF, 4, 4, &[255; 64]);
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

/// A layered pose (issue #112): the walk with the wave over some joints
/// and an additive layer, pushed into the renderer's pose table. The GPU
/// path poses the same joints as the CPU path (the oracle), the table's
/// pose is what draws (not the folded `AnimRef`), and `pose_joints` says so.
#[test]
fn layered_poses_draw_the_same_on_the_gpu_and_the_cpu() {
    use trino_core::anim::{AnimPose, JointMask, LayerMode, PoseLayer, SampleRole};
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    let walk = ClipId::from_name("walk");
    let wave = ClipId::from_name("wave");
    let pose = |t: f32| {
        let mut p = AnimPose::clip(walk, t);
        let up = p
            .add_layer(PoseLayer {
                mask: JointMask(0b1111_0000),
                mode: LayerMode::Override,
                weight: 0.8,
                decay: 0.0,
            })
            .unwrap();
        p.push(up, SampleRole::Pose, wave, 0.4, 1.0);
        let add = p
            .add_layer(PoseLayer {
                mask: JointMask::ALL,
                mode: LayerMode::Additive,
                weight: 0.5,
                decay: 0.0,
            })
            .unwrap();
        p.push(add, SampleRole::Pose, wave, 0.7, 1.0);
        p
    };
    let frame = |r: &mut PcRenderer, layered: bool| {
        begin(r, false);
        for (k, t) in [0.1f32, 0.35, 0.6].into_iter().enumerate() {
            let p = pose(t);
            let anim = if layered { r.push_pose(&p) } else { p.to_ref() };
            assert_eq!(anim.pose > 0, layered);
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
        r.end_frame();
        r.read_offscreen()
    };
    let d = oracle(&mut r, "layered_draws", |r| frame(r, true));
    assert_eq!(d.gpu_draws, 3);
    // The table's pose draws, not its fold.
    assert!(frame(&mut r, true) != frame(&mut r, false));
    // The joints the renderer poses are the layered pose's.
    r.begin_frame(Color::BLACK);
    let a = r.push_pose(&pose(0.35));
    let mut j = [Mat34::IDENTITY; trino_core::MAX_JOINTS];
    let n = r.pose_joints(ROBOT, &a, &mut j);
    let mut f = [Mat34::IDENTITY; trino_core::MAX_JOINTS];
    assert_eq!(r.pose_joints(ROBOT, &pose(0.35).to_ref(), &mut f), n);
    assert!(n > 4 && j[..n] != f[..n]);
    r.end_frame();
    // Cleared at begin_frame: the slot now draws the folded clips.
    r.begin_frame(Color::BLACK);
    let mut g = [Mat34::IDENTITY; trino_core::MAX_JOINTS];
    r.pose_joints(ROBOT, &a, &mut g);
    assert_eq!(g[..n], f[..n]);
    r.end_frame();
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
    // The translucent crate blends on the GPU path (ADR 0016 §13); the
    // flat-shaded skeleton stays.
    assert_eq!(d.gpu_draws, 2);
    frame(&mut r);
    let off = r.pc_gpu_rejects();
    assert_eq!(off[PcGpuReject::FlatSkinned as usize], 1);
    assert_eq!(off.iter().sum::<u32>(), 1, "{off:?}");
    assert!(
        r.stats().tnl.verts > 0,
        "the rejected draws ran the CPU T&L"
    );
}

/// Draws shaded per pixel (a point light and a spot reach them, gloss,
/// wetness) take the GPU path's lit pipelines: the same light sets and
/// masks as the CPU path's lit triangles, within the oracle's tolerance,
/// with no CPU vertex work. Instanced copies split into a plain and a lit
/// call. A lamp's reach used to send every draw it touched to the CPU.
#[test]
fn per_pixel_lit_draws_match_the_cpu_path() {
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    let lamps = [
        LocalLight::point(Vec3::new(-1.4, 1.0, -0.6), Color::rgb(255, 170, 80), 3.0),
        LocalLight::spot(
            Vec3::new(1.2, 3.5, 0.5),
            Vec3::new(0.0, -1.0, 0.1),
            0.3,
            0.5,
            Color::rgb(80, 220, 255),
            6.0,
        ),
    ];
    // Copies near the lamps (lit) and far from them (plain).
    let copies: Vec<Instance> = (0..6)
        .map(|i| Instance::new(at(i as f32 * 1.6 - 4.0, 0.0, 4.0 - i as f32 * 0.9, 0.3)))
        .collect();
    let frame = |r: &mut PcRenderer, wet: f32, lamps: &[LocalLight]| {
        begin(r, false);
        r.set_local_lights(lamps);
        if wet > 0.0 {
            r.set_wetness(wet);
        }
        r.draw_model_matrix(
            FLOOR,
            &at(0.0, -0.2, 2.0, 0.0),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        r.draw_model_matrix(
            BALL,
            &at(-1.4, 0.0, 0.0, 0.0),
            Material::VertexLit,
            &ModelParams {
                gloss: 200,
                ..ModelParams::DEFAULT
            },
        );
        r.draw_model_matrix(
            ROBOT,
            &at(1.2, 0.0, 0.5, 2.6),
            Material::VertexLit,
            &ModelParams {
                anim: Some(AnimRef::new(ClipId::from_name("walk"), 0.2)),
                ..ModelParams::DEFAULT
            },
        );
        r.draw_model_instances(CRATE, &copies, &ModelParams::DEFAULT);
        r.end_frame();
        r.read_offscreen()
    };
    for (name, wet) in [("lit_draws", 0.0), ("lit_draws_wet", 0.8)] {
        let d = oracle(&mut r, name, |r| frame(r, wet, &lamps));
        assert_eq!(d.gpu_draws, 9, "{name}: every draw on the GPU path");
        frame(&mut r, wet, &lamps);
        let s = r.stats();
        assert_eq!((s.tnl.verts, s.tnl.emitted), (0, 0), "{name}: no CPU T&L");
        assert!(
            r.pc_gpu_rejects().iter().all(|&n| n == 0),
            "{name}: nothing rejected: {:?}",
            r.pc_gpu_rejects()
        );
    }
    // The lamps do light the GPU frame (the lit pipelines aren't a no-op).
    let lit = frame(&mut r, 0.0, &lamps);
    let dark = frame(&mut r, 0.0, &[]);
    let brighter = lit
        .chunks(4)
        .zip(dark.chunks(4))
        .filter(|(a, b)| {
            let sum = |p: &[u8]| p[..3].iter().map(|&c| c as u32).sum::<u32>();
            sum(a) > sum(b) + 30
        })
        .count();
    assert!(brighter > 500, "{brighter} pixels lit by the lamps");
}

/// A draw wholly outside the view (behind the camera, past a side) costs
/// nothing on either path: counted as off screen, no CPU vertex work, no
/// GPU draw. Lit, translucent and alpha-textured ones too: the test comes
/// before every other reason (the CPU path used to transform their
/// vertices only to cull them).
#[test]
fn offscreen_draws_are_dropped_before_any_path() {
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    for hw in [true, false] {
        r.set_hw_tnl(hw);
        begin(&mut r, false);
        r.set_local_lights(&[LocalLight::point(
            Vec3::new(0.0, 1.0, -12.0),
            Color::WHITE,
            5.0,
        )]);
        // Behind the camera (it looks toward +z from z = -7).
        r.draw_model_matrix(
            CRATE,
            &at(0.0, 0.0, -12.0, 0.0),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        r.draw_model_matrix(
            BALL,
            &at(0.5, 0.0, -12.0, 0.0),
            Material::VertexLit,
            &ModelParams::tinted(Color::rgba(255, 255, 255, 100)),
        );
        // Far past the left edge.
        r.draw_model_matrix(
            CRATE,
            &at(-60.0, 0.0, 2.0, 0.0),
            Material::VertexLit,
            &ModelParams {
                gloss: 128,
                ..ModelParams::DEFAULT
            },
        );
        r.end_frame();
        let s = r.stats();
        assert_eq!(
            (s.tnl.verts, s.tnl.pc_gpu_draws, s.tris),
            (0, 0, 0),
            "hw_tnl={hw}: nothing transformed or drawn"
        );
        let off = r.pc_gpu_rejects();
        assert_eq!(
            off[PcGpuReject::Offscreen as usize],
            3,
            "hw_tnl={hw}: {off:?}"
        );
        assert_eq!(off.iter().sum::<u32>(), 3, "hw_tnl={hw}: {off:?}");
    }
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

fn unlit(tint: Color) -> ModelParams {
    ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::tinted(tint)
    }
}

/// Translucent draws on the GPU path (ADR 0016 §13): a tint below 255,
/// vertex alpha (`GLASS`), a double-sided sphere (back faces first), a lit
/// one under a lamp, instanced copies some of which blend, all in fog, and
/// a blended billboard among them (CPU triangles, merged back to front with
/// the GPU copies): the CPU path's picture, nothing rejected.
#[test]
fn translucent_draws_match_the_cpu_path() {
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    let lamp = [LocalLight::point(
        Vec3::new(1.6, 1.2, -0.4),
        Color::rgb(255, 190, 120),
        3.0,
    )];
    let copies: Vec<Instance> = (0..6)
        .map(|i| Instance {
            tint: Color::rgba(
                255,
                200,
                120 + i as u8 * 20,
                if i % 2 == 0 { 120 } else { 255 },
            ),
            ..Instance::new(at(i as f32 * 1.1 - 3.0, 0.0, 3.0 + i as f32 * 0.3, 0.4))
        })
        .collect();
    let frame = |r: &mut PcRenderer| {
        begin(r, true);
        r.set_local_lights(&lamp);
        r.draw_model_matrix(
            FLOOR,
            &at(0.0, -0.2, 2.0, 0.0),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        r.draw_model_matrix(
            CRATE,
            &at(-0.6, 0.0, 1.2, 0.5),
            Material::VertexLit,
            &ModelParams::tinted(Color::rgba(255, 140, 90, 140)),
        );
        r.draw_model_matrix(
            BALL,
            &at(-0.3, 0.3, -0.2, 0.0),
            Material::VertexLit,
            &ModelParams {
                double_sided: true,
                ..ModelParams::tinted(Color::rgba(90, 200, 255, 120))
            },
        );
        r.draw_model_matrix(
            GLASS,
            &at(0.5, 0.0, 0.4, 0.3),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        r.draw_model_matrix(
            CRATE,
            &at(1.6, 0.0, 0.0, 0.2),
            Material::VertexLit,
            &ModelParams {
                gloss: 180,
                ..ModelParams::tinted(Color::rgba(255, 255, 255, 150))
            },
        );
        r.draw_billboard(
            PUFF,
            Vec3::new(0.1, 0.5, 0.8),
            &BillboardParams {
                size: Vec2::new(1.2, 1.2),
                tint: Color::rgba(255, 240, 220, 130),
                ..BillboardParams::DEFAULT
            },
        );
        r.draw_model_instances(CRATE, &copies, &ModelParams::DEFAULT);
        r.end_frame();
        r.read_offscreen()
    };
    let d = oracle(&mut r, "translucent_draws", frame);
    assert_eq!(d.gpu_draws, 11, "every model draw on the GPU path");
    frame(&mut r);
    assert!(
        r.pc_gpu_rejects().iter().all(|&n| n == 0),
        "nothing rejected: {:?}",
        r.pc_gpu_rejects()
    );
}

/// A red box behind a blue one, both half transparent, drawn near first:
/// the GPU path sorts them back to front (blue over red) as the CPU path
/// does; 40 copies of one mesh in depth order are one instanced call, and
/// a blended billboard between them cuts the run where it lies. A
/// double-sided copy draws its back faces, then its front faces, each copy
/// on its own (in one call a far copy's front would cover a near copy's
/// back).
#[test]
fn translucent_copies_sort_back_to_front_in_few_calls() {
    let _gpu = gpu();
    let Some(mut r) = headless() else {
        return;
    };
    upload(&mut r);
    let pair = |r: &mut PcRenderer| {
        begin(r, false);
        let center = |z| Transform3 {
            position: Vec3::new(0.0, 0.6, z),
            scale: Vec3::new(3.0, 3.0, 3.0),
            ..Transform3::default()
        };
        r.draw_model(
            CRATE,
            &center(-2.0),
            Material::VertexLit,
            &unlit(Color::rgba(0, 0, 255, 128)),
        );
        r.draw_model(
            CRATE,
            &center(2.0),
            Material::VertexLit,
            &unlit(Color::rgba(255, 0, 0, 128)),
        );
        r.end_frame();
        let (w, h) = r.internal_size();
        r.read_pixel(w / 2, h / 2).expect("inside the frame")
    };
    for hw in [true, false] {
        r.set_hw_tnl(hw);
        let c = pair(&mut r);
        assert!(
            c.b as i32 > c.r as i32 + 40,
            "hw_tnl={hw}: blue over red, got {c:?}"
        );
    }
    r.set_hw_tnl(true);
    // A line of copies receding from the camera, submitted near first.
    let line: Vec<Instance> = (0..40)
        .map(|i| Instance::new(at(0.0, 0.0, 6.0 - i as f32 * 0.3, 0.0)))
        .collect();
    let calls = |r: &mut PcRenderer, double_sided: bool, puff: bool| {
        begin(r, false);
        if puff {
            r.draw_billboard(
                PUFF,
                Vec3::new(0.0, 0.3, 0.1),
                &BillboardParams {
                    tint: Color::rgba(255, 255, 255, 100),
                    ..BillboardParams::DEFAULT
                },
            );
        }
        r.draw_model_instances(
            CRATE,
            &line,
            &ModelParams {
                double_sided,
                ..ModelParams::tinted(Color::rgba(255, 255, 255, 90))
            },
        );
        r.end_frame();
        r.frame_times().draw_calls
    };
    assert_eq!(calls(&mut r, false, false), 1, "one instanced call");
    assert_eq!(calls(&mut r, true, false), 80, "back, then front, per copy");
    assert_eq!(
        calls(&mut r, false, true),
        3,
        "the copies behind the puff, the puff, the copies before it"
    );
    let d = oracle(&mut r, "translucent_line", |r| {
        calls(r, true, true);
        r.read_offscreen()
    });
    assert_eq!(d.gpu_draws, 40);
}
