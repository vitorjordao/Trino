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
use solid_eleven::game::app::{After, Scene};
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

/// Every menu screen, on each profile (`menu_<screen>_<profile>.png`).
#[test]
#[ignore = "local tool: screenshots (needs a GPU)"]
fn solid_eleven_menu_shots() {
    let dir = root().join("target/solid_eleven_shots");
    for (profile, tag, size) in [
        (SimProfile::N64, "n64", None),
        (SimProfile::N3ds, "3ds", None),
        (SimProfile::Native, "pc", Some((960u32, 540u32))),
        (SimProfile::Native, "pc360", Some((640u32, 360u32))),
        (SimProfile::Native, "pc720", Some((1280u32, 720u32))),
    ] {
        let mut r = renderer(profile, size);
        if profile == SimProfile::N64 {
            r.set_n64_look(true);
        }
        let (w, h) = r.internal_size();
        let mut g = SolidElevenGame::new(Vec2::new(w as f32, h as f32));
        let mut a = NullAudio;
        let idle = InputState::default();
        for _ in 0..30 {
            g.update(&idle, &mut a, 1.0 / 60.0);
        }
        // A career with a played round, a bought player and an edited one.
        g.world = solid_eleven::sim::league::Career::new(0, 7);
        g.career_on = true;
        g.world.finish_round(Some((2, 1)));
        let cheap = (0..solid_eleven::sim::team::PLAYERS)
            .filter(|&i| g.world.club_of[i] != 0)
            .min_by_key(|&i| g.world.buy_price(i))
            .unwrap();
        g.world.buy(cheap).unwrap();
        let scenes: [(&str, Scene); 11] = [
            ("title", Scene::Title),
            ("quick", Scene::Quick),
            ("pick", Scene::PickClub { then: After::Club }),
            ("club", Scene::Club),
            ("table", Scene::Table),
            ("teams", Scene::Teams),
            ("edit", Scene::Edit),
            ("market", Scene::Market),
            ("settings", Scene::Settings),
            ("results", Scene::Results),
            ("confirm", Scene::Confirm),
        ];
        for (name, scene) in scenes {
            g.scene = scene;
            if scene == Scene::Edit {
                let (list, _) = g.squad_list(0);
                let i = list[0] as usize;
                g.edit = Some((i, g.world.players[i]));
            }
            if scene == Scene::Results && g.report.is_none() {
                g.start_exhibition(0, 1, 3, [false, false], Sky::Day, 20);
                for _ in 0..60 * 50 {
                    g.update(&idle, &mut a, 1.0 / 60.0);
                    if g.scene == Scene::Results {
                        break;
                    }
                }
                g.scene = Scene::Results;
            }
            g.render(&mut r);
            save(&r, &dir, &format!("menu_{name}_{tag}"));
            if profile == SimProfile::N3ds {
                save_bottom(&r, &dir, &format!("menu_{name}_3ds_bottom"));
            }
        }
        // The match's touch screen.
        g.start_exhibition(0, 1, 5, [true, false], Sky::Day, 180);
        for _ in 0..600 {
            g.update(&idle, &mut a, 1.0 / 60.0);
        }
        g.render(&mut r);
        if profile == SimProfile::N3ds {
            save_bottom(&r, &dir, "match_3ds_bottom");
        }
        g.scene = Scene::Pause;
        g.render(&mut r);
        save(&r, &dir, &format!("menu_pause_{tag}"));
    }
}

fn save_bottom(r: &PcRenderer, dir: &Path, name: &str) {
    let rgba = r.read_second_screen();
    if rgba.is_empty() {
        return;
    }
    let out = dir.join(format!("{name}.png"));
    let file = std::fs::File::create(&out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 320, 240);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgba).unwrap();
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
        for (scene, ticks, sky, rain) in [
            ("kickoff", 0u32, Sky::Day, false),
            ("play", 60 * 40, Sky::Day, false),
            ("night", 60 * 70, Sky::Night, false),
            ("rain", 60 * 25, Sky::Dusk, true),
        ] {
            let mut g = SolidElevenGame::exhibition(
                Vec2::new(w as f32, h as f32),
                0,
                1,
                7,
                [false, false],
                sky,
            );
            if let Some(s) = &mut g.session {
                s.view.rain = rain;
            }
            let mut a = NullAudio;
            for _ in 0..ticks.max(1) {
                g.update(&InputState::default(), &mut a, 1.0 / 60.0);
            }
            // The PC renderer double-buffers nothing: one frame is enough.
            g.render(&mut r);
            save(&r, &dir, &format!("{scene}_{tag}"));
            if scene == "play" {
                // A close-up on the ball carrier (or the ball): the models.
                let m = &g.session.as_ref().unwrap().m;
                let at = match m.owner {
                    Some(o) => m.players[o as usize].pos,
                    None => m.ball.pos.ground(),
                };
                let cam = &mut g.session.as_mut().unwrap().view.cam;
                cam.focus = trino_core::Vec3::new(at.x.to_f32(), 0.9, at.y.to_f32());
                cam.height = 2.2;
                cam.back = 6.5;
                g.render(&mut r);
                save(&r, &dir, &format!("closeup_{tag}"));
            }
        }
    }
}
