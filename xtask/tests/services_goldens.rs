//! The achievements overlay on the PC renderer (issue #61): castle64's
//! baked table (`assets/achievements.ron`) and English strings drawn by
//! `trino_core::AchievementList` (two unlocked, two on their way with a
//! progress bar, one hidden) and by `AchievementToast`, on the N64, 3DS and
//! native profiles — the same core code every console runs
//! (`castle64::selftest::services` probes it in ares and Azahar). Goldens
//! `tests/golden/pc/services_{list,toast}_{n64,3ds,pc}.png`: 2D only, so
//! they hold on every driver (lavapipe, WARP, Metal).
//!
//! The popup over castle64's hub (3D, SSAO on the PC: driver noise) is
//! evidence, not a golden: `cargo test -p xtask --test services_goldens --
//! --ignored` writes `target/evidence/services/`.
//!
//! GPU-gated like the other goldens (`TRINO_REQUIRE_GPU=1` turns a missing
//! adapter into a failure); `TRINO_BLESS=1` rewrites them.

use std::path::{Path, PathBuf};

use castle64::{ACH_FIRST_COIN, ACH_FIRST_STAR, STAT_COINS, STAT_STARS};
use trino_core::{
    AchievementList, AchievementTable, Color, FontId, Game, InputState, ModelId, OverlayStyle,
    Platform, Renderer, Vec2,
};
use trino_platform_pc::{PcPlatform, PcRenderer, SimProfile};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// A renderer with the engine's baked assets and a platform with its
/// blobs; `None` (skip) without a GPU.
fn target(profile: SimProfile) -> Option<(PcRenderer, PcPlatform)> {
    let root = repo();
    let baked = root.join("target/test-projects/services-goldens");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let mut p = PcPlatform::new(&profile.caps());
    for s in assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for f in assets.fonts {
        r.upload_font(FontId(f.id), f.table);
    }
    for m in assets.models {
        r.upload_mesh(ModelId(m.id), m.tmdl);
    }
    for b in assets.blobs {
        p.insert_blob(trino_core::BlobId(b.id), b.bytes);
    }
    Some((r, p))
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let f = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// One RGBA5551 step and the dither ties drivers round differently (as in
/// the UI goldens on the N64 profile); 3D in the toast's frame too.
fn check(name: &str, w: u32, h: u32, rgba: &[u8], failures: &mut Vec<String>) {
    let golden = repo().join("tests/golden/pc");
    let path = golden.join(format!("{name}.png"));
    if std::env::var("TRINO_BLESS").is_ok() {
        write_png(&path, w, h, rgba);
        eprintln!("blessed golden: {}", path.display());
        return;
    }
    assert!(
        path.exists(),
        "golden missing at {} — run `cargo xtask test --bless` once and commit it",
        path.display()
    );
    let (gw, gh, want) = read_png(&path);
    assert_eq!((gw, gh), (w, h), "{name}: size");
    let bad = rgba
        .iter()
        .zip(&want)
        .filter(|(a, b)| a.abs_diff(**b) > 9)
        .count();
    if bad > (w * h / 100) as usize {
        let actual = golden.join(format!("actual/{name}.png"));
        write_png(&actual, w, h, rgba);
        failures.push(format!(
            "{name}: {bad} channel values differ by more than 9 (actual at {})",
            actual.display()
        ));
    }
}

struct NullAudio;

impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

#[test]
fn the_overlay_matches_its_goldens_on_every_profile() {
    let mut failures = Vec::new();
    for (profile, key) in [
        (SimProfile::N64, "n64"),
        (SimProfile::N3ds, "3ds"),
        (SimProfile::Native, "pc"),
    ] {
        let Some((mut r, mut p)) = target(profile) else {
            return;
        };
        let table = AchievementTable::from_platform(&p).expect("services/achievements baked");
        let mut en = trino_core::ui::StringsBlob::new("en-US");
        let strings = *en.get(&p).expect("i18n/en-US baked");

        // The list: a coin and a star, 12 of 25 coins, 2 of 4 stars, the
        // hidden one locked.
        p.unlock(ACH_FIRST_COIN);
        p.unlock(ACH_FIRST_STAR);
        p.set_stat(STAT_COINS, 12);
        p.set_stat(STAT_STARS, 2);
        let state = p.achievements();
        assert_eq!(state.unlocked_count(), 2);
        let style = OverlayStyle::DEFAULT;
        let mut list = AchievementList::new();
        list.focus = 1;
        r.begin_frame(Color::rgb(30, 60, 110));
        list.draw(&mut r, &strings, &table, &state, &style);
        r.end_frame();
        let (w, h) = r.internal_size();
        let rgba = r.read_offscreen();
        check(&format!("services_list_{key}"), w, h, &rgba, &mut failures);

        // The popup, fully in, over a flat sky.
        let mut toast = trino_core::AchievementToast::new();
        toast.push(0);
        toast.advance(1.0);
        r.begin_frame(Color::rgb(110, 170, 230));
        toast.draw(&mut r, &strings, &table, &style);
        r.end_frame();
        let rgba = r.read_offscreen();
        check(&format!("services_toast_{key}"), w, h, &rgba, &mut failures);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Evidence: castle64's popup over its hub after a coin, and its list after
/// a coin and a star, on each profile (`target/evidence/services/`).
#[test]
#[ignore = "writes evidence PNGs: run with --ignored"]
fn castle64_popup_and_list_evidence() {
    let out = repo().join("target/evidence/services");
    for (profile, key) in [
        (SimProfile::N64, "n64"),
        (SimProfile::N3ds, "3ds"),
        (SimProfile::Native, "pc"),
    ] {
        let Some((mut r, mut p)) = target(profile) else {
            return;
        };
        let (w, h) = r.internal_size();
        let mut g = castle64::Castle64Game::new(Vec2::new(w as f32, h as f32));
        let mut audio = NullAudio;
        g.update_with(&mut p, &InputState::default(), &mut audio, 1.0 / 60.0);
        p.unlock(ACH_FIRST_COIN);
        g.update_with(&mut p, &InputState::default(), &mut audio, 1.0);
        g.render(&mut r);
        write_png(
            &out.join(format!("castle64_popup_{key}.png")),
            w,
            h,
            &r.read_offscreen(),
        );
        p.unlock(ACH_FIRST_STAR);
        p.set_stat(STAT_COINS, 12);
        let mut y = InputState::default();
        y.set(trino_core::Button::Y, true);
        g.update_with(&mut p, &y, &mut audio, 1.0 / 60.0);
        g.render(&mut r);
        write_png(
            &out.join(format!("castle64_list_{key}.png")),
            w,
            h,
            &r.read_offscreen(),
        );
    }
}
