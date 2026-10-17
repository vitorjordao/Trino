//! Evidence for ambient occlusion (ADR 0014) across every example project
//! and authored scene on the PC, plus the consoles' baked tier. Not a gate:
//!
//! ```sh
//! cargo test -p xtask --test ao_shots -- --ignored
//! cargo test -p xtask --test ao_shots_platformer -- --ignored
//! cargo test -p xtask --test ao_shots_hello -- --ignored
//! ```
//!
//! Writes `target/evidence/ao/`:
//! - `<game>.png`: each example game on the native profile, three panels —
//!   AO off, AO on (castle64 sets its own; the others are forced on with
//!   the defaults, `AoView::On`), the occlusion alone (`AoView::Mask`);
//! - `scene_<name>.png`: each `scenes/*.scene.ron` the same way, with the
//!   scene's AO or the defaults;
//! - `baked_<profile>.png`: the self-test panel without and with AO baked
//!   into its vertex colors, on the native, N64 and 3DS profiles — the
//!   consoles' tier, identical everywhere.

mod ao_evidence;

use ao_evidence::{NullAudio, assets, out_dir, renderer, row, three};
use trino_core::{
    AmbientOcclusion, Color, Game, InputState, Material, ModelParams, Renderer, Shading,
    Transform3, Vec2, Vec3,
};
use trino_platform_pc::{AoView, SimProfile};

#[test]
#[ignore = "evidence: writes target/evidence/ao (needs a GPU)"]
fn ambient_occlusion_evidence() {
    let root = ao_evidence::root();
    let assets = assets();
    let out = out_dir();
    let idle = InputState::default();
    let mut audio = NullAudio;

    let mut r = renderer(SimProfile::Native, &assets);
    let size = r.internal_size();
    let screen = Vec2::new(size.0 as f32, size.1 as f32);

    // castle64: hub, a stage, the throne room. It sets its own AO.
    for (level, name) in [
        (0, "castle64_hub"),
        (1, "castle64_stage"),
        (4, "castle64_throne"),
    ] {
        let mut game = castle64::Castle64Game::new(screen);
        game.goto_level(level);
        for _ in 0..30 {
            game.update(&idle, &mut audio, 1.0 / 60.0);
        }
        let panels = three(&mut r, AoView::Game, |r| game.render(r));
        row(&out.join(format!("{name}.png")), size, &panels);
    }
    // The cost on this GPU: castle64's hub at 1080p and 4K, the whole
    // frame (T&L, upload, GPU) with its AO (Medium, 16 samples) and off.
    for (w, h) in [(640u32, 360u32), (1280, 720), (1920, 1080), (3840, 2160)] {
        let mut r = renderer(SimProfile::Native, &assets);
        r.set_internal_size(w, h);
        let mut game = castle64::Castle64Game::new(Vec2::new(w as f32, h as f32));
        for _ in 0..30 {
            game.update(&idle, &mut audio, 1.0 / 60.0);
        }
        // Rounds alternate off and on (clocks and other load drift); the
        // fastest round of each counts.
        let time = |r: &mut trino_platform_pc::PcRenderer,
                    view: AoView,
                    game: &mut castle64::Castle64Game| {
            r.set_ao_view(view);
            for _ in 0..5 {
                game.render(r);
            }
            r.wait_gpu();
            let t0 = std::time::Instant::now();
            for _ in 0..20 {
                game.render(r);
                r.wait_gpu();
            }
            t0.elapsed().as_secs_f64() * 1000.0 / 20.0
        };
        let (mut off, mut on) = (f64::MAX, f64::MAX);
        for _ in 0..6 {
            off = off.min(time(&mut r, AoView::Off, &mut game));
            on = on.min(time(&mut r, AoView::Game, &mut game));
        }
        eprintln!(
            "PERF castle64 hub {w}x{h} ({}): AO off {off:.2} ms/frame, on {on:.2} ms/frame",
            r.adapter_info()
        );
    }

    // The showcase (its exported scene sets no AO: forced on).
    let mut game = showcase::ShowcaseGame::with_screen(screen);
    for _ in 0..30 {
        game.update(&idle, &mut audio, 1.0 / 60.0);
    }
    let panels = three(&mut r, AoView::On, |r| game.render(r));
    row(&out.join("showcase.png"), size, &panels);
    // Authored scenes, as the editor and `TRINO_SCENE` draw them.
    let mut lib = trino_scene::Library::load(&root);
    lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
    for entry in std::fs::read_dir(root.join("scenes")).unwrap() {
        let path = entry.unwrap().path();
        let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(name) = file.strip_suffix(".scene.ron") else {
            continue;
        };
        let scene = trino_scene::Scene::load(&path).unwrap();
        let env = &scene.environment;
        let ao = env.core_ao().unwrap_or(AmbientOcclusion::DEFAULT);
        let resolved = trino_scene::resolve(&scene, &lib, 0.0);
        let camera = env.core_camera();
        let panels = three(&mut r, AoView::On, |r| {
            let c = env.clear;
            r.begin_frame(Color::rgb(c[0], c[1], c[2]));
            r.set_camera(&camera);
            r.set_light(&env.core_light());
            r.set_ambient_occlusion(Some(&ao));
            for item in &resolved.items {
                if let Some(id) = item.model_from(camera.eye) {
                    r.draw_model_matrix(id, &item.matrix, Material::VertexLit, &item.params);
                }
            }
            // 2D on top, as the Game view draws it.
            for item in &resolved.items {
                if let Some(s) = &item.sprite {
                    let t = s.tint;
                    r.draw_sprite(
                        trino_core::SpriteId(s.id.0),
                        Vec2::new(s.pos[0], s.pos[1]),
                        &trino_core::SpriteParams {
                            scale: Vec2::new(s.scale[0], s.scale[1]),
                            rotation: s.rotation,
                            tint: Color::rgba(t[0], t[1], t[2], t[3]),
                            flip_x: s.flip_x,
                            flip_y: s.flip_y,
                            ..trino_core::SpriteParams::DEFAULT
                        },
                    );
                }
            }
            r.end_frame();
        });
        row(&out.join(format!("scene_{name}.png")), size, &panels);
    }

    // The consoles' tier: AO baked into vertex colors, the same on every
    // profile (the self-test's panel, lit this time).
    let raw = trino_core::ModelId::from_path("models/selftest_ao_raw");
    let ao = trino_core::ModelId::from_path("models/selftest_ao");
    for (profile, name) in [
        (SimProfile::Native, "native"),
        (SimProfile::N64, "n64"),
        (SimProfile::N3ds, "3ds"),
    ] {
        let mut r = renderer(profile, &assets);
        let size = r.internal_size();
        let camera = trino_core::Camera3 {
            eye: Vec3::new(0.35, 0.25, -1.2),
            target: Vec3::new(0.0, 0.0, -0.1),
            fov_y: 60.0 * core::f32::consts::PI / 180.0,
        };
        let panels: Vec<Vec<u8>> = [raw, ao]
            .into_iter()
            .map(|model| {
                r.begin_frame(Color::rgb(30, 34, 48));
                r.set_camera(&camera);
                r.set_light(&trino_core::DEFAULT_LIGHT);
                r.draw_model(
                    model,
                    &Transform3::default(),
                    Material::VertexLit,
                    &ModelParams {
                        shading: Shading::Gouraud,
                        double_sided: true,
                        ..ModelParams::DEFAULT
                    },
                );
                r.end_frame();
                r.read_offscreen()
            })
            .collect();
        row(&out.join(format!("baked_{name}.png")), size, &panels);
    }
}
