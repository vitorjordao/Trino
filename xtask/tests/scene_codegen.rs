//! Editor scenes exported to game code must stay in sync with their source:
//! `examples/showcase/src/showcase_gen.rs` is generated from
//! `scenes/showcase.scene.ron` (+ prefabs + materials). Fix a failure with
//!
//! ```sh
//! cargo xtask scene export scenes/showcase.scene.ron examples/showcase/src/showcase_gen.rs
//! cargo xtask scene export --platform n64 scenes/showcase.scene.ron examples/showcase/src/showcase_gen_n64.rs
//! ```

use std::path::PathBuf;

#[test]
fn showcase_codegen_matches_the_scene() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let scene = trino_scene::Scene::load(&root.join("scenes/showcase.scene.ron")).unwrap();
    let mut lib = trino_scene::Library::load(&root);
    lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
    assert!(lib.errors.is_empty(), "{:?}", lib.errors);
    let (code, warnings) = trino_scene::codegen::to_rust(&scene, &lib);
    assert!(warnings.is_empty(), "scene has issues: {warnings:?}");
    let committed =
        std::fs::read_to_string(root.join("examples/showcase/src/showcase_gen.rs")).unwrap();
    assert!(
        committed == code,
        "examples/showcase/src/showcase_gen.rs is stale — regenerate it with \
         `cargo xtask scene export scenes/showcase.scene.ron examples/showcase/src/showcase_gen.rs`"
    );
}

/// The N64's export (`showcase_gen_n64.rs`, the scene with its N64
/// variants) stays in sync too: the N64 build and the host's N64
/// predictions play it.
#[test]
fn showcase_n64_codegen_matches_the_scene() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let scene = trino_scene::Scene::load(&root.join("scenes/showcase.scene.ron")).unwrap();
    let mut lib = trino_scene::Library::load(&root);
    lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
    let (code, warnings) =
        trino_scene::codegen::to_rust_for(&scene, &lib, trino_scene::Target::N64);
    assert!(warnings.is_empty(), "scene has issues: {warnings:?}");
    let committed =
        std::fs::read_to_string(root.join("examples/showcase/src/showcase_gen_n64.rs")).unwrap();
    assert!(
        committed == code,
        "examples/showcase/src/showcase_gen_n64.rs is stale — regenerate it with `cargo xtask \
         scene export --platform n64 scenes/showcase.scene.ron \
         examples/showcase/src/showcase_gen_n64.rs`"
    );
}
