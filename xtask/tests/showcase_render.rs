//! The two ways a scene reaches the screen must agree pixel for pixel:
//!
//! - **runtime**: `trino_scene::resolve` over the RON scene + prefabs +
//!   materials (editor viewport, `TRINO_SCENE` player);
//! - **codegen**: the `no_std` `SceneNode` data exported for consoles
//!   (`examples/showcase`).
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
    let lib = trino_scene::Library::load(&root);
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
