//! Evidence for issue #101 (ADR 0031), before and after on this machine.
//! Ignored: it measures the local GPU and CPUs.
//!
//! ```text
//! cargo test --release -p xtask --test precompile_shots -- --ignored --nocapture
//! ```
//!
//! Writes `target/evidence/precompile/`:
//!
//! - `timeline.json`: castle64's hub on the native profile, frame by frame,
//!   while MSAA, the color-vision views and a render scale are switched on
//!   and off. `cold` is the old behaviour (passes built the first time a
//!   frame needs them); `warm` is after `precompile(EVERYTHING)`. Each frame
//!   gives its wall time (update + render + GPU wait) and the pipelines the
//!   render thread compiled for it.
//! - `<step>_cold.png`, `<step>_warm.png`, `<step>_diff.png`: the same frames
//!   with and without the warm-up, and their difference ×16.
//! - `loading.gif`: a loading screen whose bar is `precompile_status()`,
//!   one GIF frame per rendered frame, the hub drawn once the warm-up is done.
//! - `threads.json`: the asset bake and a streamed group's decode on 1
//!   thread and on the `auto` budget (all CPUs − 1).
//!
//! The driver's disk caches are off (`MESA_SHADER_CACHE_DISABLE`,
//! `__GL_SHADER_DISK_CACHE`), or `cold` would measure Mesa's cache.

use std::path::{Path, PathBuf};
use std::time::Instant;

use trino_asset_pipeline::loader::{BundleLoad, LoadedAssets};
use trino_core::{Color, CvdMode, Game, InputState, PrecompileRequest, Renderer, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

const W: u32 = 960;
const H: u32 = 540;

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn out_dir() -> PathBuf {
    let d = root().join("target/evidence/precompile");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn renderer(assets: &LoadedAssets) -> PcRenderer {
    let mut r = pollster::block_on(PcRenderer::new_headless(SimProfile::Native))
        .expect("the evidence needs a GPU adapter");
    r.set_internal_size(W, H);
    r.set_hw_tnl(true);
    for s in &assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in &assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl.clone());
    }
    for f in &assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table.clone());
    }
    r
}

/// What happens before frame `i` of the timeline.
fn toggle(r: &mut PcRenderer, i: usize) -> Option<&'static str> {
    let modes = [
        (40, CvdMode::Protanopia, "protanopia"),
        (55, CvdMode::Deuteranopia, "deuteranopia"),
        (70, CvdMode::Tritanopia, "tritanopia"),
        (85, CvdMode::Grayscale, "grayscale"),
    ];
    match i {
        20 => {
            r.set_msaa(true);
            Some("msaa on")
        }
        100 => {
            r.set_cvd_view(CvdMode::Off);
            r.set_render_scale(0.5);
            Some("render scale 0.5")
        }
        120 => {
            r.set_render_scale(1.0);
            r.set_msaa(false);
            Some("msaa off, scale 1")
        }
        _ => modes.iter().find(|m| m.0 == i).map(|m| {
            r.set_cvd_view(m.1);
            m.2
        }),
    }
}

/// Frames whose pictures are compared (just after a toggle).
const SHOTS: [(usize, &str); 4] = [
    (25, "msaa"),
    (45, "protanopia"),
    (105, "render_scale_half"),
    (125, "plain"),
];

struct Run {
    frames: Vec<(f64, u32, Option<&'static str>)>,
    warm_ms: Option<f64>,
    shots: Vec<(&'static str, Vec<u8>)>,
}

fn run(assets: &LoadedAssets, warm: bool) -> Run {
    let mut r = renderer(assets);
    let mut audio = NullAudio;
    let mut game = castle64::Castle64Game::new(Vec2::new(W as f32, H as f32));
    let idle = InputState::default();
    // The game's first frame: its own pipelines and uploads, both runs.
    game.update(&idle, &mut audio, 1.0 / 60.0);
    game.render(&mut r);
    r.wait_gpu();
    let warm_ms = warm.then(|| {
        let t = Instant::now();
        r.precompile(&PrecompileRequest::EVERYTHING.blocking());
        ms(t)
    });
    let mut frames = Vec::new();
    let mut shots = Vec::new();
    for i in 0..140 {
        let t = Instant::now();
        let what = toggle(&mut r, i);
        game.update(&idle, &mut audio, 1.0 / 60.0);
        game.render(&mut r);
        r.wait_gpu();
        frames.push((ms(t), r.frame_times().pipelines, what));
        if let Some((_, name)) = SHOTS.iter().find(|s| s.0 == i) {
            shots.push((*name, r.read_offscreen()));
        }
    }
    Run {
        frames,
        warm_ms,
        shots,
    }
}

fn json_frames(frames: &[(f64, u32, Option<&'static str>)]) -> String {
    let items: Vec<String> = frames
        .iter()
        .map(|(t, p, w)| {
            format!(
                "{{\"ms\":{t:.3},\"pipes\":{p},\"event\":{}}}",
                w.map_or("null".to_string(), |w| format!("\"{w}\""))
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// A loading screen driven by `precompile_status()`: a bar, then the hub.
fn loading_gif(assets: &LoadedAssets, out: &Path) -> (u32, f64) {
    let mut r = renderer(assets);
    r.set_precompile_threads(1);
    let mut audio = NullAudio;
    let mut game = castle64::Castle64Game::new(Vec2::new(W as f32, H as f32));
    let idle = InputState::default();
    let (gw, gh) = (W / 2, H / 2);
    let file = std::fs::File::create(out).unwrap();
    let mut enc = gif::Encoder::new(file, gw as u16, gh as u16, &[]).unwrap();
    enc.set_repeat(gif::Repeat::Infinite).unwrap();
    let mut push = |rgba: &[u8], delay: u16| {
        // Half size: every other pixel of every other row.
        let mut small = Vec::with_capacity((gw * gh * 4) as usize);
        for y in 0..gh {
            for x in 0..gw {
                let i = (((y * 2) * W + x * 2) * 4) as usize;
                small.extend_from_slice(&rgba[i..i + 4]);
            }
        }
        let mut f = gif::Frame::from_rgba_speed(gw as u16, gh as u16, &mut small, 10);
        f.delay = delay;
        enc.write_frame(&f).unwrap();
    };
    let t0 = Instant::now();
    r.precompile(&PrecompileRequest::EVERYTHING);
    let mut frames = 0;
    while !r.precompile_status().is_done() {
        let p = r.precompile_status();
        r.begin_frame(Color::rgb(18, 20, 30));
        let (bw, bh) = (W as f32 * 0.6, 24.0);
        let (x, y) = ((W as f32 - bw) / 2.0, H as f32 * 0.62);
        r.fill_rect(
            trino_core::Rect::new(x - 3.0, y - 3.0, bw + 6.0, bh + 6.0),
            Color::rgb(70, 74, 92),
        );
        r.fill_rect(trino_core::Rect::new(x, y, bw, bh), Color::rgb(30, 32, 44));
        r.fill_rect(
            trino_core::Rect::new(x, y, bw * p.fraction(), bh),
            Color::rgb(42, 120, 214),
        );
        // One tick per pass.
        for k in 1..p.total {
            let tx = x + bw * k as f32 / p.total as f32;
            r.fill_rect(
                trino_core::Rect::new(tx - 1.0, y, 2.0, bh),
                Color::rgb(18, 20, 30),
            );
        }
        r.end_frame();
        r.wait_gpu();
        push(&r.read_offscreen(), 4);
        frames += 1;
    }
    let took = ms(t0);
    game.update(&idle, &mut audio, 1.0 / 60.0);
    game.render(&mut r);
    r.wait_gpu();
    push(&r.read_offscreen(), 200);
    (frames, took)
}

#[test]
#[ignore = "evidence: measures this machine (run with --ignored --nocapture)"]
fn precompile_evidence() {
    // SAFETY: before any device or worker thread; one test in this binary.
    unsafe {
        std::env::set_var("MESA_SHADER_CACHE_DISABLE", "true");
        std::env::set_var("__GL_SHADER_DISK_CACHE", "0");
    }
    let out = out_dir();
    let auto = trino_core::ThreadSetting::Auto
        .resolve(std::thread::available_parallelism().map_or(1, |n| n.get() as u32));

    // The bake, 1 thread vs auto, each into a fresh folder.
    // Under target/ (the system temp dir may be a RAM disk).
    std::fs::create_dir_all(root().join("target")).unwrap();
    let tmp = tempfile::tempdir_in(root().join("target")).unwrap();
    let assets_dir = root().join("assets");
    let mut bake = Vec::new();
    for threads in [1, auto] {
        let dir = tmp.path().join(format!("pc-{threads}"));
        let t = Instant::now();
        trino_asset_pipeline::bake_all_with(
            &assets_dir,
            trino_asset_pipeline::Platform::Pc,
            &dir,
            threads,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        bake.push((threads, ms(t)));
    }
    let baked = tmp.path().join(format!("pc-{auto}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();

    // A streamed group's decode, 1 thread vs auto: every sprite, model and
    // sound of the engine in one LZ4 group.
    let group_root = tmp.path().join("group/assets");
    copy_dir(&assets_dir, &group_root);
    // What the manifest reaches outside `assets/` (scenes, prefabs, UI).
    for extra in ["scenes", "prefabs", "ui"] {
        let from = root().join(extra);
        if from.is_dir() {
            copy_dir(&from, &group_root.parent().unwrap().join(extra));
        }
    }
    let manifest = std::fs::read_to_string(group_root.join("manifest.toml")).unwrap();
    let mut grouped = String::new();
    for line in manifest.lines() {
        if line.trim_start().starts_with("groups =") {
            continue;
        }
        grouped.push_str(line);
        grouped.push('\n');
        let t = line.trim();
        if ["[sprites.", "[models.", "[sounds.", "[music."]
            .iter()
            .any(|p| t.starts_with(p))
        {
            grouped.push_str("groups = [\"level\"]\n");
        }
    }
    grouped.push_str("\n[groups.level]\n");
    std::fs::write(group_root.join("manifest.toml"), grouped).unwrap();
    let group_out = tmp.path().join("group/pc");
    trino_asset_pipeline::bake_all_with(
        &group_root,
        trino_asset_pipeline::Platform::Pc,
        &group_out,
        auto,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let bundle = group_out.join("groups/level.tbnd");
    let bundle_bytes = std::fs::metadata(&bundle).unwrap().len();
    let decode = decode_times(&bundle, auto);

    // A level-sized group: 48 noisy 512×512 textures (what a streamed
    // level of a PC game holds), 48 MB decoded.
    let big_root = tmp.path().join("big/assets");
    let mut big_manifest = String::from("version = 1\n\n[groups.level]\n");
    let mut seed = 0x9e37_79b9u32;
    for i in 0..48 {
        let path = big_root.join(format!("shared/sprites/t{i}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut rgba = vec![0u8; 512 * 512 * 4];
        for (k, p) in rgba.iter_mut().enumerate() {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            // Smooth enough to compress a little, noisy enough to cost.
            *p = if k % 4 == 3 {
                255
            } else {
                ((k / 64) as u32 ^ (seed & 31)) as u8
            };
        }
        write_png(&path, 512, 512, &rgba);
        big_manifest.push_str(&format!(
            "\n[sprites.t{i}]\nfile = \"sprites/t{i}.png\"\nformats = {{ pc = \"RGBA8\" }}\ngroups = [\"level\"]\n"
        ));
    }
    std::fs::write(big_root.join("manifest.toml"), big_manifest).unwrap();
    let big_out = tmp.path().join("big/pc");
    trino_asset_pipeline::bake_all_with(
        &big_root,
        trino_asset_pipeline::Platform::Pc,
        &big_out,
        auto,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let big_bundle = big_out.join("groups/level.tbnd");
    let big_bytes = std::fs::metadata(&big_bundle).unwrap().len();
    let big = decode_times(&big_bundle, auto);

    // The timeline, cold then warm.
    let cold = run(&assets, false);
    let warm = run(&assets, true);
    for ((name, a), (_, b)) in cold.shots.iter().zip(&warm.shots) {
        write_png(&out.join(format!("{name}_cold.png")), W, H, a);
        write_png(&out.join(format!("{name}_warm.png")), W, H, b);
        let mut same = 0usize;
        let diff: Vec<u8> = a
            .chunks(4)
            .zip(b.chunks(4))
            .flat_map(|(p, q)| {
                let d = (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap_or(0);
                if d == 0 {
                    same += 1;
                }
                let v = d.saturating_mul(16);
                [v, v, v, 255]
            })
            .collect();
        write_png(&out.join(format!("{name}_diff.png")), W, H, &diff);
        println!("{name}: {} of {} pixels identical", same, (W * H) as usize);
    }
    let (gif_frames, gif_ms) = loading_gif(&assets, &out.join("loading.gif"));

    let adapter = pollster::block_on(PcRenderer::new_headless(SimProfile::Native))
        .unwrap()
        .adapter_info();
    std::fs::write(
        out.join("timeline.json"),
        format!(
            "{{\"adapter\":\"{adapter}\",\"size\":[{W},{H}],\"warm_ms\":{:.1},\"cold\":{},\"warm\":{}}}",
            warm.warm_ms.unwrap_or(0.0),
            json_frames(&cold.frames),
            json_frames(&warm.frames)
        ),
    )
    .unwrap();
    std::fs::write(
        out.join("threads.json"),
        format!(
            "{{\"cpus\":{},\"auto\":{auto},\"bake_ms\":{{\"1\":{:.0},\"auto\":{:.0}}},\
             \"decode\":{{\"assets\":{},\"bundle_bytes\":{bundle_bytes},\"1\":{:.1},\"auto\":{:.1}}},\
             \"decode_big\":{{\"assets\":{},\"bundle_bytes\":{big_bytes},\"1\":{:.1},\"auto\":{:.1}}},\
             \"loading_gif\":{{\"frames\":{gif_frames},\"ms\":{gif_ms:.0}}}}}",
            auto + 1,
            bake[0].1,
            bake[1].1,
            decode[0].2,
            decode[0].1,
            decode[1].1,
            big[0].2,
            big[0].1,
            big[1].1,
        ),
    )
    .unwrap();
    let worst = |f: &[(f64, u32, Option<&str>)]| f.iter().map(|x| x.0).fold(0.0, f64::max);
    let pipes = |f: &[(f64, u32, Option<&str>)]| f.iter().map(|x| x.1).sum::<u32>();
    println!(
        "adapter {adapter}\ncold: worst frame {:.1} ms, {} pipelines in frames\n\
         warm: worst frame {:.1} ms, {} pipelines in frames (warm-up {:.1} ms)\n\
         bake: 1 thread {:.0} ms, {auto} threads {:.0} ms\n\
         decode ({} assets, {} KB): 1 thread {:.1} ms, {auto} threads {:.1} ms\n\
         decode big ({} assets, {} KB): 1 thread {:.1} ms, {auto} threads {:.1} ms\n\
         loading screen: {gif_frames} frames, {gif_ms:.0} ms on 1 worker",
        worst(&cold.frames),
        pipes(&cold.frames),
        worst(&warm.frames),
        pipes(&warm.frames),
        warm.warm_ms.unwrap_or(0.0),
        bake[0].1,
        bake[1].1,
        decode[0].2,
        bundle_bytes / 1024,
        decode[0].1,
        decode[1].1,
        big[0].2,
        big_bytes / 1024,
        big[0].1,
        big[1].1,
    );
    assert_eq!(pipes(&warm.frames), 0, "the warm run compiled in frames");
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

/// A group's decode on 1 thread and on `auto`: (threads, best of 5 ms,
/// assets).
fn decode_times(bundle: &Path, auto: u32) -> Vec<(u32, f64, usize)> {
    let mut out = Vec::new();
    for threads in [1, auto] {
        // Best of 5: the file is in the page cache after the first.
        let mut best = f64::MAX;
        let mut count = 0;
        for _ in 0..5 {
            let mut b = BundleLoad::open(bundle, trino_core::HwTier::Base).unwrap();
            let t = Instant::now();
            count = 0;
            while !b.is_done() {
                let mut a = LoadedAssets::default();
                b.step_many(threads, &mut a).unwrap();
                count += a.sprites.len() + a.models.len() + a.sounds.len() + a.music.len();
            }
            best = best.min(ms(t));
        }
        out.push((threads, best, count));
    }
    out
}
