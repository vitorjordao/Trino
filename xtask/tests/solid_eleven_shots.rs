//! Screenshots of Solid Eleven, headless, for looking at changes (not a
//! gate; the goldens are `solid_eleven_render.rs`):
//!
//! ```sh
//! cargo test -p xtask --test solid_eleven_shots -- --ignored --nocapture
//! ```
//!
//! Writes `target/solid_eleven_shots/<scene>_<profile>.png`: the N64 look,
//! the 3DS profile and the PC's native profile at 960x540.

use std::path::{Path, PathBuf};

use solid_eleven::SolidElevenGame;
use solid_eleven::render::Sky;
use trino_core::{Game, InputState, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

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

pub fn renderer(profile: SimProfile, size: Option<(u32, u32)>) -> PcRenderer {
    let root = root();
    let baked = root.join("target/assets/solid-eleven/pc");
    trino_asset_pipeline::bake_all(
        &root.join("examples/solid-eleven/assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = pollster::block_on(PcRenderer::new_headless(profile)).expect("a GPU adapter");
    if let Some((w, h)) = size {
        r.set_internal_size(w, h);
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
    r
}

fn save(r: &PcRenderer, dir: &Path, name: &str) {
    let (w, h) = r.internal_size();
    let rgba = r.read_offscreen();
    std::fs::create_dir_all(dir).unwrap();
    let out = dir.join(format!("{name}.png"));
    let file = std::fs::File::create(&out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    eprintln!("screenshot: {}", out.display());
}

#[test]
#[ignore = "local tool: screenshots (needs a GPU)"]
fn solid_eleven_shots() {
    let dir = root().join("target/solid_eleven_shots");
    for (profile, tag, size) in [
        (SimProfile::N64, "n64", None),
        (SimProfile::N3ds, "3ds", None),
        (SimProfile::Native, "pc", Some((960u32, 540u32))),
    ] {
        let mut r = renderer(profile, size);
        if profile == SimProfile::N64 {
            // No strict mode: its 3D cost estimate prices every draw as CPU
            // T&L, but these draws run on the N64's RSP (measured in ares).
            r.set_n64_look(true);
        }
        let (w, h) = r.internal_size();
        for (scene, ticks, sky) in [
            ("kickoff", 0u32, Sky::Day),
            ("play", 60 * 40, Sky::Day),
            ("night", 60 * 70, Sky::Night),
        ] {
            let mut g = SolidElevenGame::exhibition(
                Vec2::new(w as f32, h as f32),
                0,
                1,
                7,
                [false, false],
                sky,
            );
            let mut a = NullAudio;
            for _ in 0..ticks.max(1) {
                g.update(&InputState::default(), &mut a, 1.0 / 60.0);
            }
            // The PC renderer double-buffers nothing: one frame is enough.
            g.render(&mut r);
            save(&r, &dir, &format!("{scene}_{tag}"));
            if scene == "play" {
                // A close-up on the ball carrier (or the ball): the models.
                let m = &g.session.m;
                let at = match m.owner {
                    Some(o) => m.players[o as usize].pos,
                    None => m.ball.pos.ground(),
                };
                let cam = &mut g.session.view.cam;
                cam.focus = trino_core::Vec3::new(at.x.to_f32(), 0.9, at.y.to_f32());
                cam.height = 2.2;
                cam.back = 6.5;
                g.render(&mut r);
                save(&r, &dir, &format!("closeup_{tag}"));
            }
        }
    }
}
