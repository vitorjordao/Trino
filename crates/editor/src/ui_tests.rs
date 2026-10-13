//! Headless panel flows with egui_kittest: the real `App` (wgpu render
//! state included) driven through its UI by accessibility labels and
//! keyboard shortcuts — what a user clicks, checked against the document.
//!
//! GPU-gated like the renderer goldens (skips without an adapter unless
//! `TRINO_REQUIRE_GPU=1`). The editor opens a temp copy of the showcase
//! scene, so nothing in the repo is written.

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use crate::app::App;

/// The editor resolves assets relative to the working directory: run from
/// the repo root. One test owns the process cwd and env, so it is the only
/// one in this module.
#[test]
fn hierarchy_select_palette_duplicate_and_undo() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("flow.scene.ron");
    std::fs::copy(repo.join("scenes/showcase.scene.ron"), &scene).unwrap();
    std::env::set_current_dir(&repo).unwrap();
    // SAFETY: single test in this binary touching the environment, before
    // any editor thread starts.
    unsafe { std::env::set_var("TRINO_EDITOR_SCENE", &scene) };

    let built = std::panic::catch_unwind(|| {
        Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .wgpu()
            .build_eframe(|cc| App::new(cc))
    });
    let mut h = match built {
        Ok(h) => h,
        Err(e) => {
            assert!(
                std::env::var("TRINO_REQUIRE_GPU").is_err(),
                "TRINO_REQUIRE_GPU set but the editor could not start"
            );
            eprintln!("skipping editor UI test (no GPU adapter): {e:?}");
            return;
        }
    };
    h.run_steps(4);
    let roots = h.state().doc.scene.entities.len();

    // Click an entity in the Hierarchy: it becomes the selection.
    h.get_by_label("pond").click();
    h.run_steps(2);
    let sel = h.state().doc.selected.clone().expect("pond selected");
    assert_eq!(h.state().doc.scene.get(&sel).unwrap().name, "pond");

    // Ctrl+K, type, Enter: the palette runs Duplicate on the selection.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
    h.run_steps(2);
    assert!(h.state().palette.is_some(), "palette opened");
    // The palette's field has keyboard focus: type into it.
    h.event(egui::Event::Text("Duplicate".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.state().palette.is_none(), "palette closed after running");
    assert_eq!(h.state().doc.scene.entities.len(), roots + 1, "duplicated");

    // Ctrl+Z restores the document.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(3);
    assert_eq!(h.state().doc.scene.entities.len(), roots, "undone");
    // The scene on disk was never saved by these steps.
    let disk = std::fs::read_to_string(&scene).unwrap();
    let original = std::fs::read_to_string(repo.join("scenes/showcase.scene.ron")).unwrap();
    assert_eq!(disk, original);
}
