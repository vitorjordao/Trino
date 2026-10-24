//! Pipeline precompilation (issue #101, ADR 0031) through the public API:
//! after `precompile(EVERYTHING)` no frame compiles a pipeline while every
//! lazily built pass is switched on (MSAA, ambient occlusion, the shadow
//! map, each color-vision view, a render scale, the GPU T&L), where the
//! same toggles without a warm-up do compile; a background warm-up keeps
//! frames coming and ends; the frames are the same with and without it;
//! a frame waits for a pass a worker is compiling; the Vulkan pipeline
//! cache round-trips through its file and survives a corrupt, truncated
//! or foreign one. GPU-gated like the goldens.

use trino_asset_pipeline::primitive::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::{
    AmbientOcclusion, Color, CvdMode, Material, ModelId, ModelParams, PrecompileRequest, Renderer,
    ShadowCast, ShadowPlane, ShadowSettings, Transform3, Vec3,
};
use trino_platform_pc::renderer::RendererOptions;
use trino_platform_pc::{PcRenderer, SimProfile};

const FLOOR: ModelId = ModelId(1);
const CRATE: ModelId = ModelId(2);

/// One GPU test at a time (parallel headless devices crash some drivers).
fn gpu() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn headless_with(options: &RendererOptions) -> Option<PcRenderer> {
    headless_on(SimProfile::Native, options)
}

fn headless_on(profile: SimProfile, options: &RendererOptions) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless_with(profile, options)) {
        Ok(mut r) => {
            r.set_internal_size(320, 180);
            r.set_hw_tnl(true);
            r.upload_mesh(
                FLOOR,
                prim(Shape::Box {
                    size: [10.0, 0.2, 10.0],
                }),
            );
            r.upload_mesh(
                CRATE,
                prim(Shape::Box {
                    size: [0.8, 0.8, 0.8],
                }),
            );
            // The setup's own pipelines (the GPU T&L path the uploads
            // built) belong to the first frame: draw it.
            r.begin_frame(Color::BLACK);
            r.end_frame();
            Some(r)
        }
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn headless() -> Option<PcRenderer> {
    headless_with(&RendererOptions::default())
}

fn prim(shape: Shape) -> Vec<u8> {
    Primitive {
        shape,
        color: [180, 150, 110, 255],
        pivot: Pivot::Bottom,
        segments: 1,
        ..Primitive::default()
    }
    .build()
    .encode()
    .unwrap()
}

/// A frame with shadows (the native profile's map), SSAO when `ao`, and
/// two models; returns the pipelines the render thread compiled in it.
fn frame(r: &mut PcRenderer, ao: bool) -> u32 {
    r.begin_frame(Color::rgb(60, 80, 110));
    r.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 3.0, -6.0),
        target: Vec3::new(0.0, 0.4, 0.0),
        fov_y: 60f32.to_radians(),
    });
    r.set_light(&DEFAULT_LIGHT);
    r.set_shadows(Some(&ShadowSettings::DEFAULT));
    r.set_shadow_receivers(&[ShadowPlane::ground(0.1)]);
    if ao {
        r.set_ambient_occlusion(Some(&AmbientOcclusion::DEFAULT));
    }
    let at = |p: Vec3| {
        Transform3 {
            position: p,
            ..Transform3::default()
        }
        .matrix()
    };
    r.draw_model_matrix(
        FLOOR,
        &at(Vec3::new(0.0, -0.1, 0.0)),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    r.draw_model_matrix(
        CRATE,
        &at(Vec3::new(0.0, 0.1, 0.0)),
        Material::VertexLit,
        &ModelParams {
            shadow: ShadowCast::Mesh,
            ..ModelParams::DEFAULT
        },
    );
    r.end_frame();
    r.frame_times().pipelines
}

/// Every lazily built pass, one after the other; the pipelines each step's
/// frame compiled.
fn toggle_everything(r: &mut PcRenderer) -> Vec<(&'static str, u32)> {
    let mut out = vec![("plain", frame(r, false))];
    out.push(("ssao", frame(r, true)));
    r.set_msaa(true);
    out.push(("msaa", frame(r, true)));
    for mode in [
        CvdMode::Protanopia,
        CvdMode::Deuteranopia,
        CvdMode::Tritanopia,
        CvdMode::Grayscale,
    ] {
        r.set_cvd_view(mode);
        out.push((mode.key(), frame(r, true)));
    }
    r.set_cvd_view(CvdMode::Off);
    r.set_render_scale(0.5);
    out.push(("render scale", frame(r, true)));
    r.set_msaa(false);
    out.push(("msaa off, scaled", frame(r, true)));
    out
}

#[test]
fn a_full_warm_up_leaves_nothing_to_compile_in_frames() {
    let _g = gpu();
    // Without a warm-up the same toggles compile mid-frame: the counter
    // sees the hitches this issue removes.
    let Some(mut cold) = headless() else { return };
    frame(&mut cold, false);
    let before = cold.pipeline_counts().render_thread;
    cold.set_msaa(true);
    assert!(
        cold.pipeline_counts().render_thread > before,
        "set_msaa builds its twins on the render thread, between frames"
    );
    cold.set_msaa(false);
    let cold_steps = toggle_everything(&mut cold);
    let compiled: u32 = cold_steps.iter().map(|s| s.1).sum();
    assert!(
        compiled > 0,
        "the lazy passes compile in frames without a warm-up: {cold_steps:?}"
    );
    let cold_pixels = cold.read_offscreen();
    drop(cold);

    let Some(mut warm) = headless() else { return };
    let status = warm.precompile(&PrecompileRequest::EVERYTHING.blocking());
    assert!(status.is_done() && status.total > 0, "{status:?}");
    assert!(status.threads >= 1);
    let counts = warm.pipeline_counts();
    assert!(
        counts.precompiled > 0,
        "the workers built the pipelines: {counts:?}"
    );
    // Nothing on the render thread at all from here, between frames
    // (`set_msaa`, `set_render_scale`) included.
    let render_thread = warm.pipeline_counts().render_thread;
    let unscoped = warm.pipeline_counts().unscoped;
    let steps = toggle_everything(&mut warm);
    for (step, n) in &steps {
        assert_eq!(*n, 0, "`{step}` compiled {n} pipelines after the warm-up");
    }
    assert_eq!(
        warm.pipeline_counts().render_thread,
        render_thread,
        "the render thread built pipelines after the warm-up: {steps:?}"
    );
    assert_eq!(
        warm.pipeline_counts().unscoped,
        unscoped,
        "a lazy site built pipelines outside the renderer's scope (uncounted, uncached)"
    );
    assert_eq!(
        warm.read_offscreen(),
        cold_pixels,
        "the warm-up changes no pixel"
    );
    // A second request finds nothing left.
    let again = warm.precompile(&PrecompileRequest::EVERYTHING.blocking());
    assert_eq!(again.total, 0, "{again:?}");
}

#[test]
fn a_background_warm_up_keeps_frames_coming_and_ends() {
    let _g = gpu();
    let Some(mut r) = headless() else { return };
    r.set_precompile_threads(2);
    let start = r.precompile(&PrecompileRequest::EVERYTHING);
    assert!(start.total > 0, "{start:?}");
    let mut frames = 0;
    let t0 = std::time::Instant::now();
    while !r.precompile_status().is_done() {
        // The loading screen: frames present while the workers compile
        // (one that needs a pass in flight waits for that job only).
        frame(&mut r, frames % 2 == 1);
        frames += 1;
        // Bounded by time, not frames: a software adapter (WARP, lavapipe)
        // compiles for seconds.
        assert!(t0.elapsed().as_secs() < 300, "{:?}", r.precompile_status());
    }
    let done = r.precompile_status();
    assert_eq!(done.done, done.total);
    assert!(done.threads >= 1 && done.threads <= 2, "{done:?}");
    // Everything is in place now.
    for (step, n) in toggle_everything(&mut r) {
        assert_eq!(n, 0, "`{step}` compiled after the background warm-up");
    }
}

/// A frame that needs a pass a worker is compiling waits for that job,
/// and the render thread builds nothing itself. The N64 profile's
/// `CURRENT` plan is one job (the blob/planar shadow pipelines), held in
/// flight by the test gate while the frame that casts a shadow runs.
#[test]
fn a_frame_waits_for_the_pass_a_worker_is_compiling() {
    let _g = gpu();
    let Some(mut r) = headless_on(SimProfile::N64, &RendererOptions::default()) else {
        return;
    };
    let gate = r.hold_precompile();
    r.set_precompile_threads(1);
    let plan = r.precompile(&PrecompileRequest::CURRENT);
    assert_eq!(plan.total, 1, "{plan:?}");
    let t0 = std::time::Instant::now();
    while r.precompile_in_flight() == 0 {
        assert!(t0.elapsed().as_secs() < 30, "the worker never took its job");
        std::thread::yield_now();
    }
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        gate.release();
    });
    let before = r.pipeline_counts();
    let t1 = std::time::Instant::now();
    let built = frame(&mut r, false);
    let waited = t1.elapsed();
    release.join().unwrap();
    assert!(
        waited >= std::time::Duration::from_millis(250),
        "the frame did not wait for the job in flight ({waited:?})"
    );
    assert_eq!(built, 0, "the render thread built the pass itself");
    let after = r.pipeline_counts();
    assert_eq!(after.render_thread, before.render_thread);
    assert!(after.precompiled > before.precompiled, "{after:?}");
    assert!(r.precompile_status().is_done());
    // And the next shadow frame is free.
    assert_eq!(frame(&mut r, false), 0);
}

#[test]
fn only_the_current_settings_are_warmed_by_current() {
    let _g = gpu();
    let Some(mut r) = headless() else { return };
    let current = r.precompile(&PrecompileRequest::CURRENT.blocking());
    let Some(mut all) = headless() else { return };
    let everything = all.precompile(&PrecompileRequest::EVERYTHING.blocking());
    assert!(
        current.total < everything.total,
        "MSAA twins and the CVD views are only in EVERYTHING: {current:?} vs {everything:?}"
    );
    // CURRENT then covers the frame the current settings draw.
    assert_eq!(frame(&mut r, true), 0);
}

#[test]
fn the_pipeline_cache_round_trips_through_its_file() {
    let _g = gpu();
    let dir = tempfile::tempdir().unwrap();
    let options = RendererOptions {
        pipeline_cache_dir: Some(dir.path().join("shaders")),
    };
    let Some(mut r) = headless_with(&options) else {
        return;
    };
    if !r.has_pipeline_cache() {
        eprintln!(
            "skipping: {} has no PIPELINE_CACHE (wgpu 29: Vulkan only; other drivers keep their own)",
            r.adapter_info()
        );
        return;
    }
    r.precompile(&PrecompileRequest::EVERYTHING.blocking());
    let bytes = r.save_pipeline_cache().unwrap().expect("a cache to save");
    assert!(bytes > 0);
    drop(r);
    let files: Vec<_> = std::fs::read_dir(dir.path().join("shaders"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "one file per adapter key: {files:?}");
    assert!(
        files[0].extension().is_some_and(|e| e == "bin"),
        "{files:?}"
    );

    // The next run loads it and draws the same.
    let Some(mut again) = headless_with(&options) else {
        return;
    };
    assert!(again.has_pipeline_cache());
    again.precompile(&PrecompileRequest::EVERYTHING.blocking());
    assert_eq!(frame(&mut again, true), 0);
    drop(again);

    // A file from another adapter or driver (its header doesn't match
    // this one's) and a truncated one start empty too.
    let valid = std::fs::read(&files[0]).unwrap();
    let mut foreign = valid.clone();
    for b in foreign.iter_mut().take(64).skip(8) {
        *b ^= 0x5a;
    }
    for (what, bytes) in [
        ("foreign", foreign),
        ("truncated", valid[..valid.len() / 2].to_vec()),
    ] {
        std::fs::write(&files[0], &bytes).unwrap();
        let Some(mut r) = headless_with(&options) else {
            return;
        };
        r.precompile(&PrecompileRequest::EVERYTHING.blocking());
        assert_eq!(frame(&mut r, true), 0, "{what}");
        assert!(r.save_pipeline_cache().unwrap().is_some(), "{what}");
        assert_ne!(
            std::fs::read(&files[0]).unwrap(),
            bytes,
            "{what}: rewritten"
        );
    }

    // A corrupt file (a driver update, a truncated write) starts empty.
    std::fs::write(&files[0], b"not a pipeline cache").unwrap();
    let Some(mut corrupt) = headless_with(&options) else {
        return;
    };
    corrupt.precompile(&PrecompileRequest::EVERYTHING.blocking());
    assert_eq!(frame(&mut corrupt, true), 0);
    assert!(corrupt.save_pipeline_cache().unwrap().is_some());
}
