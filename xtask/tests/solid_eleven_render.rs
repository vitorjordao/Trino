//! Solid Eleven's golden frames on the PC renderer: the self-test's match
//! after its scripted 300 frames, on the N64 look, the 3DS look (both
//! screens) and the native profile, and the title and team screens.
//!
//! 3D edges rasterize a little differently from one GPU to another
//! (lavapipe in CI, a desktop GPU here), so frames compare by mean
//! difference and by the share of far-off pixels, like `n3ds_parity`.
//! Re-bless after an intended change with
//! `TRINO_BLESS=1 cargo test -p xtask --test solid_eleven_render` and
//! review the PNGs in `tests/golden/pc/solid_eleven_*`.

use std::path::{Path, PathBuf};

use solid_eleven::SolidElevenGame;
use solid_eleven::game::app::Scene;
use solid_eleven::selftest;
use trino_core::{Game, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

struct Mute;
impl trino_core::Audio for Mute {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// One GPU test at a time (RADV crashed creating devices from parallel
/// threads, see `local_lights.rs`).
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn renderer(profile: SimProfile, size: Option<(u32, u32)>) -> Option<PcRenderer> {
    let baked = root().join("target/assets/solid-eleven/pc");
    trino_asset_pipeline::bake_all(
        &root().join("examples/solid-eleven/assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => r,
        Err(e) => {
            if std::env::var("TRINO_REQUIRE_GPU").is_ok() {
                panic!("TRINO_REQUIRE_GPU set but no adapter: {e}");
            }
            eprintln!("skipping: no GPU adapter ({e})");
            return None;
        }
    };
    if let Some((w, h)) = size {
        r.set_internal_size(w, h);
    }
    if profile == SimProfile::N64 {
        r.set_n64_look(true);
    }
    for s in assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    for f in assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    Some(r)
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// Compare (or bless) `tests/golden/pc/solid_eleven_<name>.png`.
fn golden(name: &str, w: u32, h: u32, img: &[u8]) {
    let dir = root().join("tests/golden/pc");
    let path = dir.join(format!("solid_eleven_{name}.png"));
    if std::env::var("TRINO_BLESS").is_ok() {
        write_png(&path, w, h, img);
        eprintln!("blessed {}", path.display());
        return;
    }
    assert!(path.exists(), "missing {}: bless it", path.display());
    let (gw, gh, want) = read_png(&path);
    assert_eq!((gw, gh), (w, h), "{name}: size");
    let mut sum = 0u64;
    let mut far = 0usize;
    for (a, b) in img.chunks(4).zip(want.chunks(4)) {
        let d: u32 = (0..3).map(|k| a[k].abs_diff(b[k]) as u32).sum::<u32>() / 3;
        sum += d as u64;
        if d > 48 {
            far += 1;
        }
    }
    let px = (w * h) as usize;
    let mean = sum as f32 / px as f32;
    let far_share = far as f32 / px as f32;
    if mean > 3.0 || far_share > 0.02 {
        write_png(
            &dir.join(format!("actual/solid_eleven_{name}.png")),
            w,
            h,
            img,
        );
        panic!(
            "{name}: mean diff {mean:.2} (max 3.0), {:.2}% far pixels (max 2%) — actual in tests/golden/pc/actual/",
            far_share * 100.0
        );
    }
}

/// The game after the self-test's script, on `profile`.
fn scripted(screen: Vec2) -> SolidElevenGame {
    let mut g = SolidElevenGame::new(screen);
    selftest::begin(&mut g);
    for f in 0..selftest::FRAMES {
        g.update(&selftest::input(f), &mut Mute, selftest::DT);
    }
    g
}

#[test]
fn the_match_matches_its_goldens_on_every_look() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    for (profile, tag, size) in [
        (SimProfile::N64, "match_n64", None),
        (SimProfile::N3ds, "match_n3ds", None),
        (SimProfile::Native, "match_native", Some((640u32, 360u32))),
    ] {
        let Some(mut r) = renderer(profile, size) else {
            return;
        };
        let (w, h) = r.internal_size();
        let mut g = scripted(Vec2::new(w as f32, h as f32));
        g.render(&mut r);
        golden(tag, w, h, &r.read_offscreen());
        if profile == SimProfile::N3ds {
            golden("match_n3ds_bottom", 320, 240, &r.read_second_screen());
        }
    }
}

#[test]
fn the_menus_match_their_goldens() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    for (profile, tag, size, scene) in [
        (SimProfile::N64, "title_n64", None, Scene::Title),
        (
            SimProfile::Native,
            "teams_native",
            Some((640u32, 360u32)),
            Scene::Teams,
        ),
    ] {
        let Some(mut r) = renderer(profile, size) else {
            return;
        };
        let (w, h) = r.internal_size();
        let mut g = SolidElevenGame::new(Vec2::new(w as f32, h as f32));
        // A fixed moment of the menus' animations.
        for _ in 0..20 {
            g.update(&trino_core::InputState::default(), &mut Mute, 1.0 / 30.0);
        }
        g.scene = scene;
        g.render(&mut r);
        golden(tag, w, h, &r.read_offscreen());
    }
}

/// PC strict mode on the N64 profile holds the match to the console's
/// budgets, splitting each frame's 3D the way the N64 does (its RSP takes
/// the eligible draws): it used to price every draw as CPU T&L and fail
/// Solid Eleven at ~100 draws while ares ran it in time.
#[test]
fn the_match_passes_strict_mode_on_the_n64_profile() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut r) = renderer(SimProfile::N64, None) else {
        return;
    };
    r.set_strict(true);
    let (w, h) = r.internal_size();
    let mut g = SolidElevenGame::new(Vec2::new(w as f32, h as f32));
    selftest::begin(&mut g);
    let mut worst = trino_platform_pc::N64Estimate::default();
    for f in 0..selftest::FRAMES {
        g.update(&selftest::input(f), &mut Mute, selftest::DT);
        if f % 5 == 0 {
            g.render(&mut r);
            let e = r.n64_estimate();
            if e.cpu_us.max(e.rsp_us) > worst.cpu_us.max(worst.rsp_us) {
                worst = e;
            }
        }
    }
    eprintln!(
        "worst frame: CPU ~{:.1} ms ({} draws on it, {} on the RSP), RSP ~{:.1} ms ({} triangles)",
        worst.cpu_us as f32 / 1000.0,
        worst.cpu_draws,
        worst.rsp_draws,
        worst.rsp_us as f32 / 1000.0,
        worst.rsp_tris
    );
    assert_eq!(worst.cpu_draws, 0, "every match draw is the RSP's");
    assert!(worst.rsp_draws > 50);
}

/// The PC profile's best look: multisampled edges (`set_msaa`) blend
/// triangle and sprite edges with what is behind them — more in-between
/// colors along the players and the lines — and leave the rest alone.
#[test]
fn msaa_smooths_the_match_edges_on_the_pc_profile() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut r) = renderer(SimProfile::Native, Some((640, 360))) else {
        return;
    };
    let (w, h) = r.internal_size();
    let shot = |r: &mut PcRenderer| {
        let mut g = scripted(Vec2::new(w as f32, h as f32));
        g.render(r);
        r.read_offscreen()
    };
    let plain = shot(&mut r);
    r.set_msaa(true);
    assert_eq!(r.msaa(), trino_platform_pc::MSAA_SAMPLES);
    let smooth = shot(&mut r);
    let colors = |img: &[u8]| {
        let mut set = std::collections::HashSet::new();
        for p in img.chunks(4) {
            set.insert((p[0], p[1], p[2]));
        }
        set.len()
    };
    let (a, b) = (colors(&plain), colors(&smooth));
    let changed = plain
        .chunks(4)
        .zip(smooth.chunks(4))
        .filter(|(x, y)| x != y)
        .count();
    eprintln!("colors: {a} single-sampled, {b} multisampled; {changed} pixels differ");
    assert!(b as f32 > a as f32 * 1.15, "{a} -> {b}");
    // Only edges change: most of the picture is identical.
    assert!(changed * 4 < (w * h) as usize, "{changed} of {}", w * h);
    // Console profiles ignore it.
    if let Some(mut n64) = renderer(SimProfile::N64, None) {
        n64.set_msaa(true);
        assert_eq!(n64.msaa(), 1);
    }
}
