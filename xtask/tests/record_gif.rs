//! Records the README gameplay GIF: the real platformer with real baked
//! assets, N64 look, played by a tiny scripted bot (runs right, jumps over
//! pits and walls, collects coins). Dev utility (needs a GPU), `#[ignore]`d:
//!
//! ```sh
//! cargo test -p xtask --test record_gif -- --ignored
//! ```
//!
//! then commit `docs/media/platformer.gif`.

use std::path::PathBuf;

use trino_core::{Button, Game, InputState, Tilemap, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Hold right; jump when the ground ahead drops away (pit) or a solid tile
/// blocks the path. Good enough to clear level 1 on camera.
fn bot_input(game: &platformer::PlatformerGame, map: &Tilemap) -> InputState {
    let mut input = InputState::default();
    input.set(Button::DpadRight, true);
    if game.on_ground {
        let feet = game.pos + platformer::HERO_SIZE;
        let ahead_x = ((feet.x + 14.0) / trino_core::TILE_SIZE) as i32;
        let below_y = (feet.y / trino_core::TILE_SIZE) as i32;
        let pit_ahead = !map.is_solid(ahead_x, below_y) && !map.is_solid(ahead_x, below_y + 1);
        let body_y = ((game.pos.y + platformer::HERO_SIZE.y * 0.5) / trino_core::TILE_SIZE) as i32;
        let wall_ahead = map.is_solid(ahead_x, body_y);
        if pit_ahead || wall_ahead {
            input.set(Button::A, true);
        }
    }
    input
}

#[test]
#[ignore = "dev utility: records docs/media/platformer.gif (needs GPU + assets)"]
fn record_readme_gameplay_gif() {
    let root = repo_root();
    let baked = root.join("target/assets/pc");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();

    let mut renderer = pollster::block_on(PcRenderer::new_headless(SimProfile::N64))
        .expect("recording needs a GPU adapter");
    renderer.set_n64_look(true);
    for sprite in assets.sprites {
        renderer.upload_sprite(
            trino_core::SpriteId(sprite.id),
            sprite.width,
            sprite.height,
            &sprite.rgba,
        );
    }
    for model in assets.models {
        renderer.upload_mesh(trino_core::ModelId(model.id), model.tmdl);
    }

    let map = Tilemap::parse(platformer::LEVEL).unwrap();
    let mut game = platformer::PlatformerGame::new(Vec2::new(320.0, 240.0));
    let mut audio = NullAudio;
    let (w, h) = renderer.internal_size();

    let out = root.join("docs/media/platformer.gif");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&out).unwrap();
    let mut encoder = gif::Encoder::new(file, w as u16, h as u16, &[]).unwrap();
    encoder.set_repeat(gif::Repeat::Infinite).unwrap();

    // 60 Hz simulation, one GIF frame every 3 sim frames (20 fps), up to
    // ~14 s of play or until the flag (plus a short win tail).
    const DT: f32 = 1.0 / 60.0;
    let mut won_frames = 0u32;
    for frame in 0..840u32 {
        let input = if game.state == platformer::GameState::Won {
            won_frames += 1;
            InputState::default()
        } else {
            bot_input(&game, &map)
        };
        game.update(&input, &mut audio, DT);

        if frame % 3 == 0 {
            game.render(&mut renderer);
            let mut rgba = renderer.read_offscreen();
            let mut gif_frame = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut rgba, 10);
            gif_frame.delay = 5; // 50 ms = 20 fps
            encoder.write_frame(&gif_frame).unwrap();
        }
        if won_frames > 90 {
            break;
        }
    }
    drop(encoder);

    assert!(
        game.state == platformer::GameState::Won,
        "the bot must reach the flag on camera (got stuck at x={})",
        game.pos.x
    );
    eprintln!("gameplay gif written to {}", out.display());
}
