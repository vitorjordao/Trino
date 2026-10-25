//! Evidence for issue #66: the physics playground (`scenes/physics_playground
//! .scene.ron`) played by a script on the engine's character controller —
//! up the 30° ramp, blocked by the 50° one and sliding off it, up the
//! stairs (0.3 m step height), jumping up through the one-way platform, and
//! over the dome's triangle mesh — rendered by the PC renderer at 640×360.
//!
//! ```sh
//! cargo test -p xtask --test physics_gif -- --ignored --nocapture
//! ```
//!
//! Writes `target/evidence/physics/controller.gif` and a PNG per segment.

use std::path::PathBuf;

use trino_core::{BakedScene, Button, Game, InputState, Vec2, Vec3};
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

/// One scripted run: where the hero's feet start, how many frames, and the
/// input on each frame.
struct Segment {
    name: &'static str,
    feet: Vec3,
    frames: u32,
    input: fn(u32) -> InputState,
}

fn walk(dx: f32, dy: f32) -> InputState {
    let mut i = InputState::default();
    i.stick = Vec2::new(dx, dy);
    i
}

const SEGMENTS: &[Segment] = &[
    Segment {
        name: "ramp-30",
        feet: Vec3::new(-7.0, 0.0, -1.5),
        frames: 150,
        input: |_| walk(0.0, 1.0),
    },
    Segment {
        name: "ramp-50",
        feet: Vec3::new(-1.0, 0.0, -1.5),
        frames: 70,
        input: |_| walk(0.0, 1.0),
    },
    Segment {
        name: "slide-50",
        feet: Vec3::new(-1.0, 3.2, -5.6),
        frames: 70,
        input: |_| walk(0.0, 0.0),
    },
    Segment {
        name: "stairs",
        feet: Vec3::new(5.0, 0.0, 0.5),
        frames: 130,
        input: |_| walk(0.0, 1.0),
    },
    Segment {
        name: "one-way",
        feet: Vec3::new(9.5, 0.0, 3.0),
        frames: 120,
        input: |f| {
            let mut i = if f < 55 {
                walk(0.0, 0.8)
            } else {
                walk(0.0, 0.0)
            };
            // Jump under the platform: up through it, onto its top.
            i.set(Button::A, (40..44).contains(&f));
            i
        },
    },
    Segment {
        name: "dome",
        feet: Vec3::new(-6.0, 0.0, 12.0),
        frames: 170,
        input: |_| walk(0.0, 1.0),
    },
];

#[test]
#[ignore = "evidence: writes target/evidence/physics/controller.gif (GPU + assets)"]
fn record_the_controller_on_the_physics_playground() {
    let root = repo_root();
    let baked = root.join("target/assets/pc");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut renderer =
        pollster::block_on(PcRenderer::new_headless(SimProfile::N64)).expect("needs a GPU adapter");
    for s in &assets.sprites {
        renderer.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in &assets.models {
        renderer.upload_mesh(trino_core::ModelId(m.id), m.tmdl.clone());
    }
    for f in &assets.fonts {
        renderer.upload_font(trino_core::FontId(f.id), f.table.clone());
    }
    let rock = assets
        .blobs
        .iter()
        .find(|b| b.logical == "collision/rock")
        .expect("collision/rock baked");
    let rock: &'static [u8] = Box::leak(rock.bytes.clone().into_boxed_slice());

    let scene =
        trino_scene::Scene::load(&root.join("scenes/physics_playground.scene.ron")).unwrap();
    let mut lib = trino_scene::Library::load(&root);
    lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
    let (blob, warnings) = trino_scene::bake::to_tscn_for(&scene, &lib, trino_scene::Target::Pc);
    assert!(warnings.is_empty(), "{warnings:?}");
    let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
    let baked_scene = BakedScene::parse(blob).unwrap();
    let (w, h) = renderer.internal_size();
    let mut game = Box::new(showcase::ShowcaseGame::from_baked(
        &baked_scene,
        Vec2::new(w as f32, h as f32),
    ));
    assert_eq!(
        game.attach_mesh(trino_core::asset_id("collision/rock"), rock),
        1
    );

    let out = root.join("target/evidence/physics");
    std::fs::create_dir_all(&out).unwrap();
    let file = std::fs::File::create(out.join("controller.gif")).unwrap();
    let mut gif = gif::Encoder::new(file, w as u16, h as u16, &[]).unwrap();
    gif.set_repeat(gif::Repeat::Infinite).unwrap();
    let mut audio = NullAudio;
    const DT: f32 = 1.0 / 60.0;
    for seg in SEGMENTS {
        game.teleport_hero(seg.feet);
        // Settle the camera on the hero first.
        for _ in 0..20 {
            game.update(&InputState::default(), &mut audio, DT);
        }
        let mut top: f32 = f32::MIN;
        let mut slid = false;
        let mut stepped = false;
        for f in 0..seg.frames {
            game.update(&(seg.input)(f), &mut audio, DT);
            let (ctl, rep) = game.controller_debug().expect("the hero's controller");
            top = top.max(ctl.pos.y - ctl.cfg.half.y);
            slid |= rep.sliding;
            stepped |= rep.stepped;
            if f % 3 == 0 {
                game.render(&mut renderer);
                let mut rgba = renderer.read_offscreen();
                if f == seg.frames / 2 / 3 * 3 {
                    write_png(&out.join(format!("{}.png", seg.name)), w, h, &rgba);
                }
                let mut frame = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut rgba, 10);
                frame.delay = 5;
                gif.write_frame(&frame).unwrap();
            }
        }
        let (ctl, _) = game.controller_debug().unwrap();
        let feet = Vec3::new(ctl.pos.x, ctl.pos.y - ctl.cfg.half.y, ctl.pos.z);
        eprintln!(
            "{:9} highest feet {top:.2} m, ends at ({:.2}, {:.2}, {:.2}){}{}",
            seg.name,
            feet.x,
            feet.y,
            feet.z,
            if slid { ", slid" } else { "" },
            if stepped { ", stepped" } else { "" }
        );
        match seg.name {
            "ramp-30" => assert!(top > 1.70, "climbed the 30° ramp: {top}"),
            "ramp-50" => assert!(top < 0.8, "the 50° ramp is a wall: {top}"),
            "slide-50" => assert!(slid && feet.y < 0.05, "slid off the 50° ramp"),
            "stairs" => assert!(stepped && top > 1.2, "climbed the stairs: {top}"),
            "one-way" => assert!(
                (feet.y - 1.3).abs() < 0.02,
                "stands on the one-way: {feet:?}"
            ),
            "dome" => assert!(
                top > 0.6 && feet.z < 3.0,
                "walked over the dome: {top} {feet:?}"
            ),
            _ => {}
        }
    }
    drop(gif);
    eprintln!("wrote {}", out.join("controller.gif").display());
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    let f = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}
