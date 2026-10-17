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
    // Runs a command and returns what the Console got meanwhile. The asset
    // watcher rebakes earlier steps' files on its own schedule and can log
    // in the same frames (slower on CI), so a test looks for its line among
    // these, never at `log.last()` alone.
    let cmd = |h: &mut Harness<'_, App>, c: Cmd| -> Vec<String> {
        let from = h.state().log.entries.len();
        h.state_mut().cmds.push(c);
        h.run_steps(2);
        h.state()
            .log
            .entries
            .iter()
            .skip(from)
            .map(|e| e.text.clone())
            .collect()
    };
    let said = |lines: &[String], needle: &str| {
        assert!(
            lines.iter().any(|l| l.contains(needle)),
            "expected a line with `{needle}`, the Console got {lines:?}"
        );
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
    let lines = cmd(&mut h, Cmd::Copy(nested.clone()));
    said(&lines, "copied probe");

    // Prefabs: make one from the probe, unpack it, edit a library prefab.
    cmd(
        &mut h,
        Cmd::MakePrefab(nested.clone(), "probe-prefab".into()),
    );
    assert!(root.join("prefabs/probe-prefab.prefab.ron").exists());
    assert!(h.state().lib.prefabs.contains_key("prefabs/probe-prefab"));
    let inst = find(&h.state().doc.scene, "probe");
    assert!(h.state().doc.scene.get(&inst).unwrap().prefab.is_some());
    let lines = cmd(&mut h, Cmd::MakePrefab(inst.clone(), "probe-prefab".into()));
    said(&lines, "already exists");
    cmd(&mut h, Cmd::Unpack(inst.clone()));
    assert!(h.state().doc.scene.get(&inst).unwrap().prefab.is_none());
    cmd(&mut h, Cmd::EditPrefab("prefabs/tree".into()));
    assert!(h.state().doc.is_prefab());
    assert_eq!(h.state().doc.title(), "prefabs/tree");
    // The prefab root is protected.
    let lines = cmd(&mut h, Cmd::Delete(vec![0]));
    said(&lines, "root cannot be deleted");
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
    let lines = cmd(&mut h, Cmd::NewScene("Bad Name".into()));
    said(&lines, "not a valid scene name");

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
    let lines = cmd(&mut h, Cmd::StopPlayHere);
    assert!(h.state().game_view.play.is_none());
    said(&lines, "play here stopped");

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
    // An exact label, else the one widget whose label contains it (icon
    // glyphs prefix most button and header texts).
    let find = |h: &Harness<'static, App>| -> crate::layout_audit::Widget {
        let all = crate::layout_audit::widgets(h, Some(&h.ctx));
        let rect = match h.query_by_label(label) {
            Some(n) => n.rect(),
            None => h.get_by_label_contains(label).rect(),
        };
        all.into_iter()
            .find(|w| (w.rect.min - rect.min).length() < 0.5)
            .unwrap_or(crate::layout_audit::Widget {
                role: eframe::egui::accesskit::Role::Unknown,
                label: label.into(),
                rect,
                visible: eframe::egui::Rect::NOTHING,
            })
    };
    let screen = h.ctx.content_rect();
    let shown = |h: &Harness<'static, App>| {
        let w = find(h);
        // Whatever egui says, off the window is not shown.
        let visible = w.visible.intersect(screen);
        (
            visible.is_positive() && visible.height() >= w.rect.height() - 1.0,
            crate::layout_audit::Widget { visible, ..w },
        )
    };
    // Ask egui to scroll it into view (the accessibility action) first: a
    // wheel can land on a viewport that zooms instead of scrolling.
    if !shown(h).0 {
        match h.query_by_label(label) {
            Some(n) => n.scroll_to_me(),
            None => h.get_by_label_contains(label).scroll_to_me(),
        }
        h.run_steps(8);
    }
    for _ in 0..40 {
        let (ok, w) = shown(h);
        if ok {
            break;
        }
        // Wheel over the part of its panel that shows: the widget's own
        // visible part, else a visible widget in the same column.
        let at = if w.visible.is_positive() {
            w.visible.center()
        } else {
            crate::layout_audit::widgets(h, Some(&h.ctx))
                .into_iter()
                .filter(|o| o.visible.is_positive())
                .filter(|o| (o.rect.center().x - w.rect.center().x).abs() < w.rect.width())
                .min_by(|a, b| {
                    let d = |o: &crate::layout_audit::Widget| {
                        (o.rect.center().y - w.rect.center().y).abs()
                    };
                    d(a).total_cmp(&d(b))
                })
                .map_or(w.rect.center(), |o| o.visible.center())
        };
        let down = w.rect.center().y > at.y;
        h.hover_at(at);
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, if down { -120.0 } else { 120.0 }),
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
    // An item picked through accessibility leaves its list open: close it
    // the way a user would, on its combo.
    if h.query_by_label("models/crate").is_some() {
        h.get_by_label("N64 model").click();
        h.run_steps(3);
    }
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

/// The Animation tab on the showcase robot: clips and their measurements,
/// frame steps, a crossfade, the pose exported to the same file the CLI
/// writes, and the clip written into the selected entity; then the layout
/// audit at both window sizes.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn animation_tab_previews_measures_and_exports() {
    use crate::app::{Cmd, Tab};
    for size in [egui::vec2(1600.0, 1000.0), egui::vec2(960.0, 600.0)] {
        let Some((mut h, _tmp, repo)) = temp_project_app(size) else {
            return;
        };
        let walker = find(&h.state().doc.scene, "robot-walker");
        h.state_mut().cmds.push(Cmd::Select(Some(walker.clone())));
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Animation));
        h.run_steps(4);
        assert_eq!(
            h.state().anim.model.as_deref(),
            Some("models/robot"),
            "follows the selected entity"
        );
        let names: Vec<String> = h
            .state()
            .anim
            .metrics
            .iter()
            .map(|m| m.name.clone())
            .collect();
        assert_eq!(names, ["idle", "walk", "wave"]);
        click(&mut h, " walk");
        assert_eq!(h.state().anim.clip.as_deref(), Some("walk"));
        // The showcase robot's walk skates and dips: the panel says so.
        assert!(h.query_by_label_contains("slides").is_some());
        assert!(h.query_by_label_contains("below the floor").is_some());
        click(&mut h, "Pause");
        h.state_mut().anim.time = 0.0;
        click(&mut h, "Next frame");
        let fps = {
            let m = trino_core::Mesh::from_tmdl(&h.state().project.models["models/robot"].tmdl)
                .unwrap();
            m.find_clip(trino_core::ClipId::from_name("walk"))
                .unwrap()
                .fps
        };
        assert!(
            (h.state().anim.time - 1.0 / fps).abs() < 1e-4,
            "one frame: {} at {fps} fps",
            h.state().anim.time
        );
        h.state_mut().anim.blend = Some(("wave".into(), 0.5));
        h.run_steps(3);
        assert!(h.state().anim.anim_ref().unwrap().blend.is_some());
        crate::layout_audit::assert_clean(
            &h,
            size,
            true,
            &format!("Animation tab at {}x{}", size.x, size.y),
        );
        if size.x < 1000.0 {
            leave_temp_project(&repo);
            continue;
        }
        // Export the pose: the same file the CLI writes.
        h.state_mut().anim.pose_name = "robot-pose".into();
        click(&mut h, "Export pose");
        h.run_steps(3);
        let file = std::path::Path::new("assets/shared/models/poses/robot-pose.gltf");
        let written = std::fs::read_to_string(file).unwrap_or_else(|e| {
            panic!(
                "pose not written ({e}); last log: {:?}",
                h.state().log.last().map(|l| l.text.clone())
            )
        });
        let (time, clip) = (h.state().anim.time, h.state().anim.clip.clone());
        trino_asset_pipeline::anim::export_pose(
            std::path::Path::new("assets"),
            "models/robot",
            clip.as_deref(),
            time,
            "robot-pose",
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            written,
            "GUI == CLI"
        );
        let manifest = std::fs::read_to_string("assets/manifest.toml").unwrap();
        assert!(manifest.contains("[models.robot-pose]"));
        // Write the clip into the entity: the scene gets it.
        h.state_mut().anim.speed = 1.5;
        click(&mut h, "Use on the selected entity");
        let e = h.state().doc.scene.get(&walker).unwrap();
        let a = e.model.as_ref().unwrap().anim.as_ref().unwrap();
        assert_eq!((a.clip.as_str(), a.speed), ("walk", 1.5));
        leave_temp_project(&repo);
    }
}

/// Particle effects in the editor, both ways: "+" writes the same file and
/// manifest entry `cargo xtask particles new` writes; the inspector's
/// preview bursts up to the previewed platform's cap; an edit lands in the
/// file once settled; an agent's edit to the file reloads in the inspector.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn particle_effects_preview_caps_and_files() {
    use crate::app::{AssetSel, Cmd};
    use trino_asset_pipeline::Platform;
    use trino_asset_pipeline::particles::ParticlesDef;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    let assets = std::path::PathBuf::from("assets");
    h.state_mut().assets_tab = crate::panels::assets::AssetTab::Particles;
    h.run_steps(3);
    click(&mut h, "New particle effect");
    h.run_steps(3);
    let file = assets.join("shared/particles/effect-1.particles.ron");
    let def = ParticlesDef::from_ron(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(
        std::fs::read_to_string(assets.join("manifest.toml"))
            .unwrap()
            .contains("[particles.effect-1]")
    );
    assert_eq!(
        h.state().asset_sel,
        Some(AssetSel::Particles("effect-1".into()))
    );
    assert_eq!(def.max, 32);

    // The self-test effect: bursts stop at each platform's cap.
    h.state_mut()
        .cmds
        .push(Cmd::SelectAsset(AssetSel::Particles(
            "selftest-sparks".into(),
        )));
    h.run_steps(3);
    h.state_mut().particles_view.system.emitting = false;
    for (label, cap) in [("N64 preview", 8), ("3DS preview", 16), ("PC preview", 24)] {
        click(&mut h, label);
        h.run_steps(2);
        h.state_mut().particles_view.burst = 100;
        click(&mut h, "Burst");
        h.run_steps(2);
        let v = &h.state().particles_view;
        assert_eq!(v.system.limit, cap, "{label}");
        assert_eq!(v.system.len(), cap, "{label}");
    }
    assert!(h.query_by_label_contains("24 / 24 live on PC").is_some());

    // An inspector edit is written once the user settles.
    h.state_mut()
        .particles
        .get_mut("selftest-sparks")
        .unwrap()
        .rate = 55.0;
    h.state_mut()
        .pending_particles
        .insert("selftest-sparks".into());
    h.run_steps(3);
    let disk = ParticlesDef::from_ron(
        &std::fs::read_to_string(assets.join("shared/particles/selftest-sparks.particles.ron"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(disk.rate, 55.0);

    // An agent edits the file: the inspector follows.
    let mut agent = disk.clone();
    agent.max = 12;
    std::fs::write(
        assets.join("shared/particles/selftest-sparks.particles.ron"),
        agent.to_ron(),
    )
    .unwrap();
    let start = std::time::Instant::now();
    while h.state().particles["selftest-sparks"].max != 12 && start.elapsed().as_secs() < 10 {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(h.state().particles["selftest-sparks"].max, 12);
    assert_eq!(h.state().particles["selftest-sparks"].cap(Platform::Pc), 12);
    h.run_steps(3);
    assert_eq!(
        h.state().particles_view.system.limit,
        12,
        "the preview rebuilt"
    );
    crate::layout_audit::assert_clean(&h, egui::vec2(1600.0, 1000.0), true, "particle inspector");
    leave_temp_project(&repo);
}

/// The scene's frame per console in the Inspector (scene settings): the N64
/// time with its RSP/CPU split and the reasons, the 3DS triangles, and a
/// mesh placed behind the camera listed as sent for nothing — the numbers
/// `cargo xtask scene budget` prints.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn platform_budget_shows_rsp_reasons_and_unseen_draws() {
    use crate::app::Cmd;
    for size in [egui::vec2(1600.0, 1000.0), egui::vec2(960.0, 600.0)] {
        let Some((mut h, _tmp, repo)) = temp_project_app(size) else {
            return;
        };
        let b = h.state().budgets.clone().expect("budgets computed").1;
        assert_eq!(b, h.state().platform_budgets(), "cached = fresh");
        assert!(h.query_by_label_contains("N64 ≈").is_some());
        assert!(
            h.query_all_by_label_contains("off the RSP:")
                .next()
                .is_some()
        );
        assert!(h.query_by_label_contains("3DS ").is_some());
        assert!(h.query_by_label_contains("unseen: lost-rock").is_none());
        // A rock far behind the game camera.
        let cam = h.state().doc.scene.environment.camera;
        let back = [
            cam.eye[0] * 2.0 - cam.target[0],
            cam.eye[1],
            cam.eye[2] * 2.0 - cam.target[2],
        ];
        let rock = trino_scene::Entity::new("lost-rock")
            .at(back)
            .with_model(trino_scene::ModelComponent::new("models/rock"));
        h.state_mut().cmds.push(Cmd::Create {
            parent: None,
            entity: Box::new(rock),
        });
        h.run_steps(3);
        h.state_mut().cmds.push(Cmd::Select(None));
        h.run_steps(4);
        let after = &h.state().budgets.as_ref().unwrap().1;
        assert!(
            after[0].wasted.contains(&"lost-rock".to_string()),
            "{:?}",
            after[0].wasted
        );
        assert_eq!(after[0].draws, b[0].draws + 1);
        // Its line in the Inspector selects it.
        click(&mut h, "unseen: lost-rock");
        h.run_steps(2);
        let lost = find(&h.state().doc.scene, "lost-rock");
        assert_eq!(h.state().doc.selected, Some(lost));
        h.state_mut().cmds.push(Cmd::Select(None));
        h.run_steps(4);
        crate::layout_audit::assert_clean(
            &h,
            size,
            true,
            &format!("platform budget at {}x{}", size.x, size.y),
        );
        leave_temp_project(&repo);
    }
}

/// The project launcher (no project argument): recent projects from the
/// file (`TRINO_EDITOR_RECENT`), a long missing path that must not break
/// the layout, Forget, Open with a bad folder, and Create from the
/// showcase demo, which opens the new project in the editor (its scene,
/// its assets) and puts it first in the recent file. Everything is written
/// under `target/test-projects/`.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn launcher_lists_recent_projects_opens_and_creates() {
    use crate::{Screen, Shell, Start};
    use trino_scaffold::{EngineSource, Names, Template};

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let base = repo.join("target/test-projects");
    std::fs::create_dir_all(&base).unwrap();
    let tmp = tempfile::tempdir_in(&base).unwrap();
    let engine = crate::launcher::engine_checkout().expect("built in a checkout");
    let old = tmp.path().join("old-game");
    trino_scaffold::scaffold(
        &engine,
        &old,
        &Names::new("old-game").unwrap(),
        &EngineSource::Path(engine.clone()),
        Template::Blank,
    )
    .unwrap();
    let gone = tmp.path().join(
        "a-folder-with-a-really-long-name-that-keeps-going/and-going/through-many/levels/of-nesting/gone-game",
    );
    let recent = tmp.path().join("recent-projects.txt");
    let now = crate::recent::now();
    std::fs::write(
        &recent,
        format!(
            "{}\t{}\n{}\t{}\n",
            now - 3600,
            old.display(),
            now - 3 * 86_400,
            gone.display()
        ),
    )
    .unwrap();
    std::env::set_current_dir(&repo).unwrap();
    // SAFETY: --ignored --test-threads=1 only (module docs).
    unsafe {
        std::env::set_var("TRINO_EDITOR_RECENT", &recent);
        std::env::remove_var("TRINO_EDITOR_SCENE");
    }
    let build = |size: egui::Vec2| {
        std::panic::catch_unwind(|| {
            Harness::builder()
                .with_size(size)
                .wgpu()
                .build_eframe(|cc| Shell::new(cc, &Start::Launcher))
        })
    };
    let mut last = None;
    for size in [egui::vec2(960.0, 600.0), egui::vec2(1600.0, 940.0)] {
        let mut h = match build(size) {
            Ok(h) => h,
            Err(e) => {
                assert!(
                    std::env::var("TRINO_REQUIRE_GPU").is_err(),
                    "TRINO_REQUIRE_GPU set but the editor could not start"
                );
                eprintln!("skipping launcher UI test (no GPU adapter): {e:?}");
                unsafe { std::env::remove_var("TRINO_EDITOR_RECENT") };
                return;
            }
        };
        h.run_steps(3);
        h.get_by_label("old-game");
        h.get_by_label("Open old-game");
        h.get_by_label_contains("folder not found");
        h.get_by_label_contains("opened 1 h ago");
        crate::layout_audit::assert_clean(
            &h,
            size,
            true,
            &format!("launcher at {}x{}", size.x, size.y),
        );
        last = Some(h);
    }
    let mut h = last.unwrap();

    // Forget the missing project: gone from the list and the file.
    h.get_by_label("Forget gone-game").click();
    h.run_steps(2);
    assert!(h.query_by_label("Forget gone-game").is_none());
    let file = std::fs::read_to_string(&recent).unwrap();
    assert!(
        !file.contains("gone-game") && file.contains("old-game"),
        "{file}"
    );

    // Open a folder that is not a project: the reason shows, nothing opens.
    launcher_mut(&mut h).open_path = tmp.path().display().to_string();
    h.get_by_label("Open").click();
    h.run_steps(2);
    h.get_by_label_contains("is not a Trino project");

    // Create from the showcase demo: the editor opens on it.
    let l = launcher_mut(&mut h);
    l.name = "new-game".into();
    l.folder = tmp.path().display().to_string();
    h.get_by_label("Based on the showcase demo").click();
    h.run_steps(1);
    h.get_by_label("Create project").click();
    h.run_steps(4);
    let made = tmp.path().join("new-game");
    match &h.state().screen {
        Screen::Editor(app) => {
            assert!(
                app.project.models.contains_key("models/robot"),
                "the demo's assets baked"
            );
            assert!(!app.doc.scene.entities.is_empty(), "the demo scene opened");
        }
        Screen::Launcher(l) => panic!("still on the launcher: {:?}", l.create_error),
    }
    assert_eq!(std::env::current_dir().unwrap(), made);
    assert!(made.join("game/src/showcase_gen.rs").is_file());
    let file = std::fs::read_to_string(&recent).unwrap();
    assert!(
        file.lines().next().unwrap().ends_with("/new-game"),
        "newest first:\n{file}"
    );
    drop(h);
    std::env::set_current_dir(&repo).unwrap();
    // SAFETY: as above.
    unsafe { std::env::remove_var("TRINO_EDITOR_RECENT") };
}

fn launcher_mut<'a>(
    h: &'a mut Harness<'static, crate::Shell>,
) -> &'a mut crate::launcher::Launcher {
    match &mut h.state_mut().screen {
        crate::Screen::Launcher(l) => l,
        crate::Screen::Editor(_) => panic!("the editor opened"),
    }
}

/// Scene view navigation in the real editor, with pointer and keys: MMB
/// pans, RMB held flies (WASD/QE, the app's own shortcuts stand down), Alt +
/// RMB dollies, 5 switches the projection, and the axis widget looks from an
/// axis, then from the opposite one.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn scene_view_pans_flies_dollies_and_flips_axes() {
    use egui::{Event, Key, Modifiers, PointerButton};
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 940.0)) else {
        return;
    };
    let widget = h.get_by_label("View orientation").rect().center();
    let p = widget + egui::vec2(-300.0, 260.0);
    let button = |h: &Harness<'static, App>, button, pos, pressed| {
        h.event(Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: h.input().modifiers,
        })
    };
    let drag = |h: &mut Harness<'static, App>, b: PointerButton, by: egui::Vec2| {
        h.hover_at(p);
        button(h, b, p, true);
        for k in 1..=8 {
            h.hover_at(p + by * (k as f32 / 8.0));
        }
        button(h, b, p + by, false);
        h.run_steps(2);
    };
    // Start settled: the editor eases to the scene's game camera on open,
    // and that ease would read as an orbit below.
    h.state_mut().scene_view.cam = crate::gizmo::OrbitCam::default();
    h.hover_at(p);
    h.run_steps(2);

    // Plain MMB drag: pan (the target moves, the view doesn't turn).
    let cam = h.state().scene_view.cam;
    drag(&mut h, PointerButton::Middle, egui::vec2(60.0, 0.0));
    let now = h.state().scene_view.cam;
    let moved = now.target - cam.target;
    assert!(moved.dot(moved) > 1e-4, "MMB pans");
    assert!(
        (now.yaw - cam.yaw).abs() < 1e-6,
        "MMB doesn't orbit any more"
    );
    // Ctrl + MMB pans too.
    h.input_mut().modifiers = Modifiers::COMMAND;
    drag(&mut h, PointerButton::Middle, egui::vec2(0.0, 40.0));
    h.input_mut().modifiers = Modifiers::NONE;
    let moved = h.state().scene_view.cam.target - now.target;
    assert!(moved.dot(moved) > 1e-4, "Ctrl+MMB pans");

    // Hold RMB: fly. E rises (and doesn't switch the gizmo to rotate),
    // W moves forward, and Shift+A doesn't open the fake-object dialog.
    let cam = h.state().scene_view.cam;
    h.hover_at(p);
    button(&h, PointerButton::Secondary, p, true);
    h.run_steps(2);
    assert!(h.state().scene_view.flying(), "RMB held flies");
    for key in [Key::E, Key::W] {
        h.key_down(key);
        h.run_steps(6);
        h.key_up(key);
        h.run_steps(1);
    }
    h.key_press_modifiers(Modifiers::SHIFT, Key::A);
    h.run_steps(2);
    button(&h, PointerButton::Secondary, p, false);
    h.run_steps(2);
    let now = h.state().scene_view.cam;
    assert!(!h.state().scene_view.flying(), "released");
    assert!(h.state().dialog.is_none(), "no shortcut fired while flying");
    assert_eq!(
        h.state().scene_view.gizmo,
        crate::gizmo::GizmoMode::Translate
    );
    assert!(now.eye().y > cam.eye().y, "E rises");
    let fwd = cam.target - cam.eye();
    assert!((now.eye() - cam.eye()).dot(fwd) > 0.0, "W moves forward");
    assert!(
        (now.dist - cam.dist).abs() < 1e-4,
        "flying keeps the orbit distance"
    );

    // Alt + RMB drag up: dolly in.
    h.input_mut().modifiers = Modifiers::ALT;
    drag(&mut h, PointerButton::Secondary, egui::vec2(0.0, -60.0));
    h.input_mut().modifiers = Modifiers::NONE;
    assert!(!h.state().scene_view.flying(), "Alt+RMB doesn't fly");
    assert!(h.state().scene_view.cam.dist < now.dist * 0.9, "dollied in");

    // 5 (with the view hovered): orthographic, and back; flying from an
    // orthographic view goes perspective.
    h.hover_at(p);
    h.key_press(Key::Num5);
    h.run_steps(2);
    assert!(h.state().scene_view.cam.ortho);
    h.key_press(Key::Num5);
    h.run_steps(2);
    assert!(!h.state().scene_view.cam.ortho);
    h.key_press(Key::Num5);
    h.run_steps(2);
    button(&h, PointerButton::Secondary, p, true);
    h.run_steps(2);
    assert!(!h.state().scene_view.cam.ortho, "flying is perspective");
    button(&h, PointerButton::Secondary, p, false);
    h.run_steps(2);

    // The axis widget: +X looks from +X; clicking it again looks from -X.
    let tip = |h: &Harness<'static, App>, idx: usize| {
        crate::gizmo::axis_tips(&h.state().scene_view.cam.camera().view(), widget)
            .into_iter()
            .find(|t| t.0 == idx)
            .unwrap()
            .1
    };
    // The ease runs on wall-clock time: step until it arrives (bounded).
    let click_and_settle = |h: &mut Harness<'static, App>, at: egui::Pos2, want: usize| {
        h.hover_at(at);
        h.drag_at(at);
        h.drop_at(at);
        let start = std::time::Instant::now();
        loop {
            h.run_steps(1);
            let cam = h.state().scene_view.cam;
            if crate::gizmo::aligned_axis(cam.yaw, cam.pitch) == Some(want) {
                break;
            }
            assert!(
                start.elapsed().as_secs_f32() < 5.0,
                "never looked from axis {want}: yaw {} pitch {}",
                cam.yaw,
                cam.pitch
            );
        }
        // Let the last of the ease settle before reading the tips again.
        h.run_steps(20);
    };
    let at = tip(&h, 0);
    click_and_settle(&mut h, at, 0);
    let at = tip(&h, 0);
    click_and_settle(&mut h, at, 3);
    assert!(h.state().doc.selected.is_none(), "widget clicks don't pick");
    leave_temp_project(&repo);
}

/// A gizmo drag takes the handle under where the button went down: a quick
/// drag off the thin ground-plane square (the pointer already far from it
/// when egui decides it is a drag) still moves on the ground plane.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn gizmo_drags_grab_the_handle_under_the_press() {
    use trino_core::Vec3;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 940.0)) else {
        return;
    };
    let rock = find(&h.state().doc.scene, "rock-1");
    h.state_mut()
        .cmds
        .push(crate::app::Cmd::Select(Some(rock.clone())));
    h.state_mut().scene_view.cam = crate::gizmo::OrbitCam::default();
    h.run_steps(10);
    // The view's image: its top-right corner anchors the axis widget.
    let widget = h.get_by_label("View orientation").rect().center();
    let (iw, ih) = h.state().scene_view.internal_size();
    let rect = egui::Rect::from_min_size(
        egui::pos2(widget.x + 48.0 - iw as f32, widget.y - 52.0),
        egui::vec2(iw as f32, ih as f32),
    );
    let cam = h.state().scene_view.cam.camera();
    let proj = crate::gizmo::Proj::new(&cam, (iw, ih), rect);
    let start = h.state().doc.scene.get(&rock).unwrap().transform.pos;
    let origin = Vec3::new(start[0], start[1], start[2]);
    let g = crate::gizmo::Gizmo::new(&proj, origin);
    let square = proj
        .project(origin + Vec3::new(g.len * 0.32, 0.0, g.len * 0.32))
        .unwrap();
    assert_eq!(
        g.hit(&proj, crate::gizmo::GizmoMode::Translate, square),
        Some(crate::gizmo::Handle::Plane(1))
    );
    h.hover_at(square);
    h.run_steps(2);
    h.drag_at(square);
    h.run_steps(1);
    // One jump, well off every handle.
    h.hover_at(square + egui::vec2(-80.0, 50.0));
    h.run_steps(2);
    h.drop_at(square + egui::vec2(-80.0, 50.0));
    h.run_steps(3);
    let now = h.state().doc.scene.get(&rock).unwrap().transform.pos;
    assert!(
        (now[1] - start[1]).abs() < 1e-4,
        "stays on the ground plane"
    );
    assert!(
        (now[0] - start[0]).abs() > 0.1 && (now[2] - start[2]).abs() > 0.1,
        "moved in X and Z: {start:?} -> {now:?}"
    );
    leave_temp_project(&repo);
}

/// Ambient occlusion from the editor: the scene's screen-space AO panel
/// (checkbox, saved to the file, undoable), the Scene view's modes from the
/// View menu (off, the mask), and a model's baked AO written to the
/// manifest and rebaked by the watcher.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn ambient_occlusion_panel_view_modes_and_baking() {
    use crate::app::{AssetSel, Cmd};
    use trino_platform_pc::AoView;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    assert_eq!(h.state().doc.scene.environment.ao, None);
    h.state_mut().cmds.push(Cmd::Select(None));
    h.run_steps(3);
    click(&mut h, "Screen-space AO (PC)");
    h.run_steps(4);
    let ao = h.state().doc.scene.environment.ao;
    assert_eq!(ao, Some(trino_scene::AmbientOcclusionDef::default()));
    assert!(h.state().scene_view.stats.ao, "the Scene view runs it");
    // The tuning shows; "Reset" is there.
    assert!(h.query_by_label_contains("Reset to defaults").is_some());
    h.state_mut().cmds.push(Cmd::Save);
    h.run_steps(3);
    let text = std::fs::read_to_string("scenes/showcase.scene.ron").unwrap();
    assert!(text.contains("ao: Some("), "saved to the scene file");
    // Off from the checkbox keeps the tuning in the file.
    click(&mut h, "Screen-space AO (PC)");
    h.run_steps(4);
    let off = h.state().doc.scene.environment.ao.unwrap();
    assert!(!off.enabled);
    assert!(!h.state().scene_view.stats.ao);
    h.state_mut().cmds.push(Cmd::Undo);
    h.run_steps(4);
    assert!(h.state().doc.scene.environment.ao.unwrap().enabled, "undo");

    // View › Ambient occlusion: the mask, then off whatever the scene says.
    click(&mut h, "View");
    click(&mut h, "Ambient occlusion");
    click(&mut h, "AO only (mask)");
    h.run_steps(4);
    assert_eq!(h.state().scene_view.ao_view, AoView::Mask);
    assert!(h.state().scene_view.stats.ao);
    h.state_mut().scene_view.ao_view = AoView::Off;
    h.run_steps(4);
    assert!(!h.state().scene_view.stats.ao, "a mode change re-renders");

    // Model › Ambient occlusion (baked): the manifest line, then the bake.
    h.state_mut()
        .cmds
        .push(Cmd::SelectAsset(AssetSel::Model("models/crate".into())));
    h.run_steps(3);
    // Through accesskit: a toast (the save, the undo) may cover them. A
    // lone box hides nothing from itself: the ground contact darkens it.
    let before = h.state().project.models["models/crate"].tmdl.clone();
    h.get_by_label_contains("Ground contact").click_accesskit();
    h.run_steps(2);
    h.get_by_label_contains("Bake AO to vertex colors")
        .click_accesskit();
    h.run_steps(3);
    let manifest = std::fs::read_to_string("assets/manifest.toml").unwrap();
    let table = manifest
        .split("[models.crate]")
        .nth(1)
        .and_then(|t| t.split("\n[").next())
        .unwrap();
    assert!(
        table.contains("ao = { strength = 0.8, ground = true }"),
        "{table}"
    );
    assert!(h.query_by_label_contains("Re-bake AO").is_some());
    let start = std::time::Instant::now();
    while h.state().project.models["models/crate"].tmdl == before && start.elapsed().as_secs() < 15
    {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        h.state().project.models["models/crate"].tmdl != before,
        "the watcher rebaked it with AO"
    );
    h.get_by_label_contains("Remove").click_accesskit();
    h.run_steps(3);
    let manifest = std::fs::read_to_string("assets/manifest.toml").unwrap();
    let table = manifest
        .split("[models.crate]")
        .nth(1)
        .and_then(|t| t.split("\n[").next())
        .unwrap();
    assert!(!table.contains("ao ="), "{table}");
    assert!(h.state().project.models["models/crate"].ao.is_none());
    leave_temp_project(&repo);
}
