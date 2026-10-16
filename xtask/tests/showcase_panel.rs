//! The showcase's bottom-screen panel (`showcase::panel`) on the PC
//! backend: the 3DS profile renders both screens, and the same
//! `check_panel` the Azahar self-test runs probes the panel read back
//! (button fills by state, edges, a tap through `update`); deliberate
//! mutations must fail the probes. The N64 profile has one screen: the
//! panel collapses. GPU-gated like the goldens; the stacked two-screen
//! picture lands in `target/showcase_shots/`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use showcase::panel::{self, Toggle};
use trino_core::{Color, Game, Renderer, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

/// Renderers are created one at a time (the RADV flake in `test-all`).
static GPU: Mutex<()> = Mutex::new(());

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

/// A headless renderer on `profile` with every baked asset. `None` (skip)
/// without a GPU.
fn renderer(profile: SimProfile) -> Option<PcRenderer> {
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root().join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return None;
        }
    };
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

fn write_png(out: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let mut enc = png::Encoder::new(std::fs::File::create(out).unwrap(), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
    eprintln!("screenshot: {}", out.display());
}

fn draw3(game: &mut showcase::ShowcaseGame, r: &mut PcRenderer) {
    for _ in 0..3 {
        game.render(r);
    }
}

#[test]
fn showcase_panel_is_probed_on_the_pc_3ds_profile() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut r) = renderer(SimProfile::N3ds) else {
        return;
    };
    // The self-test's own check: probes, a top click ignored, two taps.
    assert_eq!(panel::check_panel(&mut r, &mut NullAudio), Ok(true));

    let shots = root().join("target/showcase_shots");
    let mut game = showcase::ShowcaseGame::with_screen(Vec2::new(400.0, 240.0));
    draw3(&mut game, &mut r);
    assert_eq!(panel::probe_panel(&mut r, &game), Ok(true));
    write_png(
        &shots.join("two_screens.png"),
        800,
        960,
        &r.present_preview((800, 960)),
    );
    write_png(&shots.join("bottom.png"), 320, 240, &r.read_second_screen());
    // The metrics read the renderer: the scene's triangles are on screen.
    let s = r.stats();
    assert!(s.tris > 500 && s.models > 20, "{s:?}");

    // Mutation 1: the state changed but the panel was not redrawn — the
    // probes must see the stale button.
    game.look.apply(Toggle::Fog);
    assert!(panel::probe_panel(&mut r, &game).is_err(), "stale state");
    draw3(&mut game, &mut r);
    assert_eq!(panel::probe_panel(&mut r, &game), Ok(true), "redrawn");

    // Mutation 2: a layout off by a few pixels (each axis, either way)
    // must miss: the probes sit 2 px inside and outside every edge.
    for shift in [
        Vec2::new(3.0, 0.0),
        Vec2::new(-3.0, 0.0),
        Vec2::new(0.0, 3.0),
        Vec2::new(0.0, -3.0),
    ] {
        assert!(
            panel::probe_panel_shifted(&mut r, &game, shift).is_err(),
            "shifted by {shift:?}"
        );
    }

    // Mutation 3: the panel drawn without `on_screen(Bottom)` lands on the
    // top screen, and the bottom is left blank.
    r.begin_frame(Color::rgb(132, 178, 226));
    panel::draw(&game, &mut r);
    r.end_frame();
    assert!(panel::probe_panel(&mut r, &game).is_err(), "panel on top");

    // A different look (dusk, no fog, the high camera, point filter,
    // paused): every button lit, for the picture.
    for t in [Toggle::Camera, Toggle::Light, Toggle::Filter, Toggle::Anim] {
        game.look.apply(t);
    }
    draw3(&mut game, &mut r);
    assert_eq!(panel::probe_panel(&mut r, &game), Ok(true));
    write_png(
        &shots.join("two_screens_toggled.png"),
        800,
        960,
        &r.present_preview((800, 960)),
    );
}

/// One screen (the N64 profile): nothing to probe, the panel collapses.
#[test]
fn showcase_panel_collapses_on_the_n64_profile() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut r) = renderer(SimProfile::N64) else {
        return;
    };
    assert_eq!(panel::check_panel(&mut r, &mut NullAudio), Ok(false));
    let mut game = showcase::ShowcaseGame::new();
    draw3(&mut game, &mut r);
    assert_eq!(panel::probe_panel(&mut r, &game), Ok(false));
}
