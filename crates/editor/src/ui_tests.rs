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

/// A throwaway project (the engine's content copied into a temp dir, the
/// cwd moved there) with the editor open on `scene` at `size`. `None` when
/// there is no GPU adapter (and `TRINO_REQUIRE_GPU` is unset).
fn temp_project_app(
    size: egui::Vec2,
) -> Option<(Harness<'static, App>, tempfile::TempDir, std::path::PathBuf)> {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    for dir in ["assets", "scenes", "prefabs"] {
        copy_tree(&repo.join(dir), &tmp.path().join(dir));
    }
    copy_tree(
        &repo.join("examples/platformer/src"),
        &tmp.path().join("examples/platformer/src"),
    );
    std::env::set_current_dir(tmp.path()).unwrap();
    // SAFETY: --ignored --test-threads=1 only (module docs).
    unsafe { std::env::set_var("TRINO_EDITOR_SCENE", "scenes/showcase.scene.ron") };
    let built = std::panic::catch_unwind(|| {
        Harness::builder()
            .with_size(size)
            .wgpu()
            .build_eframe(|cc| App::new(cc))
    });
    match built {
        Ok(mut h) => {
            h.run_steps(4);
            Some((h, tmp, repo))
        }
        Err(e) => {
            std::env::set_current_dir(&repo).unwrap();
            assert!(
                std::env::var("TRINO_REQUIRE_GPU").is_err(),
                "TRINO_REQUIRE_GPU set but the editor could not start"
            );
            eprintln!("skipping editor UI test (no GPU adapter): {e:?}");
            None
        }
    }
}

fn leave_temp_project(repo: &std::path::Path) {
    std::env::set_current_dir(repo).unwrap();
    // SAFETY: as in `temp_project_app`.
    unsafe { std::env::remove_var("TRINO_EDITOR_SCENE") };
}

/// Scroll the widget labeled `label` into view, then click it (panels
/// scroll: a click on a clipped widget would land on whatever covers it).
fn click(h: &mut Harness<'static, App>, label: &str) {
    let screen = h.ctx.content_rect();
    // An exact label, else the one widget whose label contains it (icon
    // glyphs prefix most button and header texts).
    let rect = |h: &Harness<'static, App>| match h.query_by_label(label) {
        Some(n) => n.rect(),
        None => h.get_by_label_contains(label).rect(),
    };
    for _ in 0..40 {
        let r = rect(h);
        let dy = if r.bottom() > screen.bottom() - 4.0 {
            -120.0
        } else if r.top() < screen.top() + 60.0 {
            120.0
        } else {
            break;
        };
        // Wheel over the widget's panel, inside the window.
        let at = egui::pos2(
            r.center().x,
            r.center().y.clamp(80.0, screen.bottom() - 40.0),
        );
        h.hover_at(at);
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        // Wheel scrolling is animated: let it settle before measuring again.
        h.run_steps(8);
    }
    h.run_steps(4);
    match h.query_by_label(label) {
        Some(n) => n.click(),
        None => h.get_by_label_contains(label).click(),
    }
    h.run_steps(3);
}

/// The Game view's item for `name`, as its profile resolves it.
fn game_model(app: &App, name: &str) -> Option<trino_core::ModelId> {
    app.resolved_game
        .items
        .iter()
        .find(|i| i.name == name)
        .and_then(|i| i.model.as_ref().map(|(id, _)| *id))
}

/// Platforms tab ↔ Game view ↔ the scene file, both ways: a variant picked
/// in the Inspector changes what the N64 Game view draws (not the Scene
/// view), saving writes it as text, and a variant an agent writes into the
/// file shows up in the editor.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn platform_variants_reach_the_game_view_and_the_file() {
    use crate::app::{Cmd, Tab};
    use trino_core::ModelId;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    let rock = find(&h.state().doc.scene, "rock-1");
    h.state_mut().cmds.push(Cmd::Select(Some(rock.clone())));
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
    h.run_steps(3);
    {
        let app = h.state_mut();
        app.game_view
            .set_profile(trino_platform_pc::SimProfile::N64, &app.project);
    }
    h.run_steps(3);
    let rock_id = ModelId(trino_core::asset_id("models/rock"));
    let crate_id = ModelId(trino_core::asset_id("models/crate"));
    assert_eq!(game_model(h.state(), "rock-1"), Some(rock_id));
    let tris_before = h.state().game_view.stats.tris;

    // Open the Platforms section and pick another model for the N64.
    h.get_by_label_contains("Platforms").click();
    h.run_steps(2);
    click(&mut h, "N64 model");
    // Popup items: through accessibility (the list scrolls on its own).
    h.get_by_label("models/crate").click_accesskit();
    h.run_steps(3);
    let e = h.state().doc.scene.get(&rock).unwrap().clone();
    assert_eq!(
        e.platforms.n64.as_ref().unwrap().model.as_deref(),
        Some("models/crate")
    );
    assert_eq!(game_model(h.state(), "rock-1"), Some(crate_id), "N64 view");
    let pc = h.state().resolved.items.iter().find(|i| i.name == "rock-1");
    assert_eq!(
        pc.unwrap().model.as_ref().unwrap().0,
        rock_id,
        "PC unchanged"
    );
    let (rock_tris, crate_tris) = {
        let m = &h.state().project.models;
        (m["models/rock"].tris, m["models/crate"].tris)
    };
    assert_ne!(rock_tris, crate_tris);
    assert_eq!(
        h.state().game_view.stats.tris + rock_tris,
        tris_before + crate_tris,
        "the Game view drew the crate instead of the rock"
    );

    // "Not on N64": gone from the N64 view only.
    click(&mut h, "Not on N64");
    assert_eq!(game_model(h.state(), "rock-1"), None);
    assert_eq!(h.state().game_view.stats.tris + rock_tris, tris_before);
    assert!(h.state().resolved.items.iter().any(|i| i.name == "rock-1"));

    // GUI → file: saving writes the variant as text an agent can read.
    h.state_mut().cmds.push(Cmd::Save);
    h.run_steps(3);
    let disk = std::fs::read_to_string("scenes/showcase.scene.ron").unwrap();
    let saved = trino_scene::Scene::from_ron(&disk).unwrap();
    let r = saved.get(&rock).unwrap();
    assert!(disk.contains("version: 3"));
    assert!(r.platforms.n64.as_ref().unwrap().disabled);
    assert_eq!(
        r.platforms.n64.as_ref().unwrap().model.as_deref(),
        Some("models/crate")
    );

    // File → GUI: an agent gives rock-2 a 3DS variant; the editor reloads.
    let mut edited = saved.clone();
    let rock2 = find(&edited, "rock-2");
    edited.get_mut(&rock2).unwrap().platforms.n3ds = Some(trino_scene::Variant {
        model: Some("models/pillar".into()),
        ..Default::default()
    });
    std::fs::write("scenes/showcase.scene.ron", edited.to_ron()).unwrap();
    let start = std::time::Instant::now();
    while h.state().doc.scene != edited && start.elapsed().as_secs() < 10 {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(h.state().doc.scene, edited, "the external edit reloaded");
    {
        let app = h.state_mut();
        app.game_view
            .set_profile(trino_platform_pc::SimProfile::N3ds, &app.project);
    }
    h.run_steps(3);
    assert_eq!(
        game_model(h.state(), "rock-2"),
        Some(ModelId(trino_core::asset_id("models/pillar")))
    );
    assert_eq!(
        game_model(h.state(), "rock-1"),
        Some(rock_id),
        "3DS: as on PC"
    );
    leave_temp_project(&repo);
}

/// The whole editor at its minimum window size and at the default one, with
/// an entity's Inspector open (Platforms section included) and with the
/// scene settings: no widget runs off the window or overlaps another.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn editor_layout_is_clean_at_minimum_and_default_sizes() {
    use crate::app::{Cmd, Tab};
    for size in [egui::vec2(960.0, 600.0), egui::vec2(1600.0, 940.0)] {
        let Some((mut h, _tmp, repo)) = temp_project_app(size) else {
            return;
        };
        let what = |s: &str| format!("{s} at {}x{}", size.x, size.y);
        crate::layout_audit::assert_clean(&h, size, true, &what("scene settings"));
        let rock = find(&h.state().doc.scene, "rock-1");
        h.state_mut().cmds.push(Cmd::Select(Some(rock)));
        h.run_steps(3);
        h.get_by_label_contains("Platforms").click();
        h.run_steps(3);
        crate::layout_audit::assert_clean(&h, size, true, &what("entity inspector"));
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
        h.run_steps(3);
        crate::layout_audit::assert_clean(&h, size, true, &what("Game view"));
        // A LOD chain with an impostor level, previewed.
        let boar = trino_scene::Entity::new("boar").with_model(trino_scene::ModelComponent {
            lod: vec![
                trino_scene::LodDef {
                    distance: 12.0,
                    impostor: Some("impostors/selftest-boar".into()),
                    platforms: trino_scene::LodPlatforms {
                        n64: Some(trino_scene::LodAt::Never),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                trino_scene::LodDef {
                    distance: 40.0,
                    platforms: trino_scene::LodPlatforms {
                        n64: Some(trino_scene::LodAt::At(25.0)),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ],
            ..trino_scene::ModelComponent::new("models/c64_boar")
        });
        h.state_mut().cmds.push(Cmd::Create {
            parent: None,
            entity: Box::new(boar),
        });
        h.run_steps(3);
        let sel = h.state().doc.selected.clone().unwrap();
        h.state_mut().lod_preview = Some((sel, 20.0));
        h.state_mut().inspect_focus = Some("lod".into());
        h.run_steps(4);
        crate::layout_audit::assert_clean(&h, size, true, &what("LOD panel"));
        // A model asset with its impostor section.
        h.state_mut().inspect_focus = Some("impostor".into());
        h.state_mut()
            .cmds
            .push(Cmd::SelectAsset(crate::app::AssetSel::Model(
                "models/c64_boar".into(),
            )));
        h.run_steps(4);
        crate::layout_audit::assert_clean(&h, size, true, &what("impostor section"));
        leave_temp_project(&repo);
    }
}

/// Impostors from the Inspector, both ways: "Create and bake" on a model
/// writes the same files `cargo xtask impostor bake` does, the library
/// picks the baked table up, an entity's LOD level draws it (the preview
/// shows the switch in the view), and settings an agent writes appear.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn impostors_bake_from_the_inspector_and_drive_a_lod_level() {
    use crate::app::{AssetSel, Cmd};
    use trino_asset_pipeline::impostor;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    let assets = std::path::PathBuf::from("assets");
    h.state_mut()
        .cmds
        .push(Cmd::SelectAsset(AssetSel::Model("models/crate".into())));
    h.run_steps(3);
    click(&mut h, "  Impostor");
    click(&mut h, "Create and bake");
    h.run_steps(3);
    for f in [
        "impostors/crate.impostor.ron",
        "shared/impostors/crate.png",
        "shared/impostors/crate.timp",
        "n64/impostors/crate.png",
        "n64/impostors/crate.timp",
    ] {
        assert!(assets.join(f).exists(), "{f}");
    }
    let manifest = std::fs::read_to_string(assets.join("manifest.toml")).unwrap();
    assert!(
        manifest.contains("[sprites.impostor-crate]")
            && manifest.contains("[blobs.impostor-crate]")
    );
    // GUI == CLI: the command-line bake finds nothing to change.
    let again = impostor::bake(&assets, "crate").unwrap();
    assert!(again.written.is_empty() && !again.manifest_changed);

    // The asset watcher rebakes; the library gets the table.
    let start = std::time::Instant::now();
    while !h.state().lib.impostors.contains_key("impostors/crate") && start.elapsed().as_secs() < 15
    {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(h.state().lib.impostors.contains_key("impostors/crate"));

    // An entity's LOD level draws it.
    let crate_a = find(&h.state().doc.scene, "crate-a");
    h.state_mut().cmds.push(Cmd::Select(Some(crate_a.clone())));
    h.state_mut().inspect_focus = Some("lod".into());
    h.run_steps(3);
    click(&mut h, "Add level");
    click(&mut h, "LOD 1 draws");
    h.get_by_label_contains(" impostors/crate")
        .click_accesskit();
    h.run_steps(3);
    let e = h.state().doc.scene.get(&crate_a).unwrap().clone();
    let lod = &e.model.as_ref().unwrap().lod;
    assert_eq!(lod.len(), 1);
    assert_eq!(lod[0].impostor.as_deref(), Some("impostors/crate"));
    // The preview puts the camera past the level: the view draws it.
    h.state_mut().lod_preview = Some((crate_a.clone(), lod[0].distance + 5.0));
    h.run_steps(4);
    assert_eq!(
        h.state().scene_view.stats.lod.impostor,
        1,
        "one impostor drawn"
    );
    let full = h.state().scene_view.stats.lod.full;
    h.state_mut().lod_preview = Some((crate_a.clone(), 0.0));
    h.run_steps(4);
    assert_eq!(h.state().scene_view.stats.lod.impostor, 0);
    assert_eq!(
        h.state().scene_view.stats.lod.full,
        full + 1,
        "back on its mesh"
    );
    // The N64 Game view resolves the N64 table (smaller tiles).
    let n64 = h
        .state()
        .resolved_game
        .items
        .iter()
        .find(|i| i.name == "crates/crate-a")
        .map(|i| i.lod[0].draw.clone());
    match n64 {
        Some(trino_scene::LodDraw::Impostor(p, imp)) => {
            assert_eq!(p, "impostors/crate");
            assert_eq!(imp.tile, impostor::n64_tile(8, 64));
        }
        other => panic!("{other:?}"),
    }

    // An agent writes impostor settings for another model: they show up.
    std::fs::write(
        assets.join("impostors/rock.impostor.ron"),
        r#"(version: 1, model: "models/rock", views: 4, tile: 32)"#,
    )
    .unwrap();
    let start = std::time::Instant::now();
    while !h.state().impostor_defs.contains_key("rock") && start.elapsed().as_secs() < 10 {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(h.state().impostor_defs["rock"].views, 4);
    h.state_mut()
        .cmds
        .push(Cmd::SelectAsset(AssetSel::Model("models/rock".into())));
    h.run_steps(4);
    assert!(h.query_by_label_contains("not baked yet").is_some());
    leave_temp_project(&repo);
}
