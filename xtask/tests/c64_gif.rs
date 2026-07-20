//! Grava um GIF do bot JOGANDO o castle64 (hub → green hills → estrela) no
//! renderer real com look N64 — prova visual do playtest de ponta a ponta.
//! Local, NÃO commitar (está em .git/info/exclude).
//!
//! ```sh
//! cargo test -p xtask --test c64_gif -- --ignored
//! ```
//!
//! Escreve target/c64_shots/gameplay.gif.

use std::path::PathBuf;

use trino_core::{Game, Vec2};
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
#[ignore = "utilitário local: grava target/c64_shots/gameplay.gif (GPU + assets)"]
fn record_castle64_gameplay_gif() {
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
        .expect("gravação precisa de um adapter de GPU");
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

    let mut game = castle64::Castle64Game::new(Vec2::new(320.0, 240.0));
    let mut bot = castle64::bot::Bot::new(castle64::bot::GREEN_RUN);
    let mut audio = NullAudio;
    let (w, h) = renderer.internal_size();

    let out_dir = root.join("target/c64_shots");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out = out_dir.join("gameplay.gif");
    let file = std::fs::File::create(&out).unwrap();
    let mut encoder = gif::Encoder::new(file, w as u16, h as u16, &[]).unwrap();
    encoder.set_repeat(gif::Repeat::Infinite).unwrap();

    // 60 Hz de simulação, um frame de GIF a cada 3 (20 fps).
    const DT: f32 = 1.0 / 60.0;
    let mut frames = 0u32;
    let mut tail = 0u32;
    loop {
        let input = bot.drive(&game);
        game.update(&input, &mut audio, DT);
        frames += 1;
        assert!(
            bot.frames_in_step() < 1800,
            "bot travou no passo {} durante a gravação",
            bot.step_index()
        );
        assert!(frames < 60 * 90, "gravação longa demais");
        if bot.done() {
            tail += 1; // segura uns instantes no hub após a estrela
        }
        if frames.is_multiple_of(3) {
            game.render(&mut renderer);
            let mut rgba = renderer.read_offscreen();
            let mut gif_frame = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut rgba, 10);
            gif_frame.delay = 5; // 50 ms = 20 fps
            encoder.write_frame(&gif_frame).unwrap();
        }
        if tail > 60 {
            break;
        }
    }
    drop(encoder);

    assert_eq!(game.star_count(), 1, "o bot deve terminar com a estrela");
    eprintln!("gameplay gif: {}", out.display());
}
