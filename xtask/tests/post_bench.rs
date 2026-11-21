//! Issue #64: what the post stack costs on the PC native profile, per
//! quality preset level (`PostSetting`), at 1080p and 4K: castle64's hub
//! with the runner's 4x MSAA and its SSAO, drawn without post and with
//! `PostSettings::FILMIC` plus the `dusk` grading (AgX, bloom, vignette).
//! Each frame is the game's update, its render and the GPU's work
//! (`wait_gpu`). A measurement for ADR 0033 and the PR, not a gate:
//!
//! ```sh
//! cargo test --release -p xtask --test post_bench -- --ignored --nocapture
//! TRINO_POWER=low cargo test --release ...   # the iGPU of a two-GPU machine
//! ```
//!
//! `TRINO_BENCH_FRAMES` (default 240) sets the measured frames per row. The
//! table goes to `target/evidence/post/bench-<adapter>.md`, each 1080p
//! frame to `castle64-<level>.png` beside it.
//!
//! `speed_blur_cost_and_shots` measures the radial speed blur the same way
//! (that stack, with and without `set_speed_blur`) and writes the 1080p
//! hub at strengths 0, 0.15, 0.3 and 0.8, the same moment of a fresh game
//! each (`speed-blur-<strength>.png`, the table in
//! `speed-blur-<adapter>.md`):
//!
//! ```sh
//! cargo test --release -p xtask --test post_bench speed_blur -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

use trino_core::{
    Game, InputState, LutId, ModelId, PostSetting, PostSettings, PresetTable, QualityPreset,
    Renderer, SpeedBlur, Vec2,
};
use trino_platform_pc::{PcRenderer, SimProfile};

struct Mute;
impl trino_core::Audio for Mute {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn renderer(size: (u32, u32)) -> PcRenderer {
    let baked = root().join("target/assets/bench-post/pc");
    trino_asset_pipeline::bake_all(
        &root().join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let loaded = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = pollster::block_on(PcRenderer::new_headless(SimProfile::Native))
        .unwrap_or_else(|e| panic!("no adapter: {e}"));
    r.set_internal_size(size.0, size.1);
    r.set_msaa(true);
    for s in loaded.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in loaded.models {
        r.upload_mesh(ModelId(m.id), m.tmdl);
    }
    for f in loaded.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    for b in loaded.blobs {
        r.take_lut_blob(b.id, &b.bytes);
    }
    r
}

/// castle64's hub with `post` (and `blur`) set after the game's own render
/// settings.
struct Hub<'a> {
    game: Box<castle64::Castle64Game>,
    post: Option<&'a PostSettings>,
    blur: Option<SpeedBlur>,
}

impl Hub<'_> {
    fn frame(&mut self, r: &mut PcRenderer) {
        self.game
            .update(&InputState::default(), &mut Mute, castle64::perf::DT);
        let (post, blur) = (self.post, self.blur);
        let mut wrap = WithPost { r, post, blur };
        self.game.render(&mut wrap);
    }
}

/// A renderer that adds the bench's post settings to every frame.
struct WithPost<'a, 'b> {
    r: &'a mut PcRenderer,
    post: Option<&'b PostSettings>,
    blur: Option<SpeedBlur>,
}

impl Renderer for WithPost<'_, '_> {
    fn caps(&self) -> &trino_core::Caps {
        self.r.caps()
    }
    fn begin_frame(&mut self, clear: trino_core::Color) {
        self.r.begin_frame(clear);
        self.r.set_post(self.post);
        self.r.set_speed_blur(self.blur.as_ref());
    }
    fn end_frame(&mut self) {
        self.r.end_frame()
    }
    fn draw_sprite(&mut self, s: trino_core::SpriteId, p: Vec2, params: &trino_core::SpriteParams) {
        self.r.draw_sprite(s, p, params)
    }
    fn fill_rect(&mut self, rect: trino_core::Rect, c: trino_core::Color) {
        self.r.fill_rect(rect, c)
    }
    fn set_camera(&mut self, c: &trino_core::Camera3) {
        self.r.set_camera(c)
    }
    fn set_light(&mut self, l: &trino_core::Light) {
        self.r.set_light(l)
    }
    fn set_local_lights(&mut self, l: &[trino_core::LocalLight]) {
        self.r.set_local_lights(l)
    }
    fn set_ambient_occlusion(&mut self, ao: Option<&trino_core::AmbientOcclusion>) {
        self.r.set_ambient_occlusion(ao)
    }
    fn set_shadows(&mut self, s: Option<&trino_core::ShadowSettings>) {
        self.r.set_shadows(s)
    }
    fn set_shadow_receivers(&mut self, s: &[trino_core::ShadowPlane]) {
        self.r.set_shadow_receivers(s)
    }
    fn set_water(&mut self, t: f32, r: &[trino_core::Ripple]) {
        self.r.set_water(t, r)
    }
    fn draw_model_matrix(
        &mut self,
        m: ModelId,
        mat: &trino_core::Mat34,
        material: trino_core::Material,
        p: &trino_core::ModelParams,
    ) {
        self.r.draw_model_matrix(m, mat, material, p)
    }
    fn draw_model(
        &mut self,
        m: ModelId,
        t: &trino_core::Transform3,
        material: trino_core::Material,
        p: &trino_core::ModelParams,
    ) {
        self.r.draw_model(m, t, material, p)
    }
    fn draw_billboard(
        &mut self,
        s: trino_core::SpriteId,
        p: trino_core::Vec3,
        params: &trino_core::BillboardParams,
    ) {
        self.r.draw_billboard(s, p, params)
    }
    fn draw_text(
        &mut self,
        f: trino_core::FontId,
        p: Vec2,
        t: &str,
        params: &trino_core::TextParams,
    ) {
        self.r.draw_text(f, p, t, params)
    }
    fn measure_text(&self, f: trino_core::FontId, t: &str, scale: f32) -> Vec2 {
        self.r.measure_text(f, t, scale)
    }
    fn stats(&self) -> trino_core::FrameStats {
        self.r.stats()
    }
    fn lod_bias(&self) -> f32 {
        self.r.lod_bias()
    }
}

#[test]
#[ignore = "a measurement: run alone in release with --ignored --nocapture"]
fn post_stack_cost_per_preset_level() {
    let frames: usize = std::env::var("TRINO_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(240);
    let post = PostSettings {
        grading: Some(LutId::of("luts/dusk")),
        ..PostSettings::FILMIC
    };
    let mut table = String::from(
        "| Size | Post level | off ms (median) | with post ms (median) | Δ ms (median of pairs) | bloom levels |\n|---|---|---|---|---|---|\n",
    );
    let mut adapter = String::new();
    // Off and on alternate in blocks of 10 frames, so the iGPU's clock
    // drift (it swings tens of percent between runs) falls on both alike:
    // the cost is the median of the paired blocks' differences.
    let block = 10;
    let pairs = (frames / block).max(4);
    for size in [(1920u32, 1080u32), (3840, 2160)] {
        let mut r = renderer(size);
        adapter = r.adapter_info();
        for (name, level) in [
            ("Off", PostSetting::Off),
            ("Low", PostSetting::Low),
            ("Medium", PostSetting::Medium),
            ("High", PostSetting::High),
        ] {
            let mut q = PresetTable::BUILT_IN.get(QualityPreset::High);
            q.post = level;
            q.msaa = 4;
            r.apply_quality(&q);
            let scene = &castle64::perf::SCENES[0];
            let game = Box::new(castle64::perf::scene_game(
                scene,
                Vec2::new(size.0 as f32, size.1 as f32),
                &mut Mute,
            ));
            let mut hub = Hub {
                game,
                post: None,
                blur: None,
            };
            let run = |hub: &mut Hub, r: &mut PcRenderer, n: usize| {
                let t = Instant::now();
                for _ in 0..n {
                    hub.frame(r);
                    r.wait_gpu();
                }
                t.elapsed().as_secs_f64() * 1000.0 / n as f64
            };
            // Warm both paths (pipelines, targets); keep the 1080p frame
            // without post once, for the before/after.
            for on in [false, true] {
                hub.post = on.then_some(&post);
                run(&mut hub, &mut r, 20);
                if !on && size.0 == 1920 && level == PostSetting::Off {
                    let (w, h) = r.internal_size();
                    let path = root().join("target/evidence/post/castle64-none.png");
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    let file = std::fs::File::create(&path).unwrap();
                    let mut e = png::Encoder::new(std::io::BufWriter::new(file), w, h);
                    e.set_color(png::ColorType::Rgba);
                    e.set_depth(png::BitDepth::Eight);
                    e.write_header()
                        .unwrap()
                        .write_image_data(&r.read_offscreen())
                        .unwrap();
                }
            }
            let (mut offs, mut ons, mut deltas) = (Vec::new(), Vec::new(), Vec::new());
            let mut bloom = 0;
            for _ in 0..pairs {
                hub.post = None;
                let off = run(&mut hub, &mut r, block);
                hub.post = Some(&post);
                let on = run(&mut hub, &mut r, block);
                bloom = r.frame_times().bloom_levels;
                offs.push(off);
                ons.push(on);
                deltas.push(on - off);
            }
            let median = |v: &mut Vec<f64>| {
                v.sort_by(|a, b| a.total_cmp(b));
                v[v.len() / 2]
            };
            table.push_str(&format!(
                "| {}x{} | {name} | {:.2} | {:.2} | {:+.2} | {bloom} |\n",
                size.0,
                size.1,
                median(&mut offs),
                median(&mut ons),
                median(&mut deltas),
            ));
            if size.0 == 1920 {
                let (w, h) = r.internal_size();
                let path = root().join(format!(
                    "target/evidence/post/castle64-{}.png",
                    name.to_lowercase()
                ));
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                let file = std::fs::File::create(&path).unwrap();
                let mut e = png::Encoder::new(std::io::BufWriter::new(file), w, h);
                e.set_color(png::ColorType::Rgba);
                e.set_depth(png::BitDepth::Eight);
                e.write_header()
                    .unwrap()
                    .write_image_data(&r.read_offscreen())
                    .unwrap();
            }
        }
    }
    let text = format!("Adapter: {adapter}\n\n{table}");
    println!("{text}");
    let slug: String = adapter
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = root().join(format!("target/evidence/post/bench-{slug}.md"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn write_frame(r: &PcRenderer, name: &str) {
    let (w, h) = r.internal_size();
    let path = root().join(format!("target/evidence/post/{name}.png"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()
        .unwrap()
        .write_image_data(&r.read_offscreen())
        .unwrap();
}

#[test]
#[ignore = "a measurement: run alone in release with --ignored --nocapture"]
fn speed_blur_cost_and_shots() {
    let frames: usize = std::env::var("TRINO_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(240);
    let post = PostSettings {
        grading: Some(LutId::of("luts/dusk")),
        ..PostSettings::FILMIC
    };
    // The hub's camera looks ahead and a little down: the vanishing point
    // sits above the middle.
    let blur = |strength| SpeedBlur {
        center: Vec2::new(0.5, 0.45),
        ..SpeedBlur::new(strength)
    };
    let mut table = String::from(
        "| Size | Post level | Strength | GPU ms, post | GPU ms, post + blur | Δ GPU ms (median of pairs) | Δ frame ms (median of pairs) |\n|---|---|---|---|---|---|---|\n",
    );
    let mut adapter = String::new();
    let block = 10;
    let pairs = (frames / block).max(4);
    let median = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    };
    for size in [(1920u32, 1080u32), (3840, 2160)] {
        let mut r = renderer(size);
        adapter = r.adapter_info();
        for (name, level) in [("Low", PostSetting::Low), ("High", PostSetting::High)] {
            let mut q = PresetTable::BUILT_IN.get(QualityPreset::High);
            q.post = level;
            q.msaa = 4;
            // Dynamic resolution pinned at 1: the scale never moves, and
            // the renderer times each frame on the GPU (timestamps).
            q.dynamic_resolution = Some(trino_core::DynRes {
                target_us: 1_000_000,
                min_scale: 1.0,
                max_scale: 1.0,
            });
            r.apply_quality(&q);
            let scene = &castle64::perf::SCENES[0];
            let fresh = || {
                Box::new(castle64::perf::scene_game(
                    scene,
                    Vec2::new(size.0 as f32, size.1 as f32),
                    &mut Mute,
                ))
            };
            let mut hub = Hub {
                game: fresh(),
                post: Some(&post),
                blur: None,
            };
            // A block of `n` frames: the wall-clock frame and the median GPU
            // time (the timer reads a frame or two late: the first 3 skipped).
            let run = |hub: &mut Hub, r: &mut PcRenderer, n: usize| {
                let t = Instant::now();
                let mut gpu = Vec::new();
                for i in 0..n {
                    hub.frame(r);
                    r.wait_gpu();
                    if i >= 3 {
                        gpu.push(r.gpu_frame_us() as f64 / 1000.0);
                    }
                }
                let wall = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
                gpu.sort_by(|a, b| a.total_cmp(b));
                (wall, gpu.get(gpu.len() / 2).copied().unwrap_or(0.0))
            };
            if size.0 == 1920 && level == PostSetting::High {
                // The shots: the same moment of a fresh hub at each strength.
                for strength in [0.0f32, 0.15, 0.3, 0.8] {
                    let mut shot = Hub {
                        game: fresh(),
                        post: Some(&post),
                        blur: Some(blur(strength)),
                    };
                    run(&mut shot, &mut r, 30);
                    assert_eq!(r.frame_times().speed_blurs, (strength > 0.0) as u32);
                    assert_eq!(r.render_scale(), 1.0);
                    write_frame(&r, &format!("speed-blur-{strength:.2}"));
                }
            }
            for strength in [0.3f32, 0.8] {
                // Warm both paths (the gather target, its pipeline).
                for on in [false, true] {
                    hub.blur = on.then(|| blur(strength));
                    run(&mut hub, &mut r, 20);
                }
                let (mut offs, mut ons, mut deltas, mut walls) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                for _ in 0..pairs {
                    hub.blur = None;
                    let (wall_off, off) = run(&mut hub, &mut r, block);
                    hub.blur = Some(blur(strength));
                    let (wall_on, on) = run(&mut hub, &mut r, block);
                    offs.push(off);
                    ons.push(on);
                    deltas.push(on - off);
                    walls.push(wall_on - wall_off);
                }
                table.push_str(&format!(
                    "| {}x{} | {name} | {strength} | {:.2} | {:.2} | {:+.2} | {:+.2} |\n",
                    size.0,
                    size.1,
                    median(&mut offs),
                    median(&mut ons),
                    median(&mut deltas),
                    median(&mut walls),
                ));
            }
        }
    }
    let text = format!("Adapter: {adapter}\n\n{table}");
    println!("{text}");
    let slug: String = adapter
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = root().join(format!("target/evidence/post/speed-blur-{slug}.md"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Evidence for motion: the 1080p hub over 120 frames while the strength
/// ramps 0 → 0.3 → 0 (`target/evidence/post/clip/frame-NNN.png`, for a
/// video): the streaks grow and shrink smoothly and the taps' fixed jitter
/// never crawls.
#[test]
#[ignore = "evidence: run alone in release with --ignored --nocapture"]
fn speed_blur_clip_frames() {
    let size = (1920u32, 1080u32);
    let mut r = renderer(size);
    let mut q = PresetTable::BUILT_IN.get(QualityPreset::High);
    q.msaa = 4;
    q.dynamic_resolution = None;
    r.apply_quality(&q);
    let post = PostSettings {
        grading: Some(LutId::of("luts/dusk")),
        ..PostSettings::FILMIC
    };
    let mut hub = Hub {
        game: Box::new(castle64::perf::scene_game(
            &castle64::perf::SCENES[0],
            Vec2::new(size.0 as f32, size.1 as f32),
            &mut Mute,
        )),
        post: Some(&post),
        blur: None,
    };
    let dir = root().join("target/evidence/post/clip");
    std::fs::create_dir_all(&dir).unwrap();
    let n = 120;
    for i in 0..n {
        // Up over the first half, down over the second, eased.
        let t = 1.0 - ((i as f32 / (n - 1) as f32) * 2.0 - 1.0).abs();
        let strength = 0.3 * t * t * (3.0 - 2.0 * t);
        hub.blur = Some(SpeedBlur {
            center: Vec2::new(0.5, 0.45),
            ..SpeedBlur::new(strength)
        });
        hub.frame(&mut r);
        r.wait_gpu();
        write_frame(&r, &format!("clip/frame-{i:03}"));
    }
}
