//! Headless panel flows with egui_kittest: the real `App` (wgpu render
//! state included) driven through its UI by accessibility labels and
//! keyboard shortcuts — what a user clicks, checked against the document.
//!
//! The editor resolves everything relative to the working directory and
//! reads its hooks from the environment, so this test changes both. It
//! is `#[ignore]` and runs on its own, single-threaded:
//!
//! ```text
//! cargo test -p trino-editor ui_tests -- --ignored --test-threads=1
//! ```
//!
//! (CI does, under xvfb with `TRINO_REQUIRE_GPU=1`.) GPU-gated like the
//! renderer goldens. The editor opens a temp copy of the showcase scene,
//! so nothing in the repo is written.

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use crate::app::App;

/// Runs from the repo root (the editor's asset root is the cwd).
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn hierarchy_select_palette_duplicate_and_undo() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp = tempfile::tempdir().unwrap();
    let scene = tmp.path().join("flow.scene.ron");
    std::fs::copy(repo.join("scenes/showcase.scene.ron"), &scene).unwrap();
    std::env::set_current_dir(&repo).unwrap();
    // SAFETY: this test only runs with --ignored --test-threads=1 (see the
    // module docs), so no other thread reads or writes the environment.
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

/// Copy the engine's authored content into `dst` (a throwaway project root).
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).unwrap();
        }
    }
}

fn find(scene: &trino_scene::Scene, name: &str) -> Vec<usize> {
    let mut found = None;
    scene.walk(|p, e| {
        if e.name == name && found.is_none() {
            found = Some(p.to_vec());
        }
    });
    found.unwrap_or_else(|| panic!("no entity `{name}`"))
}

/// Every editor command the panels and menus issue, run on a temp copy of
/// the engine's content (so file-writing commands are safe): entity edits
/// with undo, prefabs, materials, fake objects, scenes, export, the asset
/// pages of the Inspector and each dock tab.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn commands_edit_save_and_export_in_a_temp_project() {
    use crate::app::{AssetSel, Cmd, Tab};
    use crate::ops::AlignTo;
    use trino_scene::Entity;

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for dir in ["assets", "scenes", "prefabs"] {
        copy_tree(&repo.join(dir), &root.join(dir));
    }
    copy_tree(
        &repo.join("examples/platformer/src"),
        &root.join("examples/platformer/src"),
    );
    std::env::set_current_dir(root).unwrap();
    // SAFETY: --ignored --test-threads=1 only (module docs).
    unsafe { std::env::set_var("TRINO_EDITOR_SCENE", "scenes/showcase.scene.ron") };

    let built = std::panic::catch_unwind(|| {
        Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .wgpu()
            .build_eframe(|cc| App::new(cc))
    });
    let mut h = match built {
        Ok(h) => h,
        Err(e) => {
            std::env::set_current_dir(&repo).unwrap();
            assert!(
                std::env::var("TRINO_REQUIRE_GPU").is_err(),
                "TRINO_REQUIRE_GPU set but the editor could not start"
            );
            eprintln!("skipping editor UI test (no GPU adapter): {e:?}");
            return;
        }
    };
    let cmd = |h: &mut Harness<'_, App>, c: Cmd| {
        h.state_mut().cmds.push(c);
        h.run_steps(2);
    };
    h.run_steps(4);
    assert_eq!(h.state().log.counts().1, 0, "the project loads cleanly");
    assert!(h.state().project.models.contains_key("models/crate"));
    let roots = h.state().doc.scene.entities.len();

    // Create, duplicate, delete, undo, redo.
    cmd(
        &mut h,
        Cmd::Create {
            parent: None,
            entity: Box::new(Entity::new("probe")),
        },
    );
    let probe = find(&h.state().doc.scene, "probe");
    assert_eq!(h.state().doc.selected.as_ref(), Some(&probe));
    cmd(&mut h, Cmd::Duplicate(probe.clone()));
    assert_eq!(h.state().doc.scene.entities.len(), roots + 2);
    let copy = h.state().doc.selected.clone().unwrap();
    assert_eq!(
        h.state().doc.scene.get(&copy).unwrap().transform.pos[0],
        1.0
    );
    cmd(&mut h, Cmd::Delete(copy));
    assert_eq!(h.state().doc.scene.entities.len(), roots + 1);
    cmd(&mut h, Cmd::Undo);
    assert_eq!(h.state().doc.scene.entities.len(), roots + 2);
    cmd(&mut h, Cmd::Redo);
    assert_eq!(h.state().doc.scene.entities.len(), roots + 1);
    assert!(h.state().doc.dirty());

    // Group operations: select all, align, array, duplicate, delete.
    let probe = find(&h.state().doc.scene, "probe");
    cmd(&mut h, Cmd::Select(Some(probe.clone())));
    cmd(
        &mut h,
        Cmd::ArrayCopies {
            count: 3,
            step: [2.0, 0.0, 0.0],
        },
    );
    let sel = h.state().doc.selection();
    assert_eq!(sel.len(), 4);
    // Spread the copies out far from the level, in the air, at uneven spots.
    for (i, (x, z)) in [(100.0, -3.0), (101.0, 5.0), (105.0, 1.0), (109.0, 2.0)]
        .into_iter()
        .enumerate()
    {
        let e = h.state_mut().doc.scene.get_mut(&sel[i]).unwrap();
        e.transform.pos = [x, 10.0, z];
    }
    let pos = |h: &Harness<'_, App>| -> Vec<[f32; 3]> {
        sel.iter()
            .map(|p| h.state().doc.scene.get(p).unwrap().transform.pos)
            .collect()
    };
    cmd(&mut h, Cmd::Align(2, AlignTo::Max));
    assert!(
        pos(&h).iter().all(|p| p[2] == 5.0),
        "aligned on max z: {:?}",
        pos(&h)
    );
    cmd(&mut h, Cmd::Distribute(0));
    let mut xs: Vec<f32> = pos(&h).iter().map(|p| p[0]).collect();
    xs.sort_by(f32::total_cmp);
    assert_eq!(
        xs,
        vec![100.0, 103.0, 106.0, 109.0],
        "even gaps between the ends"
    );
    cmd(&mut h, Cmd::DropToSurface);
    assert!(
        pos(&h).iter().all(|p| p[1] == 0.0),
        "nothing below: they land on the ground plane: {:?}",
        pos(&h)
    );
    cmd(&mut h, Cmd::DuplicateSelection);
    assert_eq!(h.state().doc.scene.entities.len(), roots + 8);
    cmd(&mut h, Cmd::DeleteSelection);
    assert_eq!(h.state().doc.scene.entities.len(), roots + 4);
    cmd(&mut h, Cmd::SelectAll);
    assert_eq!(h.state().doc.selection().len(), roots + 4);
    cmd(&mut h, Cmd::Select(None));
    // Align with a single entity is an error toast, not a change.
    let errors = h.state().log.counts().1;
    cmd(&mut h, Cmd::Align(0, AlignTo::Min));
    assert_eq!(h.state().log.counts().1, errors + 1);

    // Reparent under the pond, then copy as RON.
    let probe = find(&h.state().doc.scene, "probe");
    let pond = find(&h.state().doc.scene, "pond");
    cmd(
        &mut h,
        Cmd::Reparent {
            from: probe,
            to: Some(pond.clone()),
        },
    );
    let nested = find(&h.state().doc.scene, "probe");
    assert_eq!(nested.len(), 2, "now a child: {nested:?}");
    cmd(&mut h, Cmd::Copy(nested.clone()));
    assert!(h.state().log.last().unwrap().text.contains("copied probe"));

    // Prefabs: make one from the probe, unpack it, edit a library prefab.
    cmd(
        &mut h,
        Cmd::MakePrefab(nested.clone(), "probe-prefab".into()),
    );
    assert!(root.join("prefabs/probe-prefab.prefab.ron").exists());
    assert!(h.state().lib.prefabs.contains_key("prefabs/probe-prefab"));
    let inst = find(&h.state().doc.scene, "probe");
    assert!(h.state().doc.scene.get(&inst).unwrap().prefab.is_some());
    cmd(&mut h, Cmd::MakePrefab(inst.clone(), "probe-prefab".into()));
    assert!(
        h.state()
            .log
            .last()
            .unwrap()
            .text
            .contains("already exists"),
        "{}",
        h.state().log.last().unwrap().text
    );
    cmd(&mut h, Cmd::Unpack(inst.clone()));
    assert!(h.state().doc.scene.get(&inst).unwrap().prefab.is_none());
    cmd(&mut h, Cmd::EditPrefab("prefabs/tree".into()));
    assert!(h.state().doc.is_prefab());
    assert_eq!(h.state().doc.title(), "prefabs/tree");
    cmd(&mut h, Cmd::Delete(vec![0]));
    assert!(
        h.state()
            .log
            .last()
            .unwrap()
            .text
            .contains("root cannot be deleted"),
        "the prefab root is protected"
    );
    cmd(&mut h, Cmd::ExitPrefab);
    assert!(!h.state().doc.is_prefab());

    // Assets: new material, a fake object added to the scene, and the
    // Inspector page of every asset kind.
    cmd(
        &mut h,
        Cmd::NewMaterial("probe-mat".into(), trino_scene::MaterialDef::default()),
    );
    assert_eq!(
        h.state().asset_sel,
        Some(AssetSel::Material("materials/probe-mat".into()))
    );
    h.run_steps(4);
    assert!(
        root.join("assets/materials/probe-mat.material.ron")
            .exists()
    );
    cmd(
        &mut h,
        Cmd::CreateProxy(
            "probe-box".into(),
            trino_asset_pipeline::Primitive::default(),
            true,
        ),
    );
    assert!(
        std::fs::read_to_string(root.join("assets/manifest.toml"))
            .unwrap()
            .contains("[models.probe-box]")
    );
    let added = find(&h.state().doc.scene, "probe-box");
    assert_eq!(
        h.state()
            .doc
            .scene
            .get(&added)
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .path,
        "models/probe-box"
    );
    for sel in [
        AssetSel::Model("models/crate".into()),
        AssetSel::Material("materials/stone".into()),
        AssetSel::Prefab("prefabs/tree".into()),
        AssetSel::Sprite("sprites/brick".into()),
    ] {
        cmd(&mut h, Cmd::SelectAsset(sel.clone()));
        cmd(&mut h, Cmd::FocusTab(Tab::Inspector));
        assert_eq!(h.state().asset_sel, Some(sel));
        assert_eq!(h.state().doc.selected, None);
    }
    // Assets and Console share a dock node: focusing one hides the other.
    let line = h
        .state()
        .log
        .entries
        .iter()
        .find(|e| e.text.starts_with("library: "))
        .unwrap()
        .text
        .clone();
    cmd(&mut h, Cmd::FocusTab(Tab::Console));
    assert!(
        h.query_by_label(&line).is_some(),
        "the Console shows `{line}`"
    );
    cmd(&mut h, Cmd::FocusTab(Tab::Assets));
    assert!(
        h.query_by_label(&line).is_none(),
        "the Assets tab is in front"
    );
    for tab in [Tab::Level, Tab::Game, Tab::Scene] {
        cmd(&mut h, Cmd::FocusTab(tab));
    }

    // Save, export and scene tabs.
    cmd(&mut h, Cmd::Save);
    assert!(!h.state().doc.dirty());
    let saved = trino_scene::Scene::load(&root.join("scenes/showcase.scene.ron")).unwrap();
    assert_eq!(saved, h.state().doc.scene);
    let out = root.join("probe_gen.rs");
    cmd(&mut h, Cmd::Export(out.clone()));
    assert!(std::fs::read_to_string(&out).unwrap().contains("SceneNode"));
    cmd(&mut h, Cmd::NewScene("probe-level".into()));
    assert_eq!(h.state().doc.scene.name, "probe-level");
    assert!(root.join("scenes/probe-level.scene.ron").exists());
    cmd(
        &mut h,
        Cmd::OpenScene(std::path::PathBuf::from("scenes/interior.scene.ron")),
    );
    assert_eq!(h.state().open_docs.len(), 1);
    cmd(&mut h, Cmd::SwitchScene(0));
    assert_eq!(h.state().doc.scene.name, "probe-level");
    cmd(&mut h, Cmd::CloseScene(0));
    assert!(h.state().open_docs.is_empty());
    cmd(&mut h, Cmd::NewScene("Bad Name".into()));
    assert!(
        h.state()
            .log
            .last()
            .unwrap()
            .text
            .contains("not a valid scene name")
    );

    // Play here runs the scene inside the Game view, and stops.
    cmd(&mut h, Cmd::PlayHere);
    h.run_steps(6);
    let frames = h
        .state()
        .game_view
        .play
        .as_ref()
        .expect("a play session")
        .frames;
    assert!(frames > 0, "the session advanced");
    cmd(&mut h, Cmd::StopPlayHere);
    assert!(h.state().game_view.play.is_none());
    assert!(
        h.state()
            .log
            .last()
            .unwrap()
            .text
            .starts_with("play here stopped")
    );

    // Nothing above leaked into the engine's own content.
    std::env::set_current_dir(&repo).unwrap();
    // SAFETY: as above.
    unsafe { std::env::remove_var("TRINO_EDITOR_SCENE") };
    assert!(!repo.join("prefabs/probe-prefab.prefab.ron").exists());
    assert!(!repo.join("scenes/probe-level.scene.ron").exists());
}
