//! Regenerates the README hero screenshot: the real platformer, real baked
//! assets, N64 look, 90 scripted frames. Dev utility (needs a GPU), so it
//! is `#[ignore]`d — run explicitly with:
//!
//! ```sh
//! cargo test -p xtask --test hero_shot -- --ignored
//! ```
//!
//! then commit `docs/media/platformer.png`.

use std::path::PathBuf;

use trino_core::{Button, Game, InputState, Vec2};
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

#[test]
#[ignore = "dev utility: regenerates docs/media/platformer.png (needs GPU + assets)"]
fn regenerate_readme_hero_shot() {
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
        .expect("hero shot needs a GPU adapter");
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

    let mut game = platformer::PlatformerGame::new(Vec2::new(320.0, 240.0));
    let mut audio = NullAudio;
    let mut right = InputState::default();
    right.set(Button::DpadRight, true);
    let mut jump = right;
    jump.set(Button::A, true);
    for i in 0..90 {
        let input = if i == 60 { &jump } else { &right };
        game.update(input, &mut audio, 1.0 / 60.0);
    }
    game.render(&mut renderer);

    let (w, h) = renderer.internal_size();
    let rgba = renderer.read_offscreen();

    let out = root.join("docs/media/platformer.png");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&out).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&rgba)
        .unwrap();
    eprintln!("hero shot written to {}", out.display());
}
