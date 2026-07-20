//! Screenshots headless do castle64 (verificação visual local — NÃO commitar;
//! está em .git/info/exclude). Roda com:
//!
//! ```sh
//! cargo test -p xtask --test c64_shots -- --ignored
//! ```
//!
//! Escreve target/c64_shots/*.png com o look N64 e STRICT mode ligado
//! (valida os orçamentos de tris reais, incluindo a subdivisão).
//!
//! Além das vistas de spawn, cobre os cenários dos bugs visuais reportados:
//! - `hub_jump`: player no ar + sombra no chão (chão não pode cobrir ambos);
//! - `green_right`: andando para a direita na fase 1 (o "chão subindo");
//! - `hub_doorview`: câmera orbitada olhando as portas do castelo (popping).

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

fn save(renderer: &mut PcRenderer, out_dir: &std::path::Path, name: &str) {
    let (w, h) = renderer.internal_size();
    let rgba = renderer.read_offscreen();
    let out = out_dir.join(format!("{name}.png"));
    let file = std::fs::File::create(&out).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&rgba)
        .unwrap();
    eprintln!("screenshot: {}", out.display());
}

#[test]
#[ignore = "utilitário local: screenshots do castle64 (precisa de GPU + assets)"]
fn castle64_level_shots() {
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
        .expect("screenshots precisam de um adapter de GPU");
    renderer.set_n64_look(true);
    renderer.set_strict(true); // orçamentos N64 valem para TODAS as cenas
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
    let out_dir = root.join("target/c64_shots");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut audio = NullAudio;
    const DT: f32 = 1.0 / 60.0;
    let idle = InputState::default();

    // Vistas de spawn de cada nível.
    for level in 0..5usize {
        let mut game = castle64::Castle64Game::new(Vec2::new(320.0, 240.0));
        game.goto_level(level);
        for _ in 0..120 {
            game.update(&idle, &mut audio, DT);
        }
        game.render(&mut renderer);
        save(&mut renderer, &out_dir, &format!("level{level}"));
    }

    // Bug 1: chão sobre player/sombra — pulo no meio do pátio.
    {
        let mut game = castle64::Castle64Game::new(Vec2::new(320.0, 240.0));
        for _ in 0..60 {
            game.update(&idle, &mut audio, DT);
        }
        let mut jump = InputState::default();
        jump.set(Button::A, true);
        game.update(&jump, &mut audio, DT);
        for _ in 0..14 {
            game.update(&idle, &mut audio, DT); // ápice do pulo
        }
        game.render(&mut renderer);
        save(&mut renderer, &out_dir, "hub_jump");
    }

    // Bug 3: "chão subindo" avançando na fase 1 — anda na direção da
    // progressão (frente da câmera) até a beirada da ilha inicial.
    {
        let mut game = castle64::Castle64Game::new(Vec2::new(320.0, 240.0));
        game.goto_level(1);
        for _ in 0..30 {
            game.update(&idle, &mut audio, DT);
        }
        let mut fwd = InputState::default();
        fwd.stick = Vec2::new(0.0, 1.0);
        for _ in 0..40 {
            game.update(&fwd, &mut audio, DT);
        }
        game.render(&mut renderer);
        save(&mut renderer, &out_dir, "green_mid");
        // Beirada + diagonal do quad do topo em ângulo rasante.
        let mut diag = InputState::default();
        diag.stick = Vec2::new(-0.5, 0.8);
        for _ in 0..25 {
            game.update(&diag, &mut audio, DT);
        }
        game.render(&mut renderer);
        save(&mut renderer, &out_dir, "green_edge");
    }

    // Bug 2: portas sumindo — câmera orbitada perto do castelo.
    {
        let mut game = castle64::Castle64Game::new(Vec2::new(320.0, 240.0));
        for _ in 0..30 {
            game.update(&idle, &mut audio, DT);
        }
        let mut fwd = InputState::default();
        fwd.stick = Vec2::new(0.15, 1.0);
        for _ in 0..80 {
            game.update(&fwd, &mut audio, DT);
        }
        let mut orbit = InputState::default();
        orbit.set(Button::L, true);
        for _ in 0..40 {
            game.update(&orbit, &mut audio, DT);
        }
        game.render(&mut renderer);
        save(&mut renderer, &out_dir, "hub_doorview");
    }

    // Interpenetração (boné/cabeça): player de COSTAS, câmera de frente
    // para ele — o z-buffer tem que resolver as caixas que se atravessam.
    {
        let mut game = castle64::Castle64Game::new(Vec2::new(320.0, 240.0));
        for _ in 0..30 {
            game.update(&idle, &mut audio, DT);
        }
        // Anda para trás (de costas para a câmera) e para.
        let mut back = InputState::default();
        back.stick = Vec2::new(0.0, -1.0);
        for _ in 0..20 {
            game.update(&back, &mut audio, DT);
        }
        for _ in 0..5 {
            game.update(&idle, &mut audio, DT);
        }
        game.render(&mut renderer);
        save(&mut renderer, &out_dir, "player_back");
    }

    // Chão vs castelo com a câmera orbitando junto à muralha.
    {
        let mut game = castle64::Castle64Game::new(Vec2::new(320.0, 240.0));
        for _ in 0..30 {
            game.update(&idle, &mut audio, DT);
        }
        let mut fwd = InputState::default();
        fwd.stick = Vec2::new(-0.3, 1.0);
        for _ in 0..110 {
            game.update(&fwd, &mut audio, DT);
        }
        let mut orbit = InputState::default();
        orbit.set(Button::R, true);
        for _ in 0..55 {
            game.update(&orbit, &mut audio, DT);
        }
        game.render(&mut renderer);
        save(&mut renderer, &out_dir, "castle_orbit");
    }
}
