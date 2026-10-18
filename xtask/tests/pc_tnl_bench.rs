//! Issue #51 phase 6: the PC native profile's frame time with every draw on
//! the CPU T&L (`set_hw_tnl(false)`, `TRINO_PC_TNL=cpu`) against the GPU
//! T&L path, on three scenes — castle64's hub, a Solid Eleven match and a
//! stress scene of 2000 animated robots in one instanced call — at 1080p
//! and 4K, with the runner's 4x MSAA (and the games' SSAO). Each frame is
//! the game's update, its render and the GPU's work (`wait_gpu`). A
//! measurement for ADR 0016 and the PR, not a gate:
//!
//! ```sh
//! cargo test --release -p xtask --test pc_tnl_bench -- --ignored --nocapture
//! TRINO_POWER=low cargo test --release ...   # the iGPU of a two-GPU machine
//! TRINO_POWER=high cargo test --release ...  # the discrete GPU
//! ```
//!
//! `TRINO_BENCH_FRAMES` (default 240) sets the measured frames per row. The
//! table is also written to `target/evidence/pc-gpu-tnl/bench-<adapter>.md`, and
//! each scene's last 1080p GPU frame to `scene-<name>.png` beside it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::{
    AnimRef, ClipId, Color, Game, InputState, Instance, Material, ModelId, ModelParams, Renderer,
    Transform3, Vec2, Vec3,
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

/// A native-profile renderer at `size` with `assets` (a project's asset
/// folder) baked and uploaded, MSAA on like `trino-runner-pc`.
fn renderer(assets: &str, size: (u32, u32)) -> PcRenderer {
    let baked = root().join(format!(
        "target/assets/bench-{}/pc",
        assets.replace('/', "-")
    ));
    trino_asset_pipeline::bake_all(
        &root().join(assets),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let loaded = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = pollster::block_on(PcRenderer::new_headless(SimProfile::Native))
        .unwrap_or_else(|e| panic!("no adapter: {e}"));
    r.set_internal_size(size.0, size.1);
    r.set_msaa(std::env::var("TRINO_MSAA").as_deref() != Ok("0"));
    for s in loaded.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in loaded.models {
        r.upload_mesh(ModelId(m.id), m.tmdl);
    }
    for f in loaded.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    r
}

/// What one row measured.
struct Row {
    scene: &'static str,
    size: (u32, u32),
    gpu_path: bool,
    avg_ms: f64,
    p99_ms: f64,
    tnl_ms: f64,
    draws: u32,
    gpu_draws: u32,
    draw_calls: u32,
}

/// A scene: one frame (update + render) per call.
type Frame<'a> = Box<dyn FnMut(&mut PcRenderer) + 'a>;

fn measure(scene: &'static str, r: &mut PcRenderer, gpu_path: bool, mut frame: Frame) -> Row {
    let frames: usize = std::env::var("TRINO_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(240);
    r.set_hw_tnl(gpu_path);
    let (mut times, mut tnl) = (Vec::new(), 0u64);
    for f in 0..frames + 30 {
        let t = Instant::now();
        frame(r);
        r.wait_gpu();
        if f >= 30 {
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            tnl += r.frame_times().tnl_us as u64;
        }
    }
    times.sort_by(|a, b| a.total_cmp(b));
    let stats = r.stats();
    // The GPU path's last 1080p frame, as evidence.
    if gpu_path && r.internal_size().0 == 1920 {
        let (w, h) = r.internal_size();
        let slug: String = scene
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == ' ')
            .map(|c| {
                if c == ' ' {
                    '-'
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect();
        let path = root().join(format!("target/evidence/pc-gpu-tnl/scene-{slug}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut e = png::Encoder::new(std::fs::File::create(&path).unwrap(), w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()
            .unwrap()
            .write_image_data(&r.read_offscreen())
            .unwrap();
    }
    Row {
        scene,
        size: r.internal_size(),
        gpu_path,
        avg_ms: times.iter().sum::<f64>() / times.len() as f64,
        p99_ms: times[times.len() * 99 / 100],
        tnl_ms: tnl as f64 / 1000.0 / times.len() as f64,
        draws: stats.models,
        gpu_draws: stats.tnl.pc_gpu_draws,
        draw_calls: r.frame_times().draw_calls,
    }
}

fn castle64_hub(r: &mut PcRenderer, gpu_path: bool) -> Row {
    let (w, h) = r.internal_size();
    let scene = &castle64::perf::SCENES[0];
    let mut g = castle64::perf::scene_game(scene, Vec2::new(w as f32, h as f32), &mut Mute);
    measure(
        "castle64 hub",
        r,
        gpu_path,
        Box::new(move |r| {
            g.update(&InputState::default(), &mut Mute, castle64::perf::DT);
            g.render(r);
        }),
    )
}

fn solid_eleven_match(r: &mut PcRenderer, gpu_path: bool) -> Row {
    let (w, h) = r.internal_size();
    let mut g = solid_eleven::SolidElevenGame::exhibition(
        Vec2::new(w as f32, h as f32),
        0,
        1,
        7,
        [false, false],
        solid_eleven::render::Sky::Day,
    );
    measure(
        "Solid Eleven match",
        r,
        gpu_path,
        Box::new(move |r| {
            g.update(&InputState::default(), &mut Mute, 1.0 / 60.0);
            g.render(r);
        }),
    )
}

/// 2000 robots walking in one `draw_model_instances` call (one pose, a
/// tint each) on a floor: the instancing and skinning stress.
fn robots(r: &mut PcRenderer, gpu_path: bool) -> Row {
    const ROBOT: ModelId = ModelId::from_path("models/robot");
    const FLOOR: ModelId = ModelId::from_path("models/c64_block_grass");
    let copies: Vec<Instance> = (0..2000)
        .map(|i| {
            let (x, z) = ((i % 50) as f32 - 24.5, (i / 50) as f32);
            Instance {
                tint: Color::rgb(160 + (i * 29 % 96) as u8, 200, 160 + (i * 53 % 96) as u8),
                ..Instance::new(
                    Transform3 {
                        position: Vec3::new(x * 1.2, 0.0, z * 1.2),
                        rotation: Vec3::new(0.0, std::f32::consts::PI + (i % 7) as f32 * 0.2, 0.0),
                        ..Transform3::default()
                    }
                    .matrix(),
                )
            }
        })
        .collect();
    let mut t = 0.0f32;
    measure(
        "2000 robots (instanced)",
        r,
        gpu_path,
        Box::new(move |r| {
            t += 1.0 / 60.0;
            r.begin_frame(Color::rgb(120, 160, 210));
            r.set_camera(&Camera3 {
                eye: Vec3::new(0.0, 9.0, -14.0),
                target: Vec3::new(0.0, 0.0, 18.0),
                fov_y: 60f32.to_radians(),
            });
            r.set_light(&DEFAULT_LIGHT);
            r.draw_model(
                FLOOR,
                &Transform3 {
                    position: Vec3::new(0.0, -1.0, 24.0),
                    scale: Vec3::new(64.0, 1.0, 64.0),
                    ..Transform3::default()
                },
                Material::VertexLit,
                &ModelParams::DEFAULT,
            );
            r.draw_model_instances(
                ROBOT,
                &copies,
                &ModelParams {
                    anim: Some(AnimRef::new(ClipId::from_name("walk"), t)),
                    ..ModelParams::DEFAULT
                },
            );
            r.end_frame();
        }),
    )
}

#[test]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
fn cpu_vs_gpu_tnl_frame_times() {
    let mut rows = Vec::new();
    let mut adapter = String::new();
    for size in [(1920, 1080), (3840, 2160)] {
        let mut r = renderer("assets", size);
        adapter = r.adapter_info();
        for gpu in [false, true] {
            rows.push(castle64_hub(&mut r, gpu));
            rows.push(robots(&mut r, gpu));
        }
        let mut r = renderer("examples/solid-eleven/assets", size);
        for gpu in [false, true] {
            rows.push(solid_eleven_match(&mut r, gpu));
        }
    }
    let mut out = format!(
        "Adapter: {adapter}, {}x MSAA, {} measured frames per row (update + render + GPU)\n\n\
         | Scene | Size | T&L | Frame avg (ms) | p99 (ms) | CPU T&L (ms) | Draws | On GPU | Draw calls |\n\
         |---|---|---|---:|---:|---:|---:|---:|---:|\n",
        if std::env::var("TRINO_MSAA").as_deref() == Ok("0") {
            1
        } else {
            4
        },
        std::env::var("TRINO_BENCH_FRAMES").unwrap_or("240".into()),
    );
    for name in [
        "castle64 hub",
        "Solid Eleven match",
        "2000 robots (instanced)",
    ] {
        for row in rows.iter().filter(|r| r.scene == name) {
            out += &format!(
                "| {} | {}x{} | {} | {:.2} | {:.2} | {:.2} | {} | {} | {} |\n",
                row.scene,
                row.size.0,
                row.size.1,
                if row.gpu_path { "GPU" } else { "CPU" },
                row.avg_ms,
                row.p99_ms,
                row.tnl_ms,
                row.draws,
                row.gpu_draws,
                row.draw_calls
            );
        }
    }
    println!("{out}");
    let slug: String = adapter
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let path = root().join(format!("target/evidence/pc-gpu-tnl/bench-{slug}.md"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, out).unwrap();
    println!("written to {}", path.display());
}
