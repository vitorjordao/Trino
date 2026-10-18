//! Evidence for the PC's and the Web's quality presets (issue #55, ADR
//! 0022). Not a gate:
//!
//! ```sh
//! cargo test -p xtask --test quality_shots -- --ignored --nocapture
//! ```
//!
//! Writes `target/evidence/quality/`:
//! - `castle64_presets.png`: castle64's hub at Low, Medium, High and
//!   Ultra (2×2: each preset's render scale, MSAA, AO and LOD bias, the
//!   HUD at the picture's resolution);
//! - `castle64_presets_zoom.png`: the same, 3× on the boxes and a door;
//! - `castle64_graphics_page.png`: the ready-made Graphics page
//!   (`trino_core::GraphicsOptions`) over the game on a PC platform;
//! - `dynres.png`, `dynres.csv`: dynamic resolution on a frame too heavy
//!   for the GPU at full scale (supersampled, a range up to 2×, when even
//!   4K holds 16.6 ms) — the measured frame time, the GPU's and the scale
//!   per frame settling on a 16.6 ms target, then a lighter target;
//! - `pacing.txt`: the frame limiter's measured rates at 30, 60 and 120
//!   fps, and the calibration bench's line on this adapter.

mod ao_evidence;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use ao_evidence::{NullAudio, assets, renderer, root};
use trino_core::{
    Button, Caps, DynRes, FrameLimit, Game, InputState, PresetTable, QualityInfo, QualityPreset,
    QualitySettings, QualitySource, Vec2,
};
use trino_platform_pc::pacing::FrameLimiter;
use trino_platform_pc::{PcPlatform, PcRenderer, SimProfile};

fn out() -> std::path::PathBuf {
    root().join("target/evidence/quality")
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
    eprintln!("evidence: {}", path.display());
}

/// Panels in a `cols`-wide grid, 4 px gutters.
fn grid(path: &std::path::Path, (w, h): (u32, u32), cols: u32, panels: &[Vec<u8>]) {
    let rows = (panels.len() as u32).div_ceil(cols);
    let gap = 4;
    let (tw, th) = (cols * w + (cols - 1) * gap, rows * h + (rows - 1) * gap);
    let mut img = vec![0u8; (tw * th * 4) as usize];
    for px in img.chunks_mut(4) {
        px.copy_from_slice(&[20, 20, 24, 255]);
    }
    for (i, p) in panels.iter().enumerate() {
        let (cx, cy) = (i as u32 % cols, i as u32 / cols);
        let (x0, y0) = (cx * (w + gap), cy * (h + gap));
        for y in 0..h {
            let src = &p[(y * w * 4) as usize..((y + 1) * w * 4) as usize];
            let at = (((y0 + y) * tw + x0) * 4) as usize;
            img[at..at + src.len()].copy_from_slice(src);
        }
    }
    write_png(path, tw, th, &img);
}

fn hub(screen: Vec2) -> castle64::Castle64Game {
    let mut game = castle64::Castle64Game::new(screen);
    let idle = InputState::default();
    for _ in 0..30 {
        game.update(&idle, &mut NullAudio, 1.0 / 60.0);
    }
    game
}

/// One fixed frame: the preset without dynamic resolution.
fn fixed(p: QualityPreset) -> QualitySettings {
    QualitySettings {
        dynamic_resolution: None,
        ..QualitySettings::preset(p)
    }
}

#[test]
#[ignore = "evidence: writes target/evidence/quality (needs a GPU)"]
fn quality_presets_evidence() {
    let assets = assets();
    let mut r = renderer(SimProfile::Native, &assets);
    let size = (960u32, 540u32);
    r.set_internal_size(size.0, size.1);
    let screen = Vec2::new(size.0 as f32, size.1 as f32);
    let mut game = hub(screen);

    // The four presets.
    let panels: Vec<Vec<u8>> = QualityPreset::NAMED
        .into_iter()
        .map(|p| {
            r.apply_quality(&fixed(p));
            game.render(&mut r);
            eprintln!(
                "{}: 3D {:.2}x, MSAA {}, LOD bias {}",
                p.label(),
                r.render_scale(),
                r.msaa(),
                trino_core::Renderer::lod_bias(&r)
            );
            r.read_offscreen()
        })
        .collect();
    grid(&out().join("castle64_presets.png"), size, 2, &panels);
    // 3× crops of the boxes and the door, where the edges, AO and
    // supersampling show.
    let (cx, cy, cw, ch, k) = (600u32, 60u32, 300u32, 150u32, 3u32);
    let crops: Vec<Vec<u8>> = panels
        .iter()
        .map(|p| {
            let mut c = vec![0u8; (cw * k * ch * k * 4) as usize];
            for y in 0..ch * k {
                for x in 0..cw * k {
                    let src = (((cy + y / k) * size.0 + cx + x / k) * 4) as usize;
                    let dst = ((y * cw * k + x) * 4) as usize;
                    c[dst..dst + 4].copy_from_slice(&p[src..src + 4]);
                }
            }
            c
        })
        .collect();
    grid(
        &out().join("castle64_presets_zoom.png"),
        (cw * k, ch * k),
        2,
        &crops,
    );

    // The Graphics page over the game, on a PC platform whose host
    // published a calibration's pick; Render scale picked.
    let mut platform = PcPlatform::new(&Caps::PC);
    platform.set_quality_presets(PresetTable::BUILT_IN);
    let q = QualitySettings::MEDIUM;
    r.apply_quality(&QualitySettings {
        dynamic_resolution: None,
        render_scale: 0.85,
        ..q
    });
    platform.set_quality_info(Some(QualityInfo {
        settings: q,
        scale: 0.85,
        detected: Some(QualityPreset::Medium),
        source: QualitySource::Detected,
        gpu: r.gpu_class(),
        handheld: false,
        gpu_us: 9_400,
    }));
    let press = |b: Button| {
        let mut s = InputState::default();
        s.set(b, true);
        s
    };
    let idle = InputState::default();
    for input in [press(Button::Select), idle, press(Button::DpadDown), idle] {
        game.update_with(&mut platform, &input, &mut NullAudio, 1.0 / 60.0);
    }
    assert!(game.graphics.is_open());
    game.render(&mut r);
    let page = r.read_offscreen();
    write_png(
        &out().join("castle64_graphics_page.png"),
        size.0,
        size.1,
        &page,
    );
}

/// The frame's time, µs: draw, then wait for the GPU.
fn timed(r: &mut PcRenderer, game: &mut castle64::Castle64Game) -> u32 {
    let t = Instant::now();
    game.render(r);
    r.wait_gpu();
    t.elapsed().as_micros() as u32
}

#[test]
#[ignore = "evidence: writes target/evidence/quality (needs a GPU)"]
fn dynamic_resolution_and_pacing_evidence() {
    let assets = assets();
    let mut r = renderer(SimProfile::Native, &assets);
    // A picture heavy enough that full scale misses 16.6 ms here, or 4K
    // with a range up to 2× when even that holds it (a fast GPU).
    let mut size = (1280u32, 720u32);
    let mut full = 0;
    for s in [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)] {
        size = s;
        r.set_internal_size(s.0, s.1);
        let mut game = hub(Vec2::new(s.0 as f32, s.1 as f32));
        r.apply_quality(&fixed(QualityPreset::High));
        for _ in 0..3 {
            timed(&mut r, &mut game);
        }
        full = (0..5).map(|_| timed(&mut r, &mut game)).min().unwrap();
        if full > 24_000 {
            break;
        }
    }
    let top = if full > 24_000 { 1.0 } else { 2.0 };
    let mut game = hub(Vec2::new(size.0 as f32, size.1 as f32));
    let target = |us| QualitySettings {
        dynamic_resolution: Some(DynRes {
            target_us: us,
            min_scale: 0.35,
            max_scale: top,
        }),
        frame_limit: FrameLimit::Unlimited,
        ..QualitySettings::HIGH
    };
    let mut csv = String::from("frame,target_us,frame_us,gpu_us,scale\n");
    let mut rows = Vec::new();
    let (first, second) = (16_600u32, if top > 1.0 { 10_000 } else { full * 7 / 10 });
    for (i, t) in std::iter::repeat_n(first, 240)
        .chain(std::iter::repeat_n(second, 160))
        .enumerate()
    {
        if i == 0 || i == 240 {
            r.apply_quality(&target(t));
        }
        let us = timed(&mut r, &mut game);
        r.report_frame_time(us);
        let scale = r.render_scale();
        let gpu = r.gpu_frame_us();
        let _ = writeln!(csv, "{i},{t},{us},{gpu},{scale:.3}");
        rows.push((t, us, scale, gpu));
    }
    std::fs::create_dir_all(out()).unwrap();
    std::fs::write(out().join("dynres.csv"), &csv).unwrap();
    // The last 60 frames at each target: their medians against it (the
    // frame as the host times it, and the GPU's part the controller reads
    // where the adapter has timestamps).
    let median = |v: &mut Vec<u32>| {
        v.sort_unstable();
        v[v.len() / 2] as f32 / 1000.0
    };
    let held = |from: usize, col: usize| {
        let mut v: Vec<u32> = rows[from..from + 60]
            .iter()
            .map(|r| if col == 0 { r.1 } else { r.3 })
            .collect();
        median(&mut v)
    };
    let mut report = format!(
        "dynres: {}x{} on {} ({}): full scale {:.1} ms, range 0.35-{top}x\n\
         target 16.6 ms -> frame median {:.2} ms, GPU median {:.2} ms at scale {:.2} (frames 180-239)\n\
         target {:.1} ms -> frame median {:.2} ms, GPU median {:.2} ms at scale {:.2} (frames 340-399)\n",
        size.0,
        size.1,
        r.adapter_info(),
        r.gpu_class().key(),
        full as f32 / 1000.0,
        held(180, 0),
        held(180, 1),
        rows[239].2,
        second as f32 / 1000.0,
        held(340, 0),
        held(340, 1),
        rows[399].2
    );
    chart(&out().join("dynres.png"), &rows);

    // The frame limiter: 2 s at each cap.
    for fps in [30u16, 60, 120] {
        let mut l = FrameLimiter::new(FrameLimit::Fps(fps));
        let n = fps as u32 * 2;
        l.wait();
        let t = Instant::now();
        for _ in 0..n {
            std::thread::sleep(Duration::from_micros(1_000));
            l.wait();
        }
        let got = n as f64 / t.elapsed().as_secs_f64();
        let _ = writeln!(
            report,
            "limiter: cap {fps} fps -> {got:.2} fps ({:+.2}%)",
            (got / fps as f64 - 1.0) * 100.0
        );
    }
    // The calibration on this adapter, at 640x480.
    r.set_internal_size(640, 480);
    let cal = r.calibrate(&PresetTable::BUILT_IN, 15_000, Duration::from_secs(2));
    let _ = writeln!(report, "bench: {}", cal.summary());
    eprint!("{report}");
    std::fs::write(out().join("pacing.txt"), &report).unwrap();
    assert!(cal.elapsed <= Duration::from_secs(3));
}

/// The frame time (orange), its target (green) and the scale (blue, 0 to
/// its top) per frame, on a dark ground with a grid every 5 ms.
fn chart(path: &std::path::Path, rows: &[(u32, u32, f32, u32)]) {
    let (w, h) = (1000u32, 420u32);
    let mut img = vec![0u8; (w * h * 4) as usize];
    for px in img.chunks_mut(4) {
        px.copy_from_slice(&[18, 20, 28, 255]);
    }
    let max_us = rows.iter().map(|r| r.1.max(r.0)).max().unwrap_or(1) as f32 * 1.1;
    let top = rows.iter().map(|r| r.2).fold(1.0f32, f32::max).ceil();
    let mut put = |x: i32, y: i32, c: [u8; 3]| {
        if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
            let at = ((y as u32 * w + x as u32) * 4) as usize;
            img[at..at + 3].copy_from_slice(&c);
        }
    };
    let (pad, plot_h) = (20i32, h as i32 - 40);
    let y_of = |v: f32| pad + plot_h - (v * plot_h as f32) as i32;
    let mut ms = 0.0;
    while ms * 1000.0 < max_us {
        let y = y_of(ms * 1000.0 / max_us);
        for x in 0..w as i32 {
            if x % 4 == 0 {
                put(x, y, [50, 54, 66]);
            }
        }
        ms += 5.0;
    }
    let x_of = |i: usize| (i as f32 * (w as f32 - 1.0) / (rows.len() as f32 - 1.0)) as i32;
    let mut line = |a: (i32, i32), b: (i32, i32), c: [u8; 3]| {
        let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).max(1);
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let x = a.0 + ((b.0 - a.0) as f32 * t) as i32;
            let y = a.1 + ((b.1 - a.1) as f32 * t) as i32;
            put(x, y, c);
            put(x, y + 1, c);
        }
    };
    for i in 1..rows.len() {
        let (p, q) = (rows[i - 1], rows[i]);
        line(
            (x_of(i - 1), y_of(p.0 as f32 / max_us)),
            (x_of(i), y_of(q.0 as f32 / max_us)),
            [90, 200, 110],
        );
        line(
            (x_of(i - 1), y_of(p.1 as f32 / max_us)),
            (x_of(i), y_of(q.1 as f32 / max_us)),
            [255, 160, 70],
        );
        line(
            (x_of(i - 1), y_of(p.2 / top)),
            (x_of(i), y_of(q.2 / top)),
            [100, 160, 255],
        );
    }
    write_png(path, w, h, &img);
}
