//! The two ways a scene reaches the screen must agree pixel for pixel:
//!
//! - **runtime**: `trino_scene::resolve` over the RON scene + prefabs +
//!   materials (editor viewport, `TRINO_SCENE` player);
//! - **codegen**: the `no_std` `SceneNode` data exported for consoles
//!   (`examples/showcase`);
//! - **baked**: the `TSCN` blob games load at runtime (`BakedScene`).
//!
//! Same renderer, same camera, t = 0. GPU-gated like the goldens.

use std::path::PathBuf;

use trino_core::scene::draw_nodes;
use trino_core::{Mat34, Material, Renderer};
use trino_platform_pc::{PcRenderer, SimProfile};

fn renderer(root: &std::path::Path) -> Option<PcRenderer> {
    let baked = root.join("target/assets/pc");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(SimProfile::N64)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return None;
        }
    };
    r.set_n64_look(false);
    for m in assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    Some(r)
}

#[test]
fn runtime_scene_and_exported_code_render_identically() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let Some(mut r) = renderer(&root) else {
        return;
    };
    let scene = trino_scene::Scene::load(&root.join("scenes/showcase.scene.ron")).unwrap();
    let mut lib = trino_scene::Library::load(&root);
    lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
    let env = &scene.environment;

    // Runtime path.
    let c = env.clear;
    r.begin_frame(trino_core::Color::rgb(c[0], c[1], c[2]));
    r.set_camera(&env.core_camera());
    r.set_light(&env.core_light());
    let resolved = trino_scene::resolve(&scene, &lib, 0.0);
    for item in &resolved.items {
        if let Some((id, _)) = &item.model {
            r.draw_model_matrix(*id, &item.matrix, Material::VertexLit, &item.params);
        }
    }
    r.end_frame();
    let runtime = r.read_offscreen();

    // Codegen path.
    r.begin_frame(showcase::scene_consts::CLEAR);
    r.set_camera(&showcase::scene_consts::CAMERA);
    r.set_light(&showcase::scene_consts::LIGHT);
    draw_nodes(&mut r, showcase::NODES, &Mat34::IDENTITY, 0.0);
    r.end_frame();
    let codegen = r.read_offscreen();

    // Baked-blob path (TSCN, what games load at runtime).
    let (blob, warnings) = trino_scene::bake::to_tscn(&scene, &lib);
    assert!(warnings.is_empty(), "{warnings:?}");
    let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
    let baked = trino_core::BakedScene::parse(blob).unwrap();
    r.begin_frame(baked.clear());
    r.set_camera(&baked.camera());
    r.set_light(&baked.light());
    let nodes: Vec<trino_core::SceneNode> = baked.nodes().collect();
    draw_nodes(&mut r, &nodes, &Mat34::IDENTITY, 0.0);
    r.end_frame();
    assert!(
        r.read_offscreen() == runtime,
        "baked TSCN renders differently from the runtime scene"
    );

    let differing = runtime
        .as_chunks::<4>()
        .0
        .iter()
        .zip(codegen.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels differ between runtime and codegen"
    );
    // And the scene is not empty: plenty of non-background pixels.
    let bg = [c[0], c[1], c[2]];
    let drawn = runtime
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[..3] != bg)
        .count();
    assert!(drawn > 10_000, "only {drawn} pixels drawn");
}

/// Screenshots of the showcase *game* mid-run (local visual check, docs):
/// `cargo test -p xtask --test showcase_render -- --ignored` writes
/// `target/showcase_shots/*.png`.
#[test]
#[ignore = "local utility: needs a GPU; writes target/showcase_shots"]
fn showcase_gameplay_shots() {
    use trino_core::{Audio, Game};
    struct NullAudio;
    impl Audio for NullAudio {
        fn play_sound(&mut self, _: trino_core::SoundId) {}
        fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let Some(mut r) = renderer(&root) else {
        return;
    };
    let baked = root.join("target/assets/pc");
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    for f in assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    for s in assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    r.set_n64_look(true);
    let out = root.join("target/showcase_shots");
    std::fs::create_dir_all(&out).unwrap();
    let mut game = showcase::ShowcaseGame::new();
    let mut frame_no = 0u32;
    let mut shots = 0;
    let mut coins = 0;
    let mut sparkle_at = None;
    showcase::run_demo(&mut game, |g, input| {
        g.update(input, &mut NullAudio, 1.0 / 60.0);
        frame_no += 1;
        // A shot every 4 s, plus one a few frames after each pickup (the
        // sparkle particles).
        if g.coins != coins {
            coins = g.coins;
            sparkle_at = Some(frame_no + 6);
        }
        if frame_no.is_multiple_of(240) || sparkle_at == Some(frame_no) {
            g.render(&mut r);
            let (w, h) = r.internal_size();
            let path = out.join(format!("run{shots:02}.png"));
            let file = std::fs::File::create(&path).unwrap();
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .unwrap()
                .write_image_data(&r.read_offscreen())
                .unwrap();
            eprintln!("screenshot: {}", path.display());
            shots += 1;
        }
    })
    .unwrap();
    // Final frame: everything collected, goal reached.
    game.render(&mut r);
}
