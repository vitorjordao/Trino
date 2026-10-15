//! Evidence for local lights in a real game: castle64's throne room, whose
//! torches are local lights, rendered on each light tier — per pixel
//! (native profile), per vertex plus floor pools (3DS and N64 profiles) —
//! and with the lights off, for side-by-side review. Not a gate:
//!
//! ```sh
//! cargo test -p xtask --test lights_shots -- --ignored
//! ```
//!
//! Writes `target/evidence/lights/throne_<profile>[_off].png`.

use std::path::PathBuf;

use trino_core::{Game, InputState, Renderer, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// Forwards everything but the local lights (the "off" shots).
struct NoLights<'a>(&'a mut PcRenderer);

impl Renderer for NoLights<'_> {
    fn caps(&self) -> &trino_core::Caps {
        self.0.caps()
    }
    fn begin_frame(&mut self, c: trino_core::Color) {
        self.0.begin_frame(c)
    }
    fn draw_sprite(&mut self, s: trino_core::SpriteId, p: Vec2, sp: &trino_core::SpriteParams) {
        self.0.draw_sprite(s, p, sp)
    }
    fn set_camera(&mut self, c: &trino_core::Camera3) {
        self.0.set_camera(c)
    }
    fn set_light(&mut self, l: &trino_core::Light) {
        self.0.set_light(l)
    }
    fn draw_model(
        &mut self,
        m: trino_core::ModelId,
        t: &trino_core::Transform3,
        mat: trino_core::Material,
        p: &trino_core::ModelParams,
    ) {
        self.0.draw_model(m, t, mat, p)
    }
    fn draw_billboard(
        &mut self,
        s: trino_core::SpriteId,
        p: trino_core::Vec3,
        b: &trino_core::BillboardParams,
    ) {
        self.0.draw_billboard(s, p, b)
    }
    fn fill_rect(&mut self, r: trino_core::Rect, c: trino_core::Color) {
        self.0.fill_rect(r, c)
    }
    fn draw_text(&mut self, f: trino_core::FontId, p: Vec2, t: &str, tp: &trino_core::TextParams) {
        self.0.draw_text(f, p, t, tp)
    }
    fn measure_text(&self, f: trino_core::FontId, t: &str, s: f32) -> Vec2 {
        self.0.measure_text(f, t, s)
    }
    fn end_frame(&mut self) {
        self.0.end_frame()
    }
}

fn save(renderer: &PcRenderer, path: &std::path::Path) {
    let (w, h) = renderer.internal_size();
    let rgba = renderer.read_offscreen();
    let file = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&rgba).unwrap();
    eprintln!("evidence: {}", path.display());
}

#[test]
#[ignore = "evidence: castle64's torches on every light tier (needs a GPU)"]
fn throne_room_on_every_light_tier() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = root.join("target/assets/pc");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let out = root.join("target/evidence/lights");
    std::fs::create_dir_all(&out).unwrap();
    for (profile, name) in [
        (SimProfile::Native, "native"),
        (SimProfile::N3ds, "n3ds"),
        (SimProfile::N64, "n64"),
    ] {
        let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
        let mut r = pollster::block_on(PcRenderer::new_headless(profile)).expect("a GPU adapter");
        for s in assets.sprites {
            r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
        }
        for m in assets.models {
            r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
        }
        for f in assets.fonts {
            r.upload_font(trino_core::FontId(f.id), f.table);
        }
        let (w, h) = r.internal_size();
        let mut game = castle64::Castle64Game::new(Vec2::new(w as f32, h as f32));
        game.goto_level(4);
        let mut audio = NullAudio;
        let idle = InputState::default();
        for _ in 0..90 {
            game.update(&idle, &mut audio, 1.0 / 60.0);
        }
        game.render(&mut r);
        save(&r, &out.join(format!("throne_{name}.png")));
        game.render(&mut NoLights(&mut r));
        save(&r, &out.join(format!("throne_{name}_off.png")));
    }
}
