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
    // The scene has long groups (the ground's tiles): scroll to it first.
    click(&mut h, "loose-crate");
    h.run_steps(2);
    let sel = h
        .state()
        .doc
        .selected
        .clone()
        .expect("loose-crate selected");
    assert_eq!(h.state().doc.scene.get(&sel).unwrap().name, "loose-crate");

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

    // Reparent under the river, then copy as RON.
    let probe = find(&h.state().doc.scene, "probe");
    let river = find(&h.state().doc.scene, "river");
    cmd(
        &mut h,
        Cmd::Reparent {
            from: probe,
            to: Some(river.clone()),
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
    temp_project_app_with(size, |_, _| {})
}

/// [`temp_project_app`] with `setup` run on the project folder (the repo,
/// the project) before the editor opens it.
fn temp_project_app_with(
    size: egui::Vec2,
    setup: impl FnOnce(&std::path::Path, &std::path::Path),
) -> Option<(Harness<'static, App>, tempfile::TempDir, std::path::PathBuf)> {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    // Under target/test-projects/, never the system temp dir (a RAM disk
    // on some machines).
    let base = repo.join("target/test-projects");
    std::fs::create_dir_all(&base).unwrap();
    let tmp = tempfile::tempdir_in(&base).unwrap();
    for dir in ["assets", "scenes", "prefabs", "ui"] {
        copy_tree(&repo.join(dir), &tmp.path().join(dir));
    }
    copy_tree(
        &repo.join("examples/platformer/src"),
        &tmp.path().join("examples/platformer/src"),
    );
    setup(&repo, tmp.path());
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
            .unwrap_or_else(|| {
                // Not an audited leaf (a clickable label has a text run
                // child): its clipped rect from egui's own widget list.
                let visible = h.ctx.viewport(|v| {
                    v.this_pass
                        .widgets
                        .layers()
                        .flat_map(|(_, ws)| ws.iter())
                        .find(|w| {
                            (w.rect.min - rect.min).length() < 0.5
                                && (w.rect.max - rect.max).length() < 0.5
                        })
                        .map_or(eframe::egui::Rect::NOTHING, |w| {
                            rect.intersect(w.interact_rect)
                        })
                });
                crate::layout_audit::Widget {
                    role: eframe::egui::accesskit::Role::Unknown,
                    label: label.into(),
                    rect,
                    visible,
                    floating: false,
                }
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
        app.game_view.set_profile_tier(
            trino_platform_pc::SimProfile::N64,
            trino_core::HwTier::Base,
            &app.project,
        );
    }
    h.run_steps(3);
    let rock_id = ModelId(trino_core::asset_id("models/rock"));
    let crate_id = ModelId(trino_core::asset_id("models/crate"));
    assert_eq!(game_model(h.state(), "rock-1"), Some(rock_id));
    let tris_before = h.state().game_view.stats.tris;

    // Open the Platforms section (scrolled into view: the Inspector grew a
    // Web column and a fourth render path) and pick another model for the
    // N64.
    click(&mut h, "Platforms");
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

    // The Web (issue #47) follows the PC: the N64's choices don't reach a
    // browser's Game view.
    {
        let app = h.state_mut();
        app.game_view.set_profile_tier(
            trino_platform_pc::SimProfile::Web,
            trino_core::HwTier::Base,
            &app.project,
        );
    }
    h.run_steps(4);
    assert_eq!(game_model(h.state(), "rock-1"), Some(rock_id), "Web view");
    {
        let app = h.state_mut();
        app.game_view.set_profile_tier(
            trino_platform_pc::SimProfile::N64,
            trino_core::HwTier::Base,
            &app.project,
        );
    }
    h.run_steps(4);

    // GUI → file: saving writes the variant as text an agent can read.
    h.state_mut().cmds.push(Cmd::Save);
    h.run_steps(3);
    let disk = std::fs::read_to_string("scenes/showcase.scene.ron").unwrap();
    let saved = trino_scene::Scene::from_ron(&disk).unwrap();
    let r = saved.get(&rock).unwrap();
    assert!(disk.contains(&format!("version: {}", trino_scene::SCENE_VERSION)));
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
        app.game_view.set_profile_tier(
            trino_platform_pc::SimProfile::N3ds,
            trino_core::HwTier::Base,
            &app.project,
        );
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

/// Hardware tiers (issue #54) end to end: "New 3DS" in the Game view's
/// profile picker, a New 3DS variant in Inspector › Platforms (its column
/// shown under the 3DS's) that the Game view draws and the 3DS doesn't,
/// undo and redo, the file, and the platform budget's tiers side by side
/// with their RAM.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn new_3ds_tier_variant_reaches_the_game_view_the_file_and_the_budget() {
    use crate::app::{Cmd, Tab};
    use trino_core::{HwTier, ModelId};
    use trino_platform_pc::SimProfile;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    let rock = find(&h.state().doc.scene, "rock-1");
    h.state_mut().cmds.push(Cmd::Select(Some(rock.clone())));
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
    h.run_steps(3);
    let rock_id = ModelId(trino_core::asset_id("models/rock"));
    let crate_id = ModelId(trino_core::asset_id("models/crate"));

    // The profile picker: the New 3DS, after the 3DS.
    click(&mut h, "New 3DS");
    h.run_steps(3);
    let view = &h.state().game_view;
    assert_eq!(
        (view.profile, view.tier),
        (SimProfile::N3ds, HwTier::Expanded)
    );
    assert_eq!(view.target(), trino_scene::Target::N3dsNew);
    assert_eq!(view.profile_key(), "3dsnew");
    assert_eq!(view.target_label(), "New 3DS");
    assert_eq!(game_model(h.state(), "rock-1"), Some(rock_id));

    // Inspector › Platforms: the New 3DS column, collapsed under the 3DS
    // until shown, follows the 3DS; then a variant of its own.
    click(&mut h, "Platforms");
    h.run_steps(2);
    assert!(h.query_by_label("New 3DS model").is_none(), "collapsed");
    click(&mut h, "Show New 3DS");
    h.run_steps(2);
    assert!(h.query_by_label("as on 3DS").is_some());
    click(&mut h, "New 3DS model");
    h.get_by_label("models/crate").click_accesskit();
    h.run_steps(3);
    if h.query_by_label("models/crate").is_some() {
        h.get_by_label("New 3DS model").click();
        h.run_steps(3);
    }
    let e = h.state().doc.scene.get(&rock).unwrap().clone();
    assert_eq!(
        e.platforms.n3ds_new.as_ref().unwrap().model.as_deref(),
        Some("models/crate")
    );
    assert!(e.platforms.n3ds.is_none(), "the 3DS keeps the PC look");
    assert!(h.query_by_label("as on 3DS").is_none());
    assert!(
        h.query_by_label("New 3DS model").is_some(),
        "the expanded column"
    );
    assert_eq!(game_model(h.state(), "rock-1"), Some(crate_id), "New 3DS");
    {
        let app = h.state_mut();
        app.game_view
            .set_profile_tier(SimProfile::N3ds, HwTier::Base, &app.project);
    }
    h.run_steps(3);
    assert_eq!(game_model(h.state(), "rock-1"), Some(rock_id), "3DS");
    click(&mut h, "New 3DS");
    h.run_steps(3);
    assert_eq!(game_model(h.state(), "rock-1"), Some(crate_id));

    // Undo and redo the tier's variant.
    h.state_mut().cmds.push(Cmd::Undo);
    h.run_steps(3);
    let tier = |h: &Harness<'static, App>| {
        h.state()
            .doc
            .scene
            .get(&rock)
            .unwrap()
            .platforms
            .n3ds_new
            .clone()
    };
    assert_eq!(tier(&h), None, "undone");
    assert_eq!(game_model(h.state(), "rock-1"), Some(rock_id));
    h.state_mut().cmds.push(Cmd::Redo);
    h.run_steps(3);
    assert_eq!(
        tier(&h).and_then(|v| v.model).as_deref(),
        Some("models/crate"),
        "redone"
    );
    assert_eq!(game_model(h.state(), "rock-1"), Some(crate_id));

    // The file says it as text.
    h.state_mut().cmds.push(Cmd::Save);
    h.run_steps(3);
    let disk = std::fs::read_to_string("scenes/showcase.scene.ron").unwrap();
    assert!(disk.contains("n3ds_new"));
    let saved = trino_scene::Scene::from_ron(&disk).unwrap();
    assert_eq!(saved.get(&rock).unwrap().platforms, e.platforms);

    // The platform budget: each console's tiers side by side, their RAM.
    h.state_mut().cmds.push(Cmd::Select(None));
    h.state_mut().inspect_focus = Some("budget".into());
    h.run_steps(4);
    let budgets = h.state().budgets.as_ref().unwrap().1.clone();
    let ram = |t| trino_scene::budget::ram_line(crate::budget::budget_of(&budgets, t)).unwrap();
    let new = ram(trino_scene::Target::N3dsNew);
    assert!(
        new.starts_with("New 3DS RAM ") && new.ends_with(" of 65536 KB"),
        "{new}"
    );
    assert!(h.query_by_label(&new).is_some(), "the New 3DS RAM line");
    for t in [
        trino_scene::Target::N64,
        trino_scene::Target::N64Expanded,
        trino_scene::Target::N3ds,
    ] {
        assert!(h.query_by_label(&ram(t)).is_some(), "{}", ram(t));
    }
    for line in [
        "New 3DS: 2 cores at 804 MHz",
        "N64+Pak: 1 core at 93 MHz, 8 MB RAM",
    ] {
        assert!(h.query_by_label_contains(line).is_some(), "{line}");
    }
    // The tier tables' columns, the Game view's tier among them.
    assert!(h.query_all_by_label("New 3DS").count() >= 2);
    assert!(h.query_by_label("2×804 MHz").is_some());
    assert!(h.query_by_label("1×268 MHz").is_some());
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
        crate::layout_audit::assert_clean(&mut h, size, true, &what("scene settings"));
        let rock = find(&h.state().doc.scene, "rock-1");
        h.state_mut().cmds.push(Cmd::Select(Some(rock.clone())));
        h.run_steps(3);
        h.get_by_label_contains("Platforms").click();
        h.run_steps(3);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("entity inspector"));
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
        h.run_steps(3);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Game view"));
        // The hardware tiers (issue #54): both tier columns shown, the Game
        // view on each expanded tier. (At 960x600 Platforms is still
        // closed here: open it scrolled into view.)
        if h.query_by_label("Show N64 + Expansion Pak").is_none() {
            click(&mut h, "Platforms");
            h.run_steps(2);
        }
        click(&mut h, "Show N64 + Expansion Pak");
        click(&mut h, "Show New 3DS");
        h.run_steps(3);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("tier columns"));
        for profile in [
            trino_platform_pc::SimProfile::N64,
            trino_platform_pc::SimProfile::N3ds,
        ] {
            {
                let app = h.state_mut();
                app.game_view
                    .set_profile_tier(profile, trino_core::HwTier::Expanded, &app.project);
            }
            h.run_steps(3);
            let label = h.state().game_view.target_label();
            crate::layout_audit::assert_clean(
                &mut h,
                size,
                true,
                &what(&format!("{label} Game view")),
            );
        }
        // The 3DS stereo preview (issue #63): its menu and 3D slider in the
        // Game view's toolbar, on the New 3DS left from the loop.
        h.state_mut().game_view.stereo_view = trino_platform_pc::StereoView::Anaglyph;
        h.run_steps(3);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("stereo preview"));
        h.state_mut().game_view.stereo_view = trino_platform_pc::StereoView::Off;
        h.state_mut().cmds.push(Cmd::Select(None));
        h.state_mut().inspect_focus = Some("budget".into());
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("tier budgets"));
        // The scene's Stereo 3D section, on (issue #63).
        h.state_mut().doc.scene.environment.stereo = Some(trino_scene::StereoDef::default());
        h.state_mut().inspect_focus = Some("stereo".into());
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("stereo section"));
        h.state_mut().doc.scene.environment.stereo = None;
        h.state_mut().inspect_focus = None;
        h.state_mut().cmds.push(Cmd::Select(Some(rock.clone())));
        h.run_steps(3);
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
        crate::layout_audit::assert_clean(&mut h, size, true, &what("LOD panel"));
        // A model asset with its impostor section.
        h.state_mut().inspect_focus = Some("impostor".into());
        h.state_mut()
            .cmds
            .push(Cmd::SelectAsset(crate::app::AssetSel::Model(
                "models/c64_boar".into(),
            )));
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("impostor section"));
        // A slotted card with its color slots and an Outline (issue #96).
        let mut card = trino_scene::Entity::new("card").with_model(trino_scene::ModelComponent {
            palette: vec![trino_scene::SlotColorDef {
                slot: 1,
                color: trino_scene::SlotColor::Semantic("castle64.door.red".into()),
            }],
            ..trino_scene::ModelComponent::new("models/selftest_slots")
        });
        card.outline = Some(trino_scene::OutlineComponent::default());
        h.state_mut().cmds.push(Cmd::Create {
            parent: None,
            entity: Box::new(card),
        });
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("color slots and outline"));
        // The texture inspector, measured, its formats scrolled into view,
        // with a bake error (ETC1 under a min_psnr) in a column.
        let photo = "sprites/selftest_photo_etc1";
        h.state_mut().cmds.push(Cmd::SetTextureKey(
            photo.into(),
            "min_psnr",
            Some("40".into()),
        ));
        h.state_mut().inspect_focus = Some("texture".into());
        h.state_mut()
            .cmds
            .push(Cmd::SelectAsset(crate::app::AssetSel::Sprite(photo.into())));
        wait_for(&mut h, "the texture inspector", |a| {
            a.textures
                .current
                .as_ref()
                .is_some_and(|(_, i, _)| i.logical == photo && i.settings.min_psnr == Some(40.0))
        });
        crate::layout_audit::assert_clean(&mut h, size, true, &what("texture inspector"));
        // The audio inspector, measured, its platforms scrolled into view;
        // then the Mixer tab.
        h.state_mut().inspect_focus = Some("audio".into());
        h.state_mut()
            .cmds
            .push(Cmd::SelectAsset(crate::app::AssetSel::Audio(
                "music/theme".into(),
            )));
        wait_for(&mut h, "the audio inspector", |a| {
            a.audio
                .current
                .as_ref()
                .is_some_and(|(_, i)| i.logical == "music/theme")
        });
        crate::layout_audit::assert_clean(&mut h, size, true, &what("audio inspector"));
        // A captioned sound's Caption section (issue #96).
        h.state_mut().inspect_focus = None;
        h.state_mut()
            .cmds
            .push(Cmd::SelectAsset(crate::app::AssetSel::Audio(
                "sounds/beep".into(),
            )));
        wait_for(&mut h, "the caption section", |a| {
            a.audio.caption_shown().is_some_and(|c| c.key.is_some())
        });
        crate::layout_audit::assert_clean(&mut h, size, true, &what("audio caption"));
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Mixer));
        h.run_steps(3);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Mixer tab"));
        // The quality presets (#55): the Quality tab, the Game view's PC
        // toolbar with its preset picker and the dynres toggle.
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Quality));
        h.run_steps(3);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Quality tab"));
        {
            let app = h.state_mut();
            app.game_view.set_profile_tier(
                trino_platform_pc::SimProfile::Native,
                trino_core::HwTier::Base,
                &app.project,
            );
        }
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
        h.run_steps(3);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("PC Game view"));
        // Asset groups (issue #56): the Memory tab measured, the Assets
        // panel's group chips, the scene settings' Asset groups.
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Memory));
        h.run_steps(3);
        wait_for(&mut h, "the memory plans", |a| a.groups.measured());
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Memory tab"));
        // The Accessibility tab (issue #60): simulation, colors, checklist.
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Accessibility));
        h.run_steps(3);
        wait_for(&mut h, "the accessibility checklist", |a| {
            a.access.report.is_some()
        });
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Accessibility tab"));
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Assets));
        h.state_mut().assets_tab = crate::panels::assets::AssetTab::Sprites;
        h.state_mut().groups.filter = Some("selftest_level".into());
        h.state_mut().cmds.push(Cmd::Select(None));
        h.state_mut().inspect_focus = Some("groups".into());
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("asset groups"));
        // Shadows (ADR 0025): the scene's section with its tier lines, an
        // entity's Shadow component.
        h.state_mut().doc.scene.environment.shadows = Some(trino_scene::ShadowsDef::default());
        let rock_shadow = trino_scene::ShadowComponent {
            receive: true,
            ..Default::default()
        };
        h.state_mut().doc.scene.get_mut(&rock).unwrap().shadow = Some(rock_shadow);
        h.state_mut().inspect_focus = Some("shadows".into());
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("scene shadows"));
        h.state_mut().inspect_focus = None;
        h.state_mut().cmds.push(Cmd::Select(Some(rock.clone())));
        h.run_steps(3);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Shadow component"));
        // The UI tab (#58): the self-test screen with a node selected (its
        // inspector), then on the 3DS (both screens) with another.
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Ui));
        h.state_mut().ui_screens.selected = Some(vec![1, 0]);
        h.run_steps(4);
        assert!(h.query_by_label("UI preview").is_some());
        crate::layout_audit::assert_clean(&mut h, size, true, &what("UI tab"));
        h.state_mut().ui_screens.target = 1;
        h.state_mut().ui_screens.selected = Some(vec![2]);
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("UI tab on the 3DS"));
        // The Localization tab (#59): the table, its issues and fonts, over
        // tables with a missing key, a fallback and a plural.
        let i18n = std::path::Path::new("assets/i18n");
        std::fs::create_dir_all(i18n).unwrap();
        std::fs::copy(
            repo.join("templates/standalone/assets/i18n/en-US.ron"),
            i18n.join("en-US.ron"),
        )
        .unwrap();
        std::fs::write(i18n.join("pt-BR.ron"), PT_BR).unwrap();
        std::fs::write(
            i18n.join("ru.ron"),
            "Strings(lang: \"ru\", entries: {\"n\": Plural(one: \"{n}\", other: \"{n}\")})\n",
        )
        .unwrap();
        let root = h.state().project.root.clone();
        h.state_mut().localization.reload(&root);
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Localization));
        h.run_steps(4);
        assert!(h.query_by_label("Preview language").is_some());
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Localization tab"));
        // The Project tab (issue #61): both pages.
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Project));
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Project › Achievements"));
        h.state_mut().shipping.page = crate::shipping::Page::Packaging;
        h.run_steps(3);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Project › Packaging"));
        h.state_mut().shipping.page = crate::shipping::Page::Physics;
        h.run_steps(3);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Project › Physics"));
        // The Profiler tab (issue #62): empty, then A and B compared.
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Profiler));
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Profiler (empty)"));
        let root = std::env::current_dir().unwrap();
        profiler_fixtures(&root, false);
        let p = &mut h.state_mut().profiler;
        p.open_a(&root.join("target/traces/a.json"), None);
        p.open_b(&root.join("target/traces/b.log"), None);
        h.run_steps(4);
        assert!(h.query_by_label_contains("DIFF (B").is_some());
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Profiler A vs B"));
        // Its Reports and A/B views (issue #79).
        perf_fixtures(&root);
        let p = &mut h.state_mut().profiler;
        p.reports = crate::profiler::Reports::new(&root);
        p.mode = crate::profiler::Mode::Reports;
        let b = root.join("target/perf/n64-throne.json");
        p.reports.open(&b, true);
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Profiler reports A vs B"));
        h.state_mut().profiler.mode = crate::profiler::Mode::Ab;
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Profiler A/B"));
        h.state_mut().profiler.mode = crate::profiler::Mode::Timeline;
        // The Net tab and Play here's bar with a replay (issue #67).
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Net));
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Net tab"));
        // The Saves tab (issue #68), with a damaged slot pair.
        save_fixtures(&std::env::current_dir().unwrap().join("target/save"));
        h.state_mut().saves = Default::default();
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Saves));
        h.run_steps(4);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Saves tab"));
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
        h.state_mut().record_play_here();
        h.run_steps(30);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Play here recording"));
        let bytes = h
            .state_mut()
            .game_view
            .play
            .as_mut()
            .unwrap()
            .end_recording()
            .unwrap();
        let file = std::env::current_dir().unwrap().join("target/layout.trec");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, bytes).unwrap();
        h.state_mut().replay_here(&file);
        h.run_steps(6);
        crate::layout_audit::assert_clean(&mut h, size, true, &what("Play here replay"));
        h.state_mut().cmds.push(crate::app::Cmd::StopPlayHere);
        h.run_steps(2);
        leave_temp_project(&repo);
    }
}

/// With `TRINO_EVIDENCE_DIR` set, write the window as `<dir>/<name>` (PR
/// evidence; nothing otherwise).
fn evidence(h: &mut Harness<'static, App>, name: &str) {
    let Ok(dir) = std::env::var("TRINO_EVIDENCE_DIR") else {
        return;
    };
    let Ok(img) = h.render() else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(std::path::Path::new(&dir).join(name)).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width(), img.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .unwrap()
        .write_image_data(img.as_raw())
        .unwrap();
}

/// Two saves of a game, the newest damaged (bit rot): what the Saves tab
/// and `cargo xtask save inspect` show as recovered from the backup.
fn save_fixtures(dir: &std::path::Path) {
    use trino_core::save;
    std::fs::create_dir_all(dir).unwrap();
    let mut a = vec![0u8; 64];
    let n = save::encode_slot(1, b"coins=3", &mut a).unwrap();
    std::fs::write(dir.join("game.sav.a"), &a[..n]).unwrap();
    let mut b = vec![0u8; 64];
    let n = save::encode_slot(2, b"coins=9", &mut b).unwrap();
    b[n - 2] ^= 0x10;
    std::fs::write(dir.join("game.sav.b"), &b[..n]).unwrap();
}

/// Damage the editor surfaces instead of crashing (issue #68): a baked
/// model whose bytes broke (a bad bake, a disk error) is a Console error
/// with its path and a Re-bake button that rewrites it from its master; the
/// Saves tab decodes a damaged slot pair as the engine reads it.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn damaged_assets_rebake_from_the_console_and_saves_decode_in_their_tab() {
    use crate::app::{Cmd, Tab};
    let Some((mut h, tmp, repo)) = temp_project_app_with(egui::vec2(1600.0, 1000.0), |_, _| {})
    else {
        return;
    };
    let logical = "models/selftest_card";
    let id = h.state_mut().project.models[logical].id;
    let file = tmp
        .path()
        .join(crate::project::BAKED)
        .join(format!("{id:08x}.model"));
    let good = std::fs::read(&file).unwrap();
    // Indices past the vertex count: the T&L would read out of bounds.
    let mut bad = good.clone();
    let at = bad.len() - 2;
    bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    bad.truncate(at);
    std::fs::write(&file, &bad).unwrap();
    {
        let app = h.state_mut();
        app.project.reload(None).unwrap();
        crate::app::log_load_errors(&mut app.log, &app.project);
        assert!(app.project.load_errors.iter().any(|e| e.logical == logical));
    }
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Console));
    h.run_steps(4);
    // In the Console (and the status bar, which shows the last entry).
    assert!(
        h.query_all_by_label_contains("asset models/selftest_card did not load")
            .count()
            >= 1
    );
    evidence(&mut h, "console-damaged-asset.png");
    click(&mut h, "Re-bake");
    h.run_steps(6);
    evidence(&mut h, "console-rebaked.png");
    assert!(
        h.state_mut().project.load_errors.is_empty(),
        "{:?}",
        h.state_mut().project.load_errors
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        good,
        "the bake rewrote the damaged file"
    );
    assert!(h.state_mut().project.models.contains_key(logical));

    save_fixtures(&tmp.path().join("target/save"));
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Saves));
    h.run_steps(4);
    assert!(h.query_by_label("game.sav").is_some());
    assert!(h.query_by_label("recovered-from-backup").is_some());
    assert!(
        h.query_by_label_contains("Reads 7 bytes from game.sav.a")
            .is_some()
    );
    evidence(&mut h, "saves-tab.png");

    // The N64 source (issue #121): ares's EEPROM image beside the ROM.
    console_save_fixtures(tmp.path());
    let root = tmp.path().to_path_buf();
    h.state_mut()
        .saves
        .show(crate::panels::saves::Source::N64, &root);
    h.run_steps(4);
    assert!(h.query_by_label("castle64.eeprom").is_some());
    assert!(h.query_by_label_contains("Reads 13 bytes from").is_some());
    evidence(&mut h, "saves-tab-n64.png");
    // The 3DS source: Azahar's SD card, a game's folder before its first
    // save (it reads the shared folder's once) and the shared folder.
    h.state_mut()
        .saves
        .show(crate::panels::saves::Source::N3ds, &root);
    h.run_steps(4);
    assert!(h.query_by_label("castle64").is_some());
    assert!(
        h.query_by_label("3ds/trino (shared, before #119)")
            .is_some()
    );
    h.state_mut().saves.selected = Some(1);
    h.state_mut().saves.inspect();
    h.run_steps(4);
    assert!(
        h.query_by_label_contains("from the shared folder (migrates once)")
            .is_some()
    );
    evidence(&mut h, "saves-tab-3ds.png");
    // SAFETY: tests that change the environment run alone (`--test-threads=1`).
    unsafe { std::env::remove_var("TRINO_AZAHAR_SDMC") };
    leave_temp_project(&repo);
}

/// An N64 EEPROM image as ares writes it beside the ROM (two saves, A/B)
/// and an Azahar SD card (`TRINO_AZAHAR_SDMC`) with the shared folder from
/// before #119 holding a save and castle64's own folder still empty.
fn console_save_fixtures(root: &std::path::Path) {
    use trino_core::save::{self, Lz, Medium};
    struct Image(Vec<u8>);
    impl Medium for Image {
        fn size(&self) -> usize {
            self.0.len()
        }
        fn read(&mut self, at: usize, buf: &mut [u8]) {
            buf.copy_from_slice(&self.0[at..at + buf.len()]);
        }
        fn write(&mut self, at: usize, data: &[u8]) {
            self.0[at..at + data.len()].copy_from_slice(data);
        }
    }
    let mut m = Image(vec![0xFF; 2048]);
    let (mut body, mut scratch) = (vec![0u8; 2048], vec![0u8; 2048]);
    for data in [&b"stars=1;coins=4"[..], b"stars=2;coins"] {
        save::write_now(&mut m, data, &mut body, &mut scratch, &mut Lz::default()).unwrap();
    }
    std::fs::create_dir_all(root.join("target/n64")).unwrap();
    std::fs::write(root.join("target/n64/castle64.eeprom"), &m.0).unwrap();
    let sd = root.join("sdmc");
    let shared = sd.join("3ds/trino");
    std::fs::create_dir_all(shared.join("castle64")).unwrap();
    let mut a = vec![0u8; 64];
    let n = save::encode_slot(4, b"shared-save", &mut a).unwrap();
    std::fs::write(shared.join("save.a"), &a[..n]).unwrap();
    // SAFETY: tests that change the environment run alone (`--test-threads=1`).
    unsafe { std::env::set_var("TRINO_AZAHAR_SDMC", &sd) };
}

/// Play here records and replays (issue #67): Rec restarts the scene and
/// records (two simulated players), Save writes `recordings/<scene>-1.trec`
/// (what `cargo xtask replay dump` reads), the Replay menu plays it back
/// with every hash matching, and the scrubber seeks back and forth. The Net
/// tab writes `trino.toml`'s `[players]` and `[net]` (what `cargo xtask net
/// config set` writes) and shows the command its instances run.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn play_here_records_replays_and_the_net_tab_writes_trino_toml() {
    use crate::app::{Cmd, Tab};
    use egui::accesskit::Role;
    let Some((mut h, _tmp, repo)) = temp_project_app_with(egui::vec2(1600.0, 1000.0), |_, p| {
        std::fs::write(
            p.join("trino.toml"),
            "[project]\nname = \"netty\"\ntitle = \"Netty\"\n",
        )
        .unwrap();
    }) else {
        return;
    };
    // Record with two players (the bar's combo), a few hundred frames.
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
    h.state_mut().cmds.push(Cmd::PlayHere);
    h.run_steps(4);
    h.state_mut().game_view.play.as_mut().unwrap().players.count = 2;
    h.run_steps(2);
    press(&mut h, Role::Button, "Rec");
    h.run_steps(4);
    let session = h.state().game_view.play.as_ref().unwrap();
    assert_eq!(session.players.count, 2, "Rec kept the players");
    assert!(session.recorded().is_some(), "recording");
    h.run_steps(120);
    press(&mut h, Role::Button, "Save (");
    let file = std::path::Path::new("recordings/showcase-1.trec");
    assert!(file.is_file(), "the recording on disk");
    let header = trino_core::replay::Header::parse(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(header.players, 2);
    assert!(header.frames >= 100, "{} frames", header.frames);
    // Replay it from the bar's menu: every hash matches.
    press(&mut h, Role::Button, "Replay");
    press(&mut h, Role::Button, "showcase-1.trec");
    wait_for(&mut h, "the replay to end", |a| {
        a.game_view
            .play
            .as_ref()
            .and_then(|s| s.replay.as_ref())
            .is_some_and(|r| r.pos as usize == r.frames.len())
    });
    let r = h
        .state()
        .game_view
        .play
        .as_ref()
        .unwrap()
        .replay
        .as_ref()
        .unwrap();
    assert_eq!(r.diverged, None);
    assert_eq!(r.checked, header.frames);
    assert!(h.query_by_label_contains("hashes match").is_some());
    // The scrubber: back to the start, one frame, then the end again.
    press(&mut h, Role::Button, crate::theme::icon::SKIP_BACK);
    assert_eq!(
        h.state()
            .game_view
            .play
            .as_ref()
            .unwrap()
            .replay
            .as_ref()
            .unwrap()
            .pos,
        0
    );
    press(&mut h, Role::Button, crate::theme::icon::CARET_RIGHT);
    assert_eq!(
        h.state()
            .game_view
            .play
            .as_ref()
            .unwrap()
            .replay
            .as_ref()
            .unwrap()
            .pos,
        1
    );
    h.state_mut()
        .game_view
        .play
        .as_mut()
        .unwrap()
        .seek(header.frames);
    h.run_steps(2);
    assert_eq!(
        h.state()
            .game_view
            .play
            .as_ref()
            .unwrap()
            .replay
            .as_ref()
            .unwrap()
            .diverged,
        None
    );
    h.state_mut().cmds.push(Cmd::StopPlayHere);
    h.run_steps(2);

    // The Net tab.
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Net));
    h.run_steps(4);
    assert!(
        h.query_by_label_contains("cargo xtask net local --game solid-eleven")
            .is_some()
    );
    press(&mut h, Role::RadioButton, "Delay");
    set_value(&mut h, "Local players value", 3.0);
    press(&mut h, Role::Button, "Save to trino.toml");
    let toml = std::fs::read_to_string("trino.toml").unwrap();
    let n = trino_asset_pipeline::net::ProjectNet::parse(&toml).unwrap();
    assert_eq!(n.max_players, 3, "{toml}");
    assert!(n.net.contains("mode = delay"), "{toml}");
    // An edit on disk, then Read again.
    trino_asset_pipeline::net::set_in(std::path::Path::new("."), "net.input_delay", "5").unwrap();
    press(&mut h, Role::Button, "Read again");
    assert_eq!(h.state().net.cfg.input_delay, 5);
    leave_temp_project(&repo);
}

/// The Project tab (issue #61), both ways: Add achievement and its fields
/// write `assets/achievements.ron` (what `cargo xtask services` reads);
/// the checks follow; a locked row can't be removed; an edit on disk shows
/// live; the Packaging page writes `trino.toml`'s keys in place (what
/// `cargo xtask package config set` writes) and refuses a bad one.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn project_tab_edits_achievements_and_packaging_as_text() {
    use crate::app::{Cmd, Tab};
    use egui::accesskit::Role;
    let Some((mut h, _tmp, repo)) = temp_project_app_with(egui::vec2(1600.0, 1000.0), |_, p| {
        std::fs::write(
            p.join("trino.toml"),
            "# Test project\n[project]\nname = \"t\"\ntitle = \"T\"\nplatforms = [\"pc\"]\n\n[pc]\npackage = \"t-pc\"\n",
        )
        .unwrap();
    }) else {
        return;
    };
    const FILE: &str = "assets/achievements.ron";
    let read = || {
        trino_scene::services::AchievementsDef::from_ron(&std::fs::read_to_string(FILE).unwrap())
            .unwrap()
    };
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Project));
    h.run_steps(4);
    assert!(h.query_by_label_contains("services check").is_some());
    assert_eq!(h.state().shipping.def.achievements.len(), 5, "castle64's");
    press(&mut h, Role::Button, "Add achievement");
    wait_for(&mut h, "the new achievement to be written", |a| {
        !a.shipping.dirty
    });
    let def = read();
    assert_eq!(def.achievements.len(), 6);
    assert_eq!(def.achievements[5].id, "ACHIEVEMENT_6");
    // Its keys are in no table yet: the check says so.
    assert!(
        h.state()
            .shipping
            .findings
            .iter()
            .any(|f| f.subject == "ach.achievement_6.name"),
        "{:?}",
        h.state().shipping.findings
    );
    press(&mut h, Role::CheckBox, "Hidden until unlocked");
    wait_for(&mut h, "hidden to be written", |a| !a.shipping.dirty);
    assert!(read().achievements[5].hidden);
    // A locked row stays (castle64's five are locked).
    h.state_mut().shipping.selected = Some(0);
    h.run_steps(3);
    press(&mut h, Role::Button, "Remove");
    h.run_steps(3);
    assert_eq!(read().achievements.len(), 6);
    // Edited outside: the tab follows.
    let mut outside = read();
    outside.achievements[5].id = "FROM_AN_AGENT".into();
    std::fs::write(FILE, outside.to_ron()).unwrap();
    wait_for(&mut h, "the outside edit", |a| {
        a.shipping
            .def
            .achievements
            .get(5)
            .is_some_and(|x| x.id == "FROM_AN_AGENT")
    });
    // The preview's test unlock.
    h.state_mut().shipping.selected = Some(5);
    h.run_steps(3);
    press(&mut h, Role::Button, "Test unlock");
    assert_eq!(h.state().shipping.toast.current(), Some(5));
    // Packaging: a key written in place, a bad one refused.
    h.state_mut().shipping.page = crate::shipping::Page::Packaging;
    h.run_steps(3);
    h.state_mut()
        .write_ship_key("pc.identifier", "com.example.t")
        .unwrap();
    h.state_mut().write_ship_key("steam.app_id", "480").unwrap();
    let toml = std::fs::read_to_string("trino.toml").unwrap();
    assert!(toml.starts_with("# Test project\n"), "{toml}");
    assert!(
        toml.contains("package = \"t-pc\"\nidentifier = \"com.example.t\""),
        "{toml}"
    );
    assert!(toml.contains("[steam]\napp_id = 480"), "{toml}");
    assert_eq!(h.state().shipping.ship.steam_app_id, Some(480));
    assert!(
        h.state_mut()
            .write_ship_key("pc.profile", "gameboy")
            .is_err()
    );
    leave_temp_project(&repo);
}

/// Asset groups in the editor (issue #56), both ways: a model tile dropped
/// on a group chip writes the manifest line `cargo xtask assets group`
/// writes; the scene settings then say the scene draws what it would not
/// load, until its `requires` takes the group (saved as text); a cell's
/// groups toggle in its inspector; the Memory tab measures every console
/// tier; and an agent's manifest edit shows in the chips at once.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn asset_groups_chips_requires_cells_and_memory() {
    use crate::app::{Cmd, Tab};
    use egui::accesskit::Role;
    let Some((mut h, tmp, repo)) = temp_project_app(egui::vec2(1600.0, 940.0)) else {
        return;
    };
    let manifest = tmp.path().join("assets/manifest.toml");
    // The windmill (the showcase draws it) dropped on `selftest_level`.
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Assets));
    h.state_mut().assets_filter = "windmill".into();
    h.run_steps(4);
    // The Assets tile's name (the hierarchy has a `windmill` entity too).
    let tile = assets_label(&h, "windmill").center() + egui::vec2(0.0, -40.0);
    let chip = h.get_by_label("selftest_level 3").rect().center();
    h.hover_at(tile);
    h.run_steps(2);
    h.drag_at(tile);
    h.run_steps(1);
    for i in 1..=6 {
        h.hover_at(tile + (chip - tile) * (i as f32 / 6.0));
        h.run_steps(1);
    }
    h.drop_at(chip);
    h.run_steps(4);
    let text = std::fs::read_to_string(&manifest).unwrap();
    assert!(
        manifest_table(&text, "[models.windmill]").contains("groups = [\"selftest_level\"]"),
        "{}",
        manifest_table(&text, "[models.windmill]")
    );
    assert_eq!(h.state().groups.of("models/windmill"), ["selftest_level"]);
    assert!(
        h.query_by_label("selftest_level 4").is_some(),
        "the chip counts it"
    );
    // The chip filters the grid to the group.
    h.state_mut().assets_filter.clear();
    click(&mut h, "selftest_level 4");
    assert_eq!(h.state().groups.filter.as_deref(), Some("selftest_level"));
    let in_assets = |h: &Harness<'static, App>, l: &str| {
        let groups = h.get_by_label_contains("Groups").rect();
        h.query_all_by_label(l)
            .any(|n| n.rect().min.y > groups.min.y && n.rect().min.x > groups.min.x - 12.0)
    };
    assert!(in_assets(&h, "windmill"));
    assert!(!in_assets(&h, "crate"), "boot assets are filtered out");

    // The scene settings: the showcase draws the windmill, which boot no
    // longer loads...
    h.state_mut().cmds.push(Cmd::Select(None));
    h.state_mut().inspect_focus = Some("groups".into());
    h.run_steps(4);
    assert!(
        h.query_by_label_contains("draws `models/windmill`")
            .is_some(),
        "the problem `assets check` reports"
    );
    // ...until the scene requires the group.
    press(&mut h, Role::Button, "selftest_level");
    h.run_steps(4);
    assert_eq!(h.state().doc.scene.requires, ["boot", "selftest_level"]);
    assert!(
        h.query_by_label_contains("draws `models/windmill`")
            .is_none()
    );
    h.state_mut().cmds.push(Cmd::Save);
    h.run_steps(3);
    let saved = std::fs::read_to_string(tmp.path().join("scenes/showcase.scene.ron")).unwrap();
    assert!(saved.contains("version: 8"), "migrated on save");
    assert!(
        saved.contains("requires: [\n        \"boot\",\n        \"selftest_level\",\n    ],"),
        "{}",
        &saved[..400.min(saved.len())]
    );
    h.state_mut().inspect_focus = None;

    // The Memory tab: every console tier, the scene's groups ticked.
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Memory));
    h.run_steps(3);
    wait_for(&mut h, "the memory plans", |a| a.groups.measured());
    for tier in ["N64 base", "N64 expanded", "3DS base", "3DS expanded"] {
        assert!(h.query_by_label(tier).is_some(), "{tier}");
    }
    assert!(
        h.query_all_by_label_contains(&format!("{} selftest_level", crate::theme::icon::CHECK))
            .count()
            >= 4,
        "the scene's group is ticked on every tier"
    );

    // A cell's groups, in the interior scene.
    h.state_mut()
        .cmds
        .push(Cmd::OpenScene(std::path::PathBuf::from(
            "scenes/interior.scene.ron",
        )));
    h.run_steps(4);
    let room = find(&h.state().doc.scene, "room-a");
    h.state_mut().cmds.push(Cmd::Select(Some(room.clone())));
    h.run_steps(4);
    press(&mut h, Role::Button, "selftest_level2");
    h.run_steps(3);
    let cell = h.state().doc.scene.get(&room).unwrap().vis.clone();
    assert_eq!(
        cell,
        Some(trino_scene::VisComponent::Cell {
            size: [8.0, 4.0, 8.0],
            groups: vec!["selftest_level2".into()],
        })
    );
    assert!(
        h.query_by_label("boot").is_none(),
        "boot is always loaded: no chip for a cell"
    );

    // An agent declares a group: the chips follow.
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, format!("{text}\n[groups.agent_level]\n")).unwrap();
    h.run_steps(3);
    assert!(h.state().groups.names.contains(&"agent_level".to_string()));
    leave_temp_project(&repo);
}

/// The rect of the widget labeled `label` in the Assets panel, below and
/// right of its "Groups" row (a hierarchy entity can share its name).
fn assets_label(h: &Harness<'static, App>, label: &str) -> egui::Rect {
    let groups = h.get_by_label_contains("Groups").rect();
    h.query_all_by_label(label)
        .map(|n| n.rect())
        .find(|r| r.min.y > groups.min.y && r.min.x > groups.min.x - 12.0)
        .unwrap_or_else(|| panic!("no `{label}` in Assets"))
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
            &mut h,
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

/// The Animation tab's Graph mode on the strider (issue #65): it opens
/// `anim/strider`, the parameters drive the live instance (the blend
/// space's weights, a trigger into the hop and back), a state added in the
/// panel lands in the file, an outside edit to the file reloads in the
/// panel; then the layout audit at both window sizes.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn animation_graph_mode_drives_writes_and_reloads() {
    use crate::app::{Cmd, Tab};
    for size in [egui::vec2(1600.0, 1000.0), egui::vec2(960.0, 600.0)] {
        let Some((mut h, _tmp, repo)) = temp_project_app(size) else {
            return;
        };
        h.state_mut().anim.model = Some("models/strider".into());
        h.state_mut().anim.mode = crate::anim_panel::AnimMode::Graph;
        h.state_mut().cmds.push(Cmd::FocusTab(Tab::Animation));
        h.run_steps(6);
        let g = h
            .state()
            .anim
            .graph
            .as_ref()
            .expect("the strider's graph opens");
        assert_eq!(g.name, "strider");
        assert!(g.inst.is_some(), "baked against the model: {:?}", g.issues);
        assert!(g.issues.is_empty(), "{:?}", g.issues);
        // Run halfway between walk and run: the pose blends the two.
        h.state_mut()
            .anim
            .graph
            .as_mut()
            .unwrap()
            .set_param("speed", 1.4);
        h.run_steps(3);
        let pose = h.state().anim.anim_ref().unwrap();
        let b = pose.blend.expect("a blend space mixes two clips");
        assert_eq!(pose.clip, trino_core::ClipId::from_name("walk"));
        assert!((b.weight - 0.5).abs() < 1e-3, "{b:?}");
        // The trigger takes the hop (grounded), the panel shows it live.
        click(&mut h, "Fire hop");
        h.run_steps(3);
        let hop = 1;
        assert_eq!(
            h.state().anim.graph.as_ref().unwrap().live_state(),
            Some(hop)
        );
        crate::layout_audit::assert_clean(
            &mut h,
            size,
            true,
            &format!("Animation graph at {}x{}", size.x, size.y),
        );
        if size.x < 1000.0 {
            leave_temp_project(&repo);
            continue;
        }
        // A state added in the panel is in the file.
        h.state_mut().anim.graph.as_mut().unwrap().new_name = "dance".into();
        click(&mut h, "Add a clip state");
        h.run_steps(2);
        let file = std::path::Path::new("assets/anim/strider.anim.ron");
        let text = std::fs::read_to_string(file).unwrap();
        let def = trino_scene::anim::AnimGraphDef::from_ron(&text).unwrap();
        assert!(def.state("dance").is_some(), "{text}");
        // An agent's edit to the file reloads in the panel.
        let mut edited = def.clone();
        edited.transitions.push(trino_scene::anim::TransitionDef {
            from: trino_scene::anim::From::State("dance".into()),
            to: "locomotion".into(),
            when: trino_scene::anim::CondDef::Finished,
            blend: 0.3,
        });
        edited.save(file).unwrap();
        let start = std::time::Instant::now();
        while h.state().anim.graph.as_ref().unwrap().def.transitions.len()
            != edited.transitions.len()
            && start.elapsed().as_secs() < 5
        {
            h.run_steps(1);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(h.state().anim.graph.as_ref().unwrap().def, edited);
        leave_temp_project(&repo);
    }
}

/// The Animation tab's clip metadata (issue #65): an event added at the
/// playhead and root motion switched on land in the manifest as the lines
/// `cargo xtask model inspect` and the bake read; the rebake brings them
/// back into the measurements.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn animation_clip_events_and_root_motion_write_the_manifest() {
    use crate::app::{Cmd, Tab};
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    h.state_mut().anim.model = Some("models/robot".into());
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Animation));
    h.run_steps(4);
    click(&mut h, " walk");
    click(&mut h, "Pause");
    h.state_mut().anim.time = 0.25;
    h.state_mut().anim.new_event = "sfx:jump".into();
    h.run_steps(2);
    click(&mut h, "Add at");
    h.run_steps(2);
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    assert!(
        text.contains("events.walk = [{ at = 0.25, name = \"sfx:jump\" }]"),
        "{}",
        text.lines()
            .filter(|l| l.contains("events"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    click(&mut h, "Extract at bake");
    h.run_steps(2);
    let m =
        trino_asset_pipeline::Manifest::load(std::path::Path::new("assets/manifest.toml")).unwrap();
    assert_eq!(m.models["robot"].root_motion, ["walk"]);
    // The panel shows the manifest at once.
    let asset = &h.state().project.models["models/robot"];
    assert_eq!(asset.root_motion, ["walk"]);
    assert_eq!(asset.events["walk"], [(0.25, "sfx:jump".to_string())]);
    leave_temp_project(&repo);
}

/// Toasts float over whatever panel is under them (GitHub #72). A toast
/// is laid out once at a provisional place on its first frame (egui's
/// sizing pass for a new area), and that frame's labels are in the
/// accessibility tree: a background rebake that failed under load put its
/// error toasts over the Game view's toolbar in the very frame the layout
/// audit read, and the audit called them overlapping widgets. They are on
/// a floating layer, which the audit leaves out of its overlap check (and
/// still holds to the window's edges).
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn toasts_float_over_the_layout_audit() {
    use crate::log::Level;
    // The layout audit's two sizes; 1600x1000 is the particle inspector's
    // (where CI first saw it).
    for size in [egui::vec2(960.0, 600.0), egui::vec2(1600.0, 1000.0)] {
        let Some((mut h, _tmp, repo)) = temp_project_app(size) else {
            return;
        };
        h.state_mut()
            .cmds
            .push(crate::app::Cmd::FocusTab(crate::app::Tab::Game));
        h.run_steps(3);
        for i in 0..3 {
            h.state_mut().log.toast(
                Level::Error,
                format!("rebake failed ({i}): the bake's error, in a toast"),
            );
        }
        // The toasts' first frame: what the flaky audits saw.
        h.step();
        let w = crate::layout_audit::widgets(&h, Some(&h.ctx));
        let toasts: Vec<_> = w.iter().filter(|w| w.floating).collect();
        assert!(
            toasts.iter().any(|t| t.label.contains("rebake failed")),
            "the toasts' labels are floating: {toasts:?}"
        );
        for t in &toasts {
            eprintln!("toast widget {:?} `{}` {:?}", t.role, t.label, t.rect);
        }
        // They cover panel widgets: the overlap check used to fail here.
        let solid: Vec<_> = w.iter().filter(|w| !w.floating).collect();
        let covered = toasts.iter().any(|t| {
            solid.iter().any(|s| {
                let o = s.visible.intersect(t.visible);
                o.width() > 2.0 && o.height() > 2.0
            })
        });
        assert!(covered, "the toasts cover no widget: move them in the test");
        let p = crate::layout_audit::problems(&w, size, true);
        assert!(p.is_empty(), "{p:?}");
        let shot = |what: &str| format!("toasts-{what}-{}x{}.png", size.x, size.y);
        evidence(&mut h, &shot("first-frame"));
        crate::layout_audit::assert_clean(&mut h, size, true, "toasts over the Game view");
        evidence(&mut h, &shot("settled"));
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
    // The preview ages particles by egui's frame time (a fixed 0.1 s a
    // frame here, GitHub #72): long-lived sparks, so a burst is still all
    // alive when the test counts it.
    h.state_mut()
        .particles
        .get_mut("selftest-sparks")
        .unwrap()
        .life = (60.0, 60.0);
    h.run_steps(2);
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
    crate::layout_audit::assert_clean(
        &mut h,
        egui::vec2(1600.0, 1000.0),
        true,
        "particle inspector",
    );
    leave_temp_project(&repo);
}

/// The scene's frame per console in the Inspector (scene settings): the N64
/// time with its RSP/CPU split and the reasons, the 3DS triangles, the PC
/// GPU T&L split with its reasons, and a mesh placed behind the camera
/// listed as sent for nothing — the numbers `cargo xtask scene budget`
/// prints.
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
        // The 3DS's frame (its texture memory has a line of its own).
        assert!(
            h.query_by_label_contains(&format!("3DS {} tris", b[1].tris()))
                .is_some()
        );
        // The PC native profile's GPU T&L split (issue #51), its reasons
        // listed like the RSP's.
        assert_eq!(b[2].target, trino_scene::Target::Pc);
        assert!(b[2].pc_gpu_draws > 0, "{:?}", b[2]);
        let pc_line = trino_scene::budget::pc_gpu_line(&b[2]);
        assert!(pc_line.starts_with("PC GPU "), "{pc_line}");
        assert!(h.query_by_label_contains(&pc_line).is_some(), "{pc_line}");
        if !b[2].off_pc_gpu.is_empty() {
            assert!(
                h.query_all_by_label_contains("off the PC GPU:")
                    .next()
                    .is_some()
            );
        }
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
        // A PC GPU reason opens on the entities it keeps on the CPU, and
        // one of them selects it; its Model component shows its render
        // path on each platform.
        // (The budget as it is now: the unseen rock above is one more draw.)
        let now = h.state().platform_budgets();
        if let Some((reason, n)) = now[2].off_pc_gpu.first() {
            let label = reason.label();
            let name = now[2].off_path(label)[0].to_string();
            click(&mut h, &format!("off the PC GPU: {n} × {label}"));
            // The header's opening animation moves what follows it.
            h.run_steps(12);
            let item = format!("{} {name}", crate::theme::icon::CROSSHAIR);
            click(&mut h, &item);
            h.run_steps(3);
            let path = h.state().doc.scene.find_draw_name(&name).unwrap();
            assert_eq!(h.state().doc.selected, Some(path));
            assert!(h.query_by_label_contains("Render path").is_some());
            assert!(h.query_by_label_contains("PC CPU").is_some());
            assert!(
                h.query_by_label_contains(&format!("PC: {label}")).is_some(),
                "the reason under the row"
            );
            crate::layout_audit::assert_clean(
                &mut h,
                size,
                true,
                &format!("render path at {}x{}", size.x, size.y),
            );
            h.state_mut().cmds.push(Cmd::Select(None));
            h.run_steps(4);
        }
        // An entity the GPU path takes says so.
        if let Some(gpu) = b[2]
            .paths
            .iter()
            .find(|p| p.path == trino_scene::budget::RenderPath::Hardware)
        {
            let path = h.state().doc.scene.find_draw_name(&gpu.name).unwrap();
            h.state_mut().cmds.push(Cmd::Select(Some(path)));
            h.run_steps(4);
            assert!(h.query_by_label_contains("PC GPU").is_some());
            h.state_mut().cmds.push(Cmd::Select(None));
            h.run_steps(4);
        }
        crate::layout_audit::assert_clean(
            &mut h,
            size,
            true,
            &format!("platform budget at {}x{}", size.x, size.y),
        );
        leave_temp_project(&repo);
    }
}

/// Water (ADR 0015) in the editor: the showcase river's line in the
/// platform budget (triangles, 2-cycle combiner, TMEM), "Drop a ripple" on
/// the selected river, and a water material edited and saved as text.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn water_budget_ripples_and_material_edits() {
    use crate::app::Cmd;
    let Some((mut h, tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    let root = tmp.path().to_path_buf();
    let b = h.state().budgets.clone().expect("budgets computed").1;
    // The river is drawn in pieces (#69): a line for each.
    let river = &b[0].water;
    assert!(river.len() > 1, "{river:?}");
    assert!(
        river.iter().all(|w| w.name.starts_with("river/")),
        "{river:?}"
    );
    assert!(
        river.iter().all(|w| w.cycles == 2),
        "both layers: the 2-cycle combiner"
    );
    assert!(river.iter().map(|w| w.tris).sum::<u32>() > 0);
    assert!(
        h.query_all_by_label_contains("Water river/piece-0-8")
            .next()
            .is_some()
    );
    assert!(
        h.query_all_by_label_contains("TMEM 1536/4096 B")
            .next()
            .is_some()
    );
    // Select the river's far piece: the inline material editor offers a
    // ripple.
    let path = find(&h.state().doc.scene, "piece-0-8");
    h.state_mut().cmds.push(Cmd::Select(Some(path.clone())));
    h.run_steps(3);
    assert!(h.state().ripples.is_empty());
    h.state_mut().cmds.push(Cmd::DropRipple(path));
    h.run_steps(2);
    assert_eq!(h.state().ripples.len(), 1, "a ripple rings out");
    // On the water itself, where the scene camera looks: inside one of the
    // river's triangles (the middle of its bounds is on the bank, the
    // course bends).
    let at = h.state().ripples.as_slice()[0].pos;
    let river = h.state().project.models["models/river_0_8"].tmdl.clone();
    let mesh = trino_core::render3d::Mesh::from_tmdl(&river).unwrap();
    let inside = (0..mesh.index_count / 3).any(|t| {
        let p = [0, 1, 2].map(|k| mesh.position(mesh.index(t * 3 + k) as usize));
        let side = |a: trino_core::Vec3, b: trino_core::Vec3| {
            (b.x - a.x) * (at.y - a.z) - (b.z - a.z) * (at.x - a.x)
        };
        let s = [side(p[0], p[1]), side(p[1], p[2]), side(p[2], p[0])];
        s.iter().all(|v| *v >= 0.0) || s.iter().all(|v| *v <= 0.0)
    });
    assert!(inside, "ripple at {at:?} is off the river");
    // Edit the water material as the inspector does, and save it.
    let def = h
        .state_mut()
        .lib
        .materials
        .get_mut("materials/water")
        .expect("the showcase's water material");
    def.water.as_mut().expect("a water surface").wave_amplitude = 0.2;
    h.state_mut()
        .cmds
        .push(Cmd::SaveMaterial("materials/water".into()));
    h.run_steps(6);
    let text = std::fs::read_to_string(root.join("assets/materials/water.material.ron")).unwrap();
    assert!(text.contains("wave_amplitude: 0.2"), "{text}");
    leave_temp_project(&repo);
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
            &mut h,
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
            let log: Vec<&str> = app.log.entries.iter().map(|e| e.text.as_str()).collect();
            assert!(
                app.project.models.contains_key("models/robot"),
                "the demo's assets baked: {log:#?}"
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

/// Accessibility from the editor (issue #60): the tab's checklist is the
/// CLI's, its color-vision simulation reaches the Scene, Game and UI views
/// (and View › Accessibility simulation turns it off), an edit to a
/// semantic color is written to `assets/palettes.ron` and the checklist
/// reruns, and an outside edit of the file shows live.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn accessibility_tab_palettes_simulation_and_checklist() {
    use crate::app::{Cmd, Tab};
    use trino_core::CvdMode;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Accessibility));
    h.run_steps(3);
    wait_for(&mut h, "the checklist", |a| a.access.report.is_some());
    {
        let r = h.state().access.report.as_ref().unwrap().as_ref().unwrap();
        assert!(r.findings.is_empty(), "{:#?}", r.findings);
        assert_eq!(r.colors, 7);
        assert!(r.checklist.iter().all(|l| l.passed));
    }
    assert!(h.query_by_label("castle64.door.green").is_some());
    assert!(
        h.query_all_by_label_contains("stay apart in every mode")
            .count()
            >= 2
    );

    // The simulation: the tab's mode reaches every view.
    // The simulation row's button (the colors table names the modes too).
    h.query_all_by_label("Deuteranopia")
        .next()
        .unwrap()
        .click_accesskit();
    h.run_steps(4);
    assert_eq!(h.state().access.preview.cvd, CvdMode::Deuteranopia);
    assert_eq!(h.state().scene_view.cvd_view, CvdMode::Deuteranopia);
    assert_eq!(h.state().game_view.cvd_view, CvdMode::Deuteranopia);
    assert_eq!(
        h.state().ui_screens.access.cvd,
        CvdMode::Off,
        "the UI tab picks it up when shown"
    );
    click(&mut h, "View");
    click(&mut h, "Accessibility simulation");
    click(&mut h, "Color vision: as drawn");
    h.run_steps(4);
    assert_eq!(h.state().scene_view.cvd_view, CvdMode::Off);

    // An edit that makes two teams one color for a deuteranope: written,
    // and the checklist says so.
    let before = std::fs::read_to_string("assets/palettes.ron").unwrap();
    {
        let a = &mut h.state_mut().access;
        a.palettes
            .colors
            .get_mut("selftest.team_a")
            .unwrap()
            .set_alternative(CvdMode::Deuteranopia, Some([40, 90, 220]));
        a.edited();
    }
    h.run_steps(4);
    let text = std::fs::read_to_string("assets/palettes.ron").unwrap();
    assert!(text.contains("deuteranopia: Some((40, 90, 220))"), "{text}");
    {
        let r = h.state().access.report.as_ref().unwrap().as_ref().unwrap();
        assert!(
            r.findings
                .iter()
                .any(|f| f.mode == Some("deuteranopia") && f.subject.contains("selftest.team_a")),
            "{:#?}",
            r.findings
        );
    }
    // An outside edit (an agent, a text editor) shows live.
    std::fs::write("assets/palettes.ron", &before).unwrap();
    wait_for(&mut h, "the outside edit", |a| {
        a.access.palettes.colors["selftest.team_a"].deuteranopia == Some([240, 160, 0])
    });
    h.run_steps(3);
    let r = h.state().access.report.as_ref().unwrap().as_ref().unwrap();
    assert!(r.findings.is_empty(), "{:#?}", r.findings);
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

/// Stereo 3D from the editor (issue #63): the scene's Stereo 3D section
/// (on, tuned, saved to the file, undoable), the Game view's Stereo menu
/// and 3D slider on the 3DS reaching its renderer (two eyes drawn, the
/// HUD's stereo line), and View › Convergence plane.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn stereo_section_game_view_preview_and_convergence_plane() {
    use crate::app::{Cmd, Tab};
    use trino_platform_pc::{SimProfile, StereoView};
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    assert_eq!(h.state().doc.scene.environment.stereo, None);
    h.state_mut().cmds.push(Cmd::Select(None));
    h.run_steps(3);
    click(&mut h, "Stereo 3D (3DS top screen)");
    h.run_steps(3);
    assert_eq!(
        h.state().doc.scene.environment.stereo,
        Some(trino_scene::StereoDef::default())
    );
    assert!(h.query_by_label_contains("Convergence").is_some());
    set_value(&mut h, "Stereo convergence", 7.5);
    assert_eq!(
        h.state().doc.scene.environment.stereo.unwrap().convergence,
        7.5
    );
    h.state_mut().cmds.push(Cmd::Save);
    h.run_steps(3);
    let text = std::fs::read_to_string("scenes/showcase.scene.ron").unwrap();
    assert!(text.contains("stereo: Some("), "{text}");
    assert!(text.contains("convergence: 7.5"), "{text}");
    // Undo back to no stereo block.
    h.state_mut().cmds.push(Cmd::Undo);
    h.run_steps(3);
    h.state_mut().cmds.push(Cmd::Undo);
    h.run_steps(3);
    assert_eq!(h.state().doc.scene.environment.stereo, None, "undo");

    // The Game view on the 3DS: the preview menu and the slider.
    {
        let app = h.state_mut();
        app.game_view
            .set_profile_tier(SimProfile::N3ds, trino_core::HwTier::Base, &app.project);
    }
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
    h.run_steps(3);
    assert_eq!(h.state().game_view.stats.frame.stereo.eyes, 1, "off: mono");
    pick(&mut h, "Game view stereo", StereoView::Anaglyph.label());
    h.run_steps(4);
    let gv = &h.state().game_view;
    assert_eq!(gv.stereo_view, StereoView::Anaglyph);
    assert_eq!(gv.renderer_stereo_view(), StereoView::Anaglyph);
    assert_eq!(
        gv.stats.frame.stereo.eyes, 2,
        "no stereo block: the preview's default one"
    );
    set_value(&mut h, "Game view 3D slider", 0.0);
    h.run_steps(4);
    assert_eq!(h.state().game_view.stereo_slider, 0.0);
    assert_eq!(
        h.state().game_view.stats.frame.stereo.eyes,
        1,
        "slider down"
    );
    set_value(&mut h, "Game view 3D slider", 0.5);
    h.run_steps(4);
    assert_eq!(h.state().game_view.stats.frame.stereo.slider_pct, 50);
    wait_for(&mut h, "the budgets", |a| a.budgets.is_some());
    h.run_steps(3);
    let line = {
        let app = h.state();
        app.game_view
            .stereo_hud_line(&app.budgets.as_ref().unwrap().1)
            .unwrap()
    };
    assert!(line.starts_with("stereo ×2: "), "{line}");

    // View › Convergence plane toggles the Scene view's.
    assert!(h.state().scene_view.show_convergence);
    click(&mut h, "View");
    click(&mut h, "Convergence plane");
    h.run_steps(3);
    assert!(!h.state().scene_view.show_convergence);
    leave_temp_project(&repo);
}

/// Post-processing from the editor (issue #64): Scene settings ›
/// Post-processing (on, a tonemap, a grading LUT; saved to the file,
/// undoable), the Scene view running it and its View › Post-processing view
/// modes (off, the bloom mask), an entity's Glow component from Add
/// component, the Game view on the N64 drawing the console's graded
/// models instead of a post pass, and the Accessibility tab's Daltonize
/// preview reaching both views' renderers.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn post_section_view_modes_glow_and_the_correction_preview() {
    use crate::app::{Cmd, Tab};
    use trino_platform_pc::{PostView, SimProfile};
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    assert_eq!(h.state().doc.scene.environment.post, None);
    h.state_mut().cmds.push(Cmd::Select(None));
    h.run_steps(3);
    click(&mut h, "Post stack (PC and Web)");
    h.run_steps(3);
    assert_eq!(
        h.state().doc.scene.environment.post,
        Some(trino_scene::PostDef::default())
    );
    pick(&mut h, "Post tonemap", "AgX");
    pick(&mut h, "Post grading", "luts/dusk");
    set_value(&mut h, "Post exposure", 1.5);
    h.run_steps(4);
    let post = h.state().doc.scene.environment.post.clone().unwrap();
    assert_eq!(post.tonemap, trino_scene::TonemapDef::AgX);
    assert_eq!(post.grading.as_deref(), Some("luts/dusk"));
    assert_eq!(post.exposure, 1.5);
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Scene));
    h.run_steps(4);
    assert!(
        h.state().scene_view.stats.post,
        "the Scene view runs the stack"
    );
    h.state_mut().cmds.push(Cmd::Save);
    h.run_steps(3);
    let text = std::fs::read_to_string("scenes/showcase.scene.ron").unwrap();
    assert!(text.contains("post: Some("), "{text}");
    assert!(text.contains("grading: Some(\"luts/dusk\")"), "{text}");

    // View › Post-processing view: off, then the bloom alone.
    click(&mut h, "View");
    click(&mut h, "Post-processing view");
    click(&mut h, "Post: off");
    h.run_steps(4);
    assert_eq!(h.state().scene_view.post_view, PostView::Off);
    assert!(!h.state().scene_view.stats.post);
    h.state_mut().scene_view.post_view = PostView::Bloom;
    h.run_steps(4);
    assert!(h.state().scene_view.stats.post);
    h.state_mut().scene_view.post_view = PostView::Game;

    // A Glow component from Add component.
    h.state_mut().cmds.push(Cmd::Select(Some(vec![0])));
    h.run_steps(3);
    click(&mut h, "Add component");
    click(&mut h, "Glow");
    h.run_steps(3);
    let glow = h.state().doc.scene.entities[0].glow.clone();
    assert_eq!(glow, Some(trino_scene::GlowComponent::default()));

    // The N64's Game view: no post pass (the console bakes its grading).
    {
        let app = h.state_mut();
        app.game_view
            .set_profile_tier(SimProfile::N64, trino_core::HwTier::Base, &app.project);
    }
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
    h.run_steps(4);
    assert!(!h.state().game_view.stats.post);

    // Undo back to no post block.
    for _ in 0..8 {
        h.state_mut().cmds.push(Cmd::Undo);
        h.run_steps(2);
    }
    assert_eq!(h.state().doc.scene.environment.post, None, "undo");

    // Accessibility › Correction filter: Daltonize reaches both views.
    h.state_mut().access.preview.cvd = trino_core::CvdMode::Deuteranopia;
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Accessibility));
    h.run_steps(3);
    click(&mut h, "Daltonize");
    h.run_steps(4);
    assert!(h.state().access.preview.cvd_filter);
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Scene));
    h.run_steps(4);
    assert_eq!(
        h.state().scene_view.renderer_cvd_filter(),
        trino_core::CvdMode::Deuteranopia
    );
    leave_temp_project(&repo);
}

/// Shadows from the editor (ADR 0025): the scene's Shadows section (on,
/// saved to the file, undoable), an entity's Shadow component from Add
/// component (a model casts, a bare collider receives), the Scene view
/// drawing them at each previewed profile's tier (the map on the PC, blobs
/// on the N64, projected shapes on the 3DS), the Scene view's modes from
/// View › Shadow view (off; debug, even with the scene's off), and each
/// tier's line in the section.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn shadows_panel_components_tiers_and_view_modes() {
    use crate::app::Cmd;
    use crate::theme::icon;
    use trino_core::{HwTier, ShadowTier};
    use trino_platform_pc::{ShadowView, SimProfile};
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    assert_eq!(h.state().doc.scene.environment.shadows, None);
    // The scene's shadows on.
    h.state_mut().cmds.push(Cmd::Select(None));
    h.run_steps(3);
    click(&mut h, "Shadows (every platform)");
    h.run_steps(3);
    assert_eq!(
        h.state().doc.scene.environment.shadows,
        Some(trino_scene::ShadowsDef::default())
    );
    assert!(h.query_by_label_contains("Best tier").is_some());

    // A floor (a bare collider) under the rock: Add component › Shadow
    // makes it a receiver, the rock a caster.
    let add_shadow = |h: &mut Harness<'static, App>| {
        click(h, "Add component");
        h.get_by_label(&format!("{}  Shadow", icon::SUN_HORIZON))
            .click_accesskit();
        h.run_steps(3);
    };
    let mut floor = trino_scene::Entity::new("shadow-floor");
    floor.transform.pos = [5.0, 0.0, 8.0];
    floor.collider = Some(trino_scene::ColliderComponent {
        shape: trino_scene::ShapeDef::Box {
            size: [8.0, 0.1, 8.0],
        },
        ..Default::default()
    });
    h.state_mut().cmds.push(Cmd::Create {
        parent: None,
        entity: Box::new(floor),
    });
    h.run_steps(3);
    add_shadow(&mut h);
    let floor = find(&h.state().doc.scene, "shadow-floor");
    let shadow = h.state().doc.scene.get(&floor).unwrap().shadow;
    assert_eq!(
        shadow,
        Some(trino_scene::ShadowComponent {
            cast: trino_scene::ShadowCastDef::None,
            receive: true,
            ..Default::default()
        }),
        "a bare collider receives"
    );
    let rock = find(&h.state().doc.scene, "rock-1");
    h.state_mut().cmds.push(Cmd::Select(Some(rock.clone())));
    h.run_steps(3);
    add_shadow(&mut h);
    let shadow = h.state().doc.scene.get(&rock).unwrap().shadow;
    assert_eq!(shadow, Some(trino_scene::ShadowComponent::default()));
    assert!(h.query_by_label_contains("N64: blob 0.50 m").is_some());
    assert_eq!(h.state().resolved.receivers().len(), 1);

    // The Scene view, looking at the rock, at each profile's tier.
    h.state_mut().scene_view.cam.look_from(
        trino_core::Vec3::new(5.0, 4.0, 14.0),
        trino_core::Vec3::new(5.0, 0.3, 8.0),
    );
    let preview = |h: &mut Harness<'static, App>, profile| {
        {
            let app = h.state_mut();
            app.scene_view
                .set_profile_tier(profile, HwTier::Base, &app.project);
        }
        h.run_steps(4);
        let s = &h.state().scene_view.stats;
        (s.shadow_tier, s.frame.shadows)
    };
    let (tier, s) = preview(&mut h, SimProfile::Native);
    assert_eq!(tier, ShadowTier::Map);
    assert!(s.mapped >= 1, "{s:?}");
    let (tier, s) = preview(&mut h, SimProfile::N64);
    assert_eq!(tier, ShadowTier::Blob);
    assert!(s.blobs >= 1 && s.mapped == 0, "{s:?}");
    let (tier, s) = preview(&mut h, SimProfile::N3ds);
    assert_eq!(tier, ShadowTier::Planar);
    assert!(s.planar >= 1 && s.blobs == 0, "{s:?}");
    preview(&mut h, SimProfile::Native);

    // Saved as text.
    h.state_mut().cmds.push(Cmd::Save);
    h.run_steps(3);
    let text = std::fs::read_to_string("scenes/showcase.scene.ron").unwrap();
    assert!(text.contains("shadows: Some("), "{text}");
    assert!(text.contains("shadow: Some("), "the components saved");
    assert!(text.contains("receive: true"), "the floor saved");

    // View › Shadow view: off whatever the scene says, then the debug
    // color even with the scene's off.
    click(&mut h, "View");
    click(&mut h, "Shadow view");
    click(&mut h, "Shadows: off");
    h.run_steps(4);
    assert_eq!(h.state().scene_view.shadow_view, ShadowView::Off);
    assert_eq!(h.state().scene_view.stats.shadow_tier, ShadowTier::None);
    h.state_mut().cmds.push(Cmd::Select(None));
    h.run_steps(3);
    // The save's toast can cover the Inspector: click through accesskit.
    h.get_by_label("Shadows (every platform)").click_accesskit();
    h.run_steps(3);
    assert!(!h.state().doc.scene.environment.shadows.unwrap().enabled);
    h.state_mut().scene_view.shadow_view = ShadowView::Debug;
    h.run_steps(4);
    let stats = &h.state().scene_view.stats;
    assert_eq!(
        stats.shadow_tier,
        ShadowTier::Map,
        "a mode change re-renders"
    );
    assert!(stats.frame.shadows.mapped >= 1);
    h.state_mut().cmds.push(Cmd::Undo);
    h.run_steps(4);
    assert!(
        h.state().doc.scene.environment.shadows.unwrap().enabled,
        "undo"
    );
    // Each tier's line under the section.
    wait_for(&mut h, "the budgets", |a| a.budgets.is_some());
    for line in [
        "N64 shadows: blob",
        "3DS shadows: planar",
        "PC shadows: map",
    ] {
        assert!(h.query_by_label_contains(line).is_some(), "{line}");
    }
    leave_temp_project(&repo);
}

/// The manifest table `[kind.name]` of `text`, up to the next table.
fn manifest_table<'a>(text: &'a str, header: &str) -> &'a str {
    text.split(header)
        .nth(1)
        .and_then(|t| t.split("\n[").next())
        .unwrap()
}

/// Close a combo's list left open by an accessibility click: a click on
/// the grid's "Memory" label, which no list covers (one on the combo could
/// land on the list's first item instead).
fn close_popup(h: &mut Harness<'static, App>) {
    h.get_by_label("Memory").click();
    h.run_steps(3);
}

/// Step until `done` (a worker finishing, the watcher rebaking), 30 s at
/// most.
fn wait_for(h: &mut Harness<'static, App>, what: &str, done: impl Fn(&App) -> bool) {
    let start = std::time::Instant::now();
    while !done(h.state()) {
        assert!(
            start.elapsed().as_secs() < 30,
            "timed out waiting for {what}"
        );
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    h.run_steps(2);
}

/// Assets › Sprite › Texture formats (#52): the inspector measures the
/// photo card on every platform; picking a 3DS format and a quality in
/// its combos writes the manifest and the columns follow; a `min_psnr`
/// the declared ETC1 misses is a bake error shown in the panel and the
/// Console, never a crash; the budget gets the project's texture memory.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn texture_inspector_formats_quality_and_errors() {
    use crate::app::{AssetSel, Cmd};
    use trino_asset_pipeline::Platform;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    const PHOTO: &str = "sprites/selftest_photo";
    const TABLE: &str = "[sprites.selftest_photo]";
    let report = |app: &App, p: Platform| {
        let (_, insp, _) = app.textures.current.as_ref()?;
        let col = insp.columns.iter().find(|c| c.platform == p)?;
        Some(col.report.clone())
    };
    let n3ds_format = |app: &App| {
        report(app, Platform::N3ds)
            .and_then(|r| r.ok())
            .map(|r| r.format)
    };
    h.state_mut()
        .cmds
        .push(Cmd::SelectAsset(AssetSel::Sprite(PHOTO.into())));
    wait_for(&mut h, "the first measurement", |a| {
        n3ds_format(a).is_some()
    });
    assert_eq!(n3ds_format(h.state()).as_deref(), Some("RGBA8"), "declared");
    assert!(h.query_by_label_contains("Texture formats").is_some());

    // 3DS format: ETC1 from its combo.
    click(&mut h, "3DS format");
    h.get_by_label("ETC1").click_accesskit();
    h.run_steps(3);
    close_popup(&mut h);
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    let table = manifest_table(&text, TABLE);
    assert!(table.contains(r#"3ds = "ETC1""#), "{table}");
    assert!(
        table.contains(r#"n64 = "CI8""#),
        "other platforms kept: {table}"
    );
    wait_for(&mut h, "the 3DS column to follow", |a| {
        n3ds_format(a).as_deref() == Some("ETC1")
    });
    let etc1 = report(h.state(), Platform::N3ds).unwrap().unwrap();
    assert!(etc1.psnr.unwrap() < 35.0, "ETC1 loses detail on the photo");
    let pc_before = report(h.state(), Platform::Pc).unwrap().unwrap();
    assert!(pc_before.auto);
    // The Web's column (issue #47): `auto` measured KTX2 through Binomial's
    // decode; its bytes are the download, its memory the VRAM.
    let web = report(h.state(), Platform::Web).unwrap().unwrap();
    assert!(web.auto, "{web:?}");
    assert!(
        web.candidates.iter().any(|c| c.format == "KTX2-ETC1S"),
        "{web:?}"
    );
    assert!(
        h.query_by_label("Web format").is_some(),
        "a format combo per platform"
    );
    // KTX2-UASTC from its combo: written as `web = ...`, measured again.
    click(&mut h, "Web format");
    h.get_by_label("KTX2-UASTC").click_accesskit();
    h.run_steps(3);
    close_popup(&mut h);
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    assert!(
        manifest_table(&text, TABLE).contains(r#"web = "KTX2-UASTC""#),
        "{text}"
    );
    wait_for(&mut h, "the Web column to follow", |a| {
        report(a, Platform::Web)
            .and_then(|r| r.ok())
            .is_some_and(|r| r.format == "KTX2-UASTC" && !r.auto)
    });
    h.run_steps(2);
    // The grid has a Web column too (it once stopped at the 3DS).
    assert!(
        h.query_by_label("KTX2-UASTC").is_some(),
        "the Web column's Uses cell"
    );

    // Quality: high, from its combo; `auto` on the PC must keep 40 dB.
    click(&mut h, "Texture quality");
    h.get_by_label("high (40 dB)").click_accesskit();
    h.run_steps(3);
    close_popup(&mut h);
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    assert!(manifest_table(&text, TABLE).contains(r#"quality = "high""#));
    wait_for(&mut h, "the quality to apply", |a| {
        a.textures
            .current
            .as_ref()
            .is_some_and(|(_, i, _)| i.quality.as_deref() == Some("high"))
    });
    let pc = report(h.state(), Platform::Pc).unwrap().unwrap();
    assert!(pc.psnr.unwrap() >= 40.0, "{pc:?}");
    assert_eq!(pc.threshold, Some(40.0));

    // A min_psnr the declared ETC1 misses: the bake's error, in the panel
    // and the Console.
    h.state_mut().cmds.push(Cmd::SetTextureKey(
        PHOTO.into(),
        "min_psnr",
        Some("40".into()),
    ));
    wait_for(&mut h, "the bake error", |a| {
        matches!(report(a, Platform::N3ds), Some(Err(_)))
    });
    let Some(Err(e)) = report(h.state(), Platform::N3ds) else {
        unreachable!()
    };
    assert!(e.contains("min_psnr 40"), "{e}");
    assert!(h.query_by_label_contains("bake fails").is_some());
    assert!(
        h.state().log.entries.iter().any(|l| l.text == e),
        "the error reached the Console"
    );
    // Back to the default format: the entry leaves the manifest, and the
    // 3DS goes back to `auto`, which passes.
    h.state_mut()
        .cmds
        .push(Cmd::SetTextureFormat(PHOTO.into(), Platform::N3ds, None));
    wait_for(&mut h, "auto on the 3DS", |a| {
        report(a, Platform::N3ds).is_some_and(|r| r.is_ok_and(|r| r.auto))
    });
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    assert!(!manifest_table(&text, TABLE).contains("3ds ="));

    // The scene settings' budget: the project's texture memory.
    h.state_mut().cmds.push(Cmd::Select(None));
    h.state_mut().asset_sel = None;
    h.run_steps(3);
    wait_for(&mut h, "the texture totals", |a| {
        a.textures.totals.is_some()
    });
    assert!(h.query_by_label_contains("Textures PC").is_some());
    assert!(h.query_by_label_contains("Textures 3DS").is_some());
    assert!(h.query_by_label_contains("N64 largest TMEM load").is_some());
    leave_temp_project(&repo);
}

/// Press the widget with `role` and `label` (exactly, else the one whose
/// label contains it: icon glyphs prefix button texts) through
/// accessibility: no pointer, so a widget scrolled out of view, or one
/// sharing its label with its row's caption ("Stream" the label and the
/// checkbox), works.
fn press(h: &mut Harness<'static, App>, role: egui::accesskit::Role, label: &str) {
    use egui_kittest::kittest::By;
    match h.query_by_role_and_label(role, label) {
        Some(n) => n.click_accesskit(),
        None => h
            .get(By::new().role(role).label_contains(label))
            .click_accesskit(),
    }
    h.run_steps(3);
}

/// Pick `item` in the combo labeled `combo`, then close its list.
fn pick(h: &mut Harness<'static, App>, combo: &str, item: &str) {
    use egui::accesskit::Role;
    press(h, Role::ComboBox, combo);
    press(h, Role::Button, item);
    egui::Popup::close_all(&h.ctx);
    h.run_steps(3);
}

/// Set a slider's or a drag value's number, as a screen reader does.
fn set_value(h: &mut Harness<'static, App>, label: &str, value: f64) {
    use egui::accesskit::{Action, ActionData, ActionRequest};
    use egui_kittest::kittest::NodeT;
    let (target_node, target_tree) = h.get_by_label(label).accesskit_node().locate();
    h.event(egui::Event::AccessKitActionRequest(ActionRequest {
        action: Action::SetValue,
        target_node,
        target_tree,
        data: Some(ActionData::NumericValue(value)),
    }));
    h.run_steps(3);
}

/// Assets › Audio and the Mixer tab (#53), both ways: the inspector
/// measures the theme on every platform; its combos and checkboxes write
/// the manifest (comments kept) and the columns follow; Play decodes on
/// the PC's output; a bus slider and duck-rule edits write
/// `assets/audio.toml`, which parses; the file edited outside shows up,
/// an invalid one is an error, not a crash; the budget gets audio lines.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn audio_inspector_and_mixer_write_the_text_files() {
    use crate::app::{AssetSel, Cmd, Tab};
    use egui::accesskit::Role;
    use trino_asset_pipeline::Platform;
    use trino_asset_pipeline::audio::{Codec, MixFile};
    use trino_core::Bus;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    const THEME: &str = "music/theme";
    const TABLE: &str = "[music.theme]";
    let n64 = |app: &App| {
        let (_, insp) = app.audio.current.as_ref()?;
        let col = insp.columns.iter().find(|c| c.platform == Platform::N64)?;
        col.result.as_ref().ok().map(|m| (m.settings.codec, m.rate))
    };
    h.state_mut()
        .cmds
        .push(Cmd::SelectAsset(AssetSel::Audio(THEME.into())));
    wait_for(&mut h, "the first measurement", |a| n64(a).is_some());
    assert_eq!(n64(h.state()).unwrap().0, Codec::Vadpcm, "auto on the N64");
    for label in ["Waveform", "Bake settings", "Platforms", "Play master"] {
        assert!(h.query_by_label_contains(label).is_some(), "{label}");
    }
    for p in ["PC", "N64", "3DS", "Web"] {
        assert!(
            h.query_by_role_and_label(Role::ComboBox, &format!("{p} codec"))
                .is_some(),
            "a codec combo per platform ({p})"
        );
    }

    // N64 codec: PCM from its combo; the other platforms keep auto.
    pick(&mut h, "N64 codec", "pcm");
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    let table = manifest_table(&text, TABLE);
    assert!(table.contains(r#"codec = { n64 = "pcm" }"#), "{table}");
    assert!(
        text.contains("# Music streams by default"),
        "comments kept: {text}"
    );
    wait_for(&mut h, "the N64 column to follow", |a| {
        n64(a).is_some_and(|(c, _)| c == Codec::Pcm)
    });
    // N64 rate: 16 kHz.
    pick(&mut h, "N64 rate", "16.0 kHz");
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    let table = manifest_table(&text, TABLE);
    assert!(table.contains("sample_rate = { n64 = 16000 }"), "{table}");
    wait_for(&mut h, "the N64 rate to follow", |a| {
        n64(a) == Some((Codec::Pcm, 16_000))
    });
    // Stream off (music streams by default), then the bus.
    press(&mut h, Role::CheckBox, "Stream");
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    assert!(
        manifest_table(&text, TABLE).contains("stream = false"),
        "{text}"
    );
    pick(&mut h, "Bus", "ambience");
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    assert!(
        manifest_table(&text, TABLE).contains(r#"bus = "ambience""#),
        "{text}"
    );
    wait_for(&mut h, "the settings to apply", |a| {
        a.audio
            .current
            .as_ref()
            .is_some_and(|(_, i)| i.stream == Some(false) && i.bus.as_deref() == Some("ambience"))
    });

    // Play the N64's decode on the PC's output (silent without a device),
    // then stop it.
    press(&mut h, Role::Button, "Play N64");
    let playing = |h: &mut Harness<'static, App>| {
        h.state_mut()
            .audio
            .player
            .as_mut()
            .and_then(|p| p.playing().cloned())
    };
    assert_eq!(
        playing(&mut h),
        Some((THEME.to_string(), Some(Platform::N64)))
    );
    press(&mut h, Role::Button, "Stop");
    assert_eq!(playing(&mut h), None);

    // The Mixer: a bus slider, a duck rule added, edited and another removed.
    let mix = || MixFile::parse(&std::fs::read_to_string("assets/audio.toml").unwrap()).unwrap();
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Mixer));
    h.run_steps(3);
    assert!(h.query_by_label_contains("Project mix").is_some());
    set_value(&mut h, "music volume", 50.0);
    wait_for(&mut h, "the mix to be written", |a| !a.mixer.dirty);
    assert_eq!(mix().buses.get("music"), Some(&0.5));
    assert_eq!(mix().buses.get("ambience"), Some(&0.9), "the rest kept");
    press(&mut h, Role::Button, "Add duck rule");
    let rules = |m: &MixFile| -> Vec<(String, String, f32)> {
        m.duck
            .iter()
            .map(|d| (d.trigger.clone(), d.target.clone(), d.level))
            .collect()
    };
    assert_eq!(
        rules(&h.state().mixer.file)[1],
        ("voice".into(), "ambience".into(), 0.4),
        "a new rule takes a free pair"
    );
    pick(&mut h, "Duck 2 target", "sfx");
    set_value(&mut h, "Duck 2 level", 25.0);
    press(&mut h, Role::Button, "Remove duck 1");
    wait_for(&mut h, "the duck rules to be written", |a| !a.mixer.dirty);
    assert_eq!(
        rules(&mix()),
        vec![("voice".into(), "sfx".into(), 0.25)],
        "{}",
        std::fs::read_to_string("assets/audio.toml").unwrap()
    );
    assert!(h.state().mixer.error.is_none());

    // Edited outside: the tab follows; an invalid mix is shown, not loaded.
    std::fs::write("assets/audio.toml", "version = 1\n\n[buses]\nmusic = 0.3\n").unwrap();
    wait_for(&mut h, "the outside edit", |a| {
        a.mixer.volume(Bus::Music) == 0.3
    });
    assert!(h.state().mixer.file.duck.is_empty());
    // Shown, not written back (a slider rounding what it shows once read
    // as an edit every frame, and the editor rewrote the file).
    h.run_steps(20);
    assert_eq!(
        std::fs::read_to_string("assets/audio.toml").unwrap(),
        "version = 1\n\n[buses]\nmusic = 0.3\n"
    );
    std::fs::write("assets/audio.toml", "version = 1\n\n[buses]\nradio = 0.5\n").unwrap();
    wait_for(&mut h, "the invalid mix", |a| a.mixer.error.is_some());
    let e = h.state().mixer.error.clone().unwrap();
    assert!(e.contains("unknown bus `radio`"), "{e}");
    assert_eq!(h.state().mixer.volume(Bus::Music), 0.3, "the last good mix");
    assert!(
        h.query_all_by_label_contains("unknown bus `radio`")
            .next()
            .is_some(),
        "shown in the tab"
    );

    // The budget: the Game view's HUD and the scene settings' lines.
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
    h.run_steps(3);
    wait_for(&mut h, "the audio totals", |a| a.audio.totals.is_some());
    let (_, totals) = h.state().audio.totals.as_ref().unwrap();
    assert!(totals.sounds >= 10, "{}", totals.sounds);
    h.state_mut().cmds.push(Cmd::Select(None));
    h.state_mut().asset_sel = None;
    h.run_steps(4);
    for p in ["PC", "N64", "3DS", "Web"] {
        assert!(
            h.query_by_label_contains(&format!("Audio {p}")).is_some(),
            "the {p} budget line"
        );
    }
    leave_temp_project(&repo);
}

/// The Quality tab (#55), both ways: a cell edit writes
/// `assets/quality.ron` (what `cargo xtask quality show` reads), Reset
/// takes a preset back to the engine's values; the file edited outside
/// shows up, an invalid one is an error, not a crash, and is never
/// written back; the Game view's PC profile previews a preset from its
/// toolbar (or a column's eye) with the 3D at the preset's scale.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn quality_tab_writes_the_presets_and_the_game_view_previews_them() {
    use crate::app::{Cmd, Tab};
    use egui::accesskit::Role;
    use trino_asset_pipeline::quality::QualityFile;
    use trino_core::{QualityPreset, QualitySettings};
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    const FILE: &str = "assets/quality.ron";
    let table = || {
        QualityFile::parse(&std::fs::read_to_string(FILE).unwrap())
            .and_then(|f| f.table())
            .unwrap()
    };
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Quality));
    h.run_steps(3);
    assert!(h.query_by_label_contains("Quality presets").is_some());
    assert!(h.query_by_label_contains("an edit creates it").is_some());
    for p in ["low", "medium", "high", "ultra"] {
        for row in ["msaa", "ao", "frame limit"] {
            assert!(
                h.query_by_role_and_label(Role::ComboBox, &format!("{p} {row}"))
                    .is_some(),
                "{p} {row}"
            );
        }
    }

    // Low's render scale, High's MSAA: written; the rest stays built in.
    set_value(&mut h, "low render scale", 0.6);
    wait_for(&mut h, "the presets to be written", |a| {
        a.quality.exists && !a.quality.dirty
    });
    assert_eq!(table().get(QualityPreset::Low).render_scale, 0.6);
    pick(&mut h, "high msaa", "off");
    wait_for(&mut h, "High's MSAA to be written", |a| !a.quality.dirty);
    assert_eq!(table().get(QualityPreset::High).msaa, 1);
    assert_eq!(table().get(QualityPreset::Medium), QualitySettings::MEDIUM);
    press(&mut h, Role::Button, "Reset high");
    wait_for(&mut h, "High's reset to be written", |a| !a.quality.dirty);
    assert_eq!(table().get(QualityPreset::High), QualitySettings::HIGH);
    assert!(
        !std::fs::read_to_string(FILE).unwrap().contains("high: ("),
        "an unchanged preset isn't written"
    );

    // Edited outside: the tab follows, and doesn't write it back.
    let outside = "(version: 1, medium: (lod_bias: 0.7))\n";
    std::fs::write(FILE, outside).unwrap();
    wait_for(&mut h, "the outside edit", |a| {
        a.quality.get(QualityPreset::Medium).lod_bias == 0.7
    });
    assert_eq!(
        h.state().quality.get(QualityPreset::Low),
        QualitySettings::LOW
    );
    h.run_steps(20);
    assert_eq!(std::fs::read_to_string(FILE).unwrap(), outside);
    // Invalid: shown in the tab, the last good table kept.
    std::fs::write(FILE, "(version: 1, low: (msaa: 3))\n").unwrap();
    wait_for(&mut h, "the invalid presets", |a| a.quality.error.is_some());
    let e = h.state().quality.error.clone().unwrap();
    assert!(e.contains("msaa 3"), "{e}");
    assert!(h.query_all_by_label_contains("msaa 3").next().is_some());
    assert_eq!(h.state().quality.get(QualityPreset::Medium).lod_bias, 0.7);
    std::fs::write(FILE, "(version: 1, low: (render_scale: 0.6))\n").unwrap();
    wait_for(&mut h, "the fixed presets", |a| {
        a.quality.error.is_none() && a.quality.get(QualityPreset::Low).render_scale == 0.6
    });

    // The Game view on the PC: Low from its toolbar draws the 3D at Low's
    // scale (dynamic resolution as Low says, then toggled off).
    {
        let app = h.state_mut();
        app.game_view.set_profile_tier(
            trino_platform_pc::SimProfile::Native,
            trino_core::HwTier::Base,
            &app.project,
        );
    }
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
    h.run_steps(3);
    pick(&mut h, "Game view quality", "Low");
    assert_eq!(h.state().game_view.quality, QualityPreset::Low);
    assert!(h.state().game_view.dynres, "Low's dynamic resolution");
    press(&mut h, Role::CheckBox, "Dynamic resolution");
    h.run_steps(3);
    let gv = &h.state().game_view;
    assert!(!gv.dynres);
    assert_eq!(gv.render_scale(), 0.6, "the project's Low");
    let q = gv.quality_settings().unwrap();
    assert_eq!((q.preset, q.dynamic_resolution), (QualityPreset::Low, None));
    // Ultra from the Quality tab's eye: supersampled, Ultra's LOD bias.
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Quality));
    h.run_steps(3);
    press(&mut h, Role::CheckBox, "Preview ultra");
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
    h.run_steps(4);
    let gv = &h.state().game_view;
    assert_eq!(gv.quality, QualityPreset::Ultra);
    assert_eq!(gv.render_scale(), 1.25);
    assert_eq!(gv.quality_settings().unwrap().lod_bias, 1.5);
    leave_temp_project(&repo);
}

/// A language longer than English, for the UI tab's overflow highlight.
const PT_BR: &str = r#"Strings(
    lang: "pt-BR",
    fallback: "en-US",
    entries: {
        "pause.title": "Pausado",
        "pause.resume": "Continuar",
        "pause.options": "Opcoes",
        "pause.quit": "Voltar para a tela inicial",
    },
)
"#;

/// The UI tab (#58), both ways: `TRINO_EDITOR_UI` opens it on a screen; a
/// screen picked, a node selected in the tree and its text edited → the
/// file on disk (its comments kept); the file edited outside → the tab
/// (not written back); a node added into the menu, duplicated, deleted,
/// dragged before a sibling and moved up; on the 3DS the menu is on the
/// touch screen; a language whose text is too long is highlighted and
/// listed; clicks on the preview and on an issue select the node.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn ui_tab_edits_screens_and_follows_their_files() {
    use egui::accesskit::Role;
    use egui::{Event, PointerButton};
    use trino_core::Screen;
    use trino_scene::ui::ScreenDef;
    let setup = |repo: &std::path::Path, dir: &std::path::Path| {
        for name in ["title", "pause", "options"] {
            let text = std::fs::read_to_string(
                repo.join(format!("templates/standalone/ui/{name}.ui.ron")),
            )
            .unwrap();
            let mut text = text.replace("{{name}}", "orbit-cat");
            if name == "pause" {
                // A fixed box for Quit, so pt-BR's longer text overflows it
                // (the starter screens grow with their text since #60).
                // The menu as this test was written against: Resume, Options,
                // Quit (the Achievements button, #61, left out).
                text = text
                    .lines()
                    .filter(|l| !l.contains("id: \"achievements\""))
                    .map(|l| {
                        if l.contains("id: \"quit\"") {
                            l.replace(
                                "min: (128, 18), padding: (6, 3, 6, 3)",
                                "size: (Px(128), Px(18))",
                            )
                        } else {
                            l.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            std::fs::write(dir.join(format!("ui/{name}.ui.ron")), text).unwrap();
        }
        std::fs::create_dir_all(dir.join("assets/i18n")).unwrap();
        std::fs::copy(
            repo.join("templates/standalone/assets/i18n/en-US.ron"),
            dir.join("assets/i18n/en-US.ron"),
        )
        .unwrap();
        std::fs::write(dir.join("assets/i18n/pt-BR.ron"), PT_BR).unwrap();
    };
    // SAFETY: --ignored --test-threads=1 only (module docs).
    unsafe { std::env::set_var("TRINO_EDITOR_UI", "options,n64") };
    let started = temp_project_app_with(egui::vec2(1600.0, 1000.0), setup);
    // SAFETY: as above.
    unsafe { std::env::remove_var("TRINO_EDITOR_UI") };
    let Some((mut h, _tmp, repo)) = started else {
        return;
    };
    const PAUSE: &str = "ui/pause.ui.ron";
    let disk = || ScreenDef::from_ron(&std::fs::read_to_string(PAUSE).unwrap()).unwrap();
    let ids = |d: &ScreenDef| -> Vec<String> {
        let stack = d.root.at(&[1, 0]).unwrap();
        stack.children.iter().map(|c| c.id.clone()).collect()
    };
    let node = |a: &App, path: &[usize]| a.ui_screens.def.as_ref()?.root.at(path).cloned();

    // The hook: the UI tab, on Options, the N64's preview.
    h.run_steps(3);
    assert!(
        h.query_by_label("UI screen").is_some(),
        "the UI tab is in front"
    );
    assert_eq!(h.state().ui_screens.name(), "options");
    assert_eq!(
        h.state().ui_screens.preview.as_ref().unwrap().size,
        (320, 240)
    );

    // Pause, its Resume button selected in the tree, its text edited.
    pick(&mut h, "UI screen", "pause");
    assert_eq!(h.state().ui_screens.name(), "pause");
    click(&mut h, "Button \"resume\"");
    assert_eq!(h.state().ui_screens.selected, Some(vec![1, 0, 1]));
    // The pad's order from the first focus: Resume, Options, Quit.
    let order = &h.state().ui_screens.preview.as_ref().unwrap().order;
    assert_eq!(order.len(), 3, "{order:?}");
    h.get_by_label("Node text").focus();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(Event::Text("Continue".into()));
    h.key_press(egui::Key::Enter);
    wait_for(&mut h, "the edit to be written", |a| {
        !a.ui_screens.dirty && node(a, &[1, 0, 1]).is_some_and(|n| n.text == "Continue")
    });
    let text = std::fs::read_to_string(PAUSE).unwrap();
    assert!(
        text.starts_with("// The pause menu"),
        "comments kept:\n{text}"
    );
    assert_eq!(disk().root.at(&[1, 0, 1]).unwrap().text, "Continue");

    // Edited outside: the tab follows (the selection kept) and doesn't
    // write it back.
    let outside = text.replace("Quit to title", "Leave");
    std::fs::write(PAUSE, &outside).unwrap();
    wait_for(&mut h, "the outside edit", |a| {
        node(a, &[1, 0, 3]).is_some_and(|n| n.text == "Leave")
    });
    assert_eq!(h.state().ui_screens.selected, Some(vec![1, 0, 1]));
    h.run_steps(20);
    assert_eq!(std::fs::read_to_string(PAUSE).unwrap(), outside);

    // A text added into the menu's stack, Options duplicated and deleted.
    click(&mut h, "Stack");
    press(&mut h, Role::Button, "Add node");
    press(&mut h, Role::Button, "Add Text");
    egui::Popup::close_all(&h.ctx);
    h.run_steps(3);
    assert_eq!(h.state().ui_screens.selected, Some(vec![1, 0, 4]));
    click(&mut h, "Button \"options\"");
    press(&mut h, Role::Button, "Duplicate node");
    assert_eq!(h.state().ui_screens.selected, Some(vec![1, 0, 3]));
    assert_eq!(node(h.state(), &[1, 0, 3]).unwrap().id, "options2");
    press(&mut h, Role::Button, "Delete node");
    wait_for(&mut h, "the new node to be written", |a| {
        !a.ui_screens.dirty
    });
    assert_eq!(ids(&disk()), ["", "resume", "options", "quit", ""]);
    assert_eq!(disk().root.at(&[1, 0, 4]).unwrap().text, "Text");

    // Quit dragged onto Resume's upper half: before it.
    let from = h.get_by_label("Button \"quit\"").rect();
    let onto = h.get_by_label("Button \"resume\"").rect();
    let (a, b) = (from.center(), egui::pos2(onto.center().x, onto.min.y + 4.0));
    h.hover_at(a);
    h.run_steps(1);
    let button = |h: &Harness<'static, App>, pos, pressed| {
        h.event(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        })
    };
    button(&h, a, true);
    h.run_steps(1);
    for k in 1..=10 {
        h.hover_at(a + (b - a) * (k as f32 / 10.0));
        h.run_steps(1);
    }
    button(&h, b, false);
    h.run_steps(3);
    assert_eq!(h.state().ui_screens.selected, Some(vec![1, 0, 1]));
    // And the new text moved up past Options.
    click(&mut h, "Text 'Text'");
    press(&mut h, Role::Button, "Move node up");
    wait_for(&mut h, "the moves to be written", |a| !a.ui_screens.dirty);
    assert_eq!(ids(&disk()), ["", "quit", "resume", "", "options"]);

    // The title on the 3DS: the menu on the touch screen, both shown.
    pick(&mut h, "UI target", "3DS");
    pick(&mut h, "UI screen", "title");
    let s = &h.state().ui_screens;
    let p = s.preview.as_ref().unwrap();
    assert_eq!((p.size, p.bottom_size), ((400, 240), Some((320, 240))));
    let play = p.paths.iter().position(|q| q == &[2, 0]).unwrap();
    let slot =
        p.ui.slots()
            .iter()
            .find(|s| s.node as usize == play)
            .unwrap();
    assert_eq!(slot.on, Screen::Bottom);
    assert!(p.placed.unwrap().bottom.is_some());
    assert!(s.issues.is_empty(), "{:?}", s.issues);

    // Pause in Portuguese: Quit's text runs past its button.
    pick(&mut h, "UI language", "pt-BR");
    pick(&mut h, "UI screen", "pause");
    let s = &h.state().ui_screens;
    let quit = s
        .def
        .as_ref()
        .unwrap()
        .nodes()
        .iter()
        .position(|(_, n)| n.id == "quit");
    assert_eq!(s.overflowing(), [quit.unwrap()]);
    assert!(h.query_by_label_contains("1 overflowing").is_some());
    let issue = h.get_by_label_contains("text overflows").rect();
    // A click on the preview selects what is under it (Options, on the
    // 3DS's top screen); one on the issue selects Quit.
    let at = h.state().ui_screens.node_point(&[1, 0, 4]).unwrap();
    h.hover_at(at);
    button(&h, at, true);
    button(&h, at, false);
    h.run_steps(3);
    assert_eq!(h.state().ui_screens.selected, Some(vec![1, 0, 4]));
    h.hover_at(issue.center());
    button(&h, issue.center(), true);
    button(&h, issue.center(), false);
    h.run_steps(3);
    assert_eq!(h.state().ui_screens.selected, Some(vec![1, 0, 1]));
    // Nothing above was written as a side effect of looking.
    assert_eq!(ids(&disk()), ["", "quit", "resume", "", "options"]);

    // A new screen: written, opened, valid.
    press(&mut h, Role::Button, "New screen");
    h.get_by_label("New screen name").focus();
    h.run_steps(2);
    h.event(Event::Text("shop".into()));
    h.run_steps(2);
    press(&mut h, Role::Button, "Create screen");
    wait_for(&mut h, "the new screen to be written", |a| {
        !a.ui_screens.dirty && std::path::Path::new("ui/shop.ui.ron").exists()
    });
    assert_eq!(h.state().ui_screens.name(), "shop");
    let shop = ScreenDef::load(std::path::Path::new("ui/shop.ui.ron")).unwrap();
    assert!(trino_scene::ui::validate(&shop).is_empty());
    assert!(h.state().ui_screens.files.len() >= 5);
    leave_temp_project(&repo);
}

/// The Localization tab (#59), both ways: `TRINO_EDITOR_TAB=localization`
/// and `TRINO_EDITOR_LANG` open it with a preview language the UI tab
/// shares; a cell edited → its language's RON file on disk; the file
/// edited outside → the cell; a language added → its file, falling back to
/// the default; the pseudo-locale and the CSV written as files; the fonts
/// measured per language.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn localization_tab_edits_the_tables_and_follows_their_files() {
    use egui::Event;
    use egui::accesskit::Role;
    use trino_scene::i18n::StringsDef;
    let setup = |repo: &std::path::Path, dir: &std::path::Path| {
        let i18n = dir.join("assets/i18n");
        let _ = std::fs::remove_dir_all(&i18n);
        std::fs::create_dir_all(&i18n).unwrap();
        std::fs::copy(
            repo.join("templates/standalone/assets/i18n/en-US.ron"),
            i18n.join("en-US.ron"),
        )
        .unwrap();
        std::fs::write(i18n.join("pt-BR.ron"), PT_BR).unwrap();
        std::fs::write(
            i18n.join("es-ES.ron"),
            "Strings(lang: \"es-ES\", fallback: \"en-US\", entries: {\"pause.title\": \"Pausa\"})\n",
        )
        .unwrap();
    };
    // SAFETY: --ignored --test-threads=1 only (module docs).
    unsafe {
        std::env::set_var("TRINO_EDITOR_TAB", "localization");
        std::env::set_var("TRINO_EDITOR_LANG", "pt-BR");
    }
    let started = temp_project_app_with(egui::vec2(1600.0, 940.0), setup);
    // SAFETY: as above.
    unsafe {
        std::env::remove_var("TRINO_EDITOR_TAB");
        std::env::remove_var("TRINO_EDITOR_LANG");
    }
    let Some((mut h, _tmp, repo)) = started else {
        return;
    };
    const PT: &str = "assets/i18n/pt-BR.ron";
    let disk = |f: &str| StringsDef::load(std::path::Path::new(f)).unwrap();
    h.run_steps(3);
    assert!(
        h.query_by_label("Preview language").is_some(),
        "the Localization tab is in front"
    );
    assert_eq!(h.state().localization.preview.as_deref(), Some("pt-BR"));
    assert_eq!(
        h.state().ui_screens.lang.as_deref(),
        Some("pt-BR"),
        "shared"
    );
    assert_eq!(
        h.state().localization.columns()[0],
        "en-US",
        "the default first"
    );
    // pt-BR lacks some keys the template has: they fall back to en-US.
    assert!(
        h.state()
            .localization
            .issues
            .iter()
            .any(|i| i.lang == "pt-BR")
    );

    // A cell edited: the file on disk follows once the field is left.
    h.get_by_label("pt-BR pause.resume").focus();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(Event::Text("Retomar".into()));
    h.key_press(egui::Key::Enter);
    wait_for(&mut h, "the edit to be written", |_| {
        disk(PT)
            .entries
            .get("pause.resume")
            .map(|e| e.text().to_string())
            == Some("Retomar".into())
    });
    // A missing cell typed into: the key joins the language.
    h.get_by_label("es-ES pause.quit").focus();
    h.run_steps(2);
    h.event(Event::Text("Salir".into()));
    h.key_press(egui::Key::Enter);
    wait_for(&mut h, "the new es-ES key to be written", |_| {
        disk("assets/i18n/es-ES.ron")
            .entries
            .contains_key("pause.quit")
    });

    // Edited outside: the tab follows and doesn't write it back.
    let outside = std::fs::read_to_string(PT)
        .unwrap()
        .replace("Pausado", "Em pausa");
    std::fs::write(PT, &outside).unwrap();
    let row = crate::localization::Row {
        key: "pause.title".into(),
        form: None,
    };
    wait_for(&mut h, "the outside edit", |a| {
        a.localization.text("pt-BR", &row) == "Em pausa"
    });
    h.run_steps(20);
    assert_eq!(std::fs::read_to_string(PT).unwrap(), outside);

    // A language added: its file, falling back to the default.
    press(&mut h, Role::Button, "Add language");
    h.get_by_label("New language tag").focus();
    h.run_steps(2);
    h.event(Event::Text("fr_FR".into()));
    h.run_steps(2);
    press(&mut h, Role::Button, "Create language");
    wait_for(&mut h, "fr-FR to be written", |_| {
        std::path::Path::new("assets/i18n/fr-FR.ron").exists()
    });
    assert_eq!(disk("assets/i18n/fr-FR.ron").fallback, "en-US");

    // The pseudo-locale and the CSV are files too.
    press(&mut h, Role::Button, "Make pseudo-locale");
    wait_for(&mut h, "qps-ploc to be written", |_| {
        std::path::Path::new("assets/i18n/qps-ploc.ron").exists()
    });
    assert!(
        disk("assets/i18n/qps-ploc.ron").entries["pause.title"]
            .text()
            .starts_with('[')
    );
    press(&mut h, Role::Button, "Export CSV");
    let csv = std::fs::read_to_string(crate::localization::CSV_PATH).unwrap();
    assert!(csv.starts_with("key,en-US,"), "{csv}");
    assert!(csv.contains("Retomar"));

    // The preview language picked here is the UI tab's.
    pick(&mut h, "Preview language", "es-ES");
    assert_eq!(h.state().ui_screens.lang.as_deref(), Some("es-ES"));

    // The fonts: measured on a worker, a line per checked font × language.
    wait_for(&mut h, "the fonts to be measured", |a| {
        !a.localization.fonts_measuring() && !a.localization.fonts.is_empty()
    });
    let f = &h.state().localization.fonts;
    assert!(
        f.iter().any(|r| r.lang == "pt-BR" && r.n64_bytes.is_some()),
        "{f:?}"
    );
    leave_temp_project(&repo);
}

/// The Profiler tab's fixtures (issue #62): trace A as Chrome JSON
/// (`chrome::write`), trace B as a log of `TRINO_TRACE:` lines (what `cargo
/// xtask run pc --trace` writes), so both loaders run. B's `tnl` is 50 µs
/// slower and it adds an RSP zone; `extra` adds a zone to A (a live
/// reload). `n64-test.json` is an N64 run of the open scene (the budget's
/// "measured" line).
fn profiler_fixtures(project: &std::path::Path, extra: bool) {
    use trino_core::trace::{TracePlatform, TraceRing, track};
    let dir = project.join("target/traces");
    std::fs::create_dir_all(&dir).unwrap();
    let ring = |tnl: u32, rsp: bool, late: bool| {
        let mut r = TraceRing::<256>::new();
        for f in 0..4u32 {
            let t = f * 1000;
            r.frame(t, f);
            r.zone("tnl", track::CPU, t + 10, t + 10 + tnl);
            r.zone("sort", track::CPU, t + 300, t + 310);
            if rsp {
                r.zone("rsp wait", track::RSP, t + 400, t + 440);
            }
            if late {
                r.zone("late zone", track::CPU, t + 500, t + 520);
            }
        }
        r
    };
    let mut text = String::new();
    ring(100, false, extra)
        .write_lines(TracePlatform::Pc, "pc-test", &mut 0, &mut text)
        .unwrap();
    let a = trino_trace::collect(&text).unwrap();
    std::fs::write(
        dir.join("a.json"),
        trino_trace::chrome::write(&a).to_string(),
    )
    .unwrap();
    let mut log = String::from("[game] boot\n");
    ring(150, true, false)
        .write_lines(TracePlatform::Pc, "pc-test", &mut 0, &mut log)
        .unwrap();
    std::fs::write(dir.join("b.log"), log).unwrap();
    let mut n64 = String::new();
    ring(100, false, false)
        .write_lines(TracePlatform::N64, "n64-showcase", &mut 0, &mut n64)
        .unwrap();
    let t = trino_trace::collect(&n64).unwrap();
    std::fs::write(
        dir.join("n64-test.json"),
        trino_trace::chrome::write(&t).to_string(),
    )
    .unwrap();
}

/// The Profiler tab (issue #62): pick trace A in the file combo, its zones
/// show in the table; pick B (a log), the diff shows B − A; A rewritten on
/// disk reloads live; the scene settings' platform budget shows the N64
/// frame a trace measured, and its button opens that trace here.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn profiler_tab_reads_compares_and_follows_traces() {
    use crate::app::{Cmd, Tab};
    use egui::accesskit::Role;
    let Some((mut h, _tmp, repo)) = temp_project_app_with(egui::vec2(1600.0, 1000.0), |_, p| {
        profiler_fixtures(p, false)
    }) else {
        return;
    };
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Profiler));
    h.run_steps(4);
    assert!(
        h.query_all_by_label_contains("No trace open")
            .next()
            .is_some()
    );
    let names: Vec<String> = h
        .state()
        .profiler
        .files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["a.json", "b.log", "n64-test.json"]);
    pick(&mut h, "Trace A", "a.json");
    assert_eq!(h.state().profiler.session().unwrap().name, "pc-test");
    assert!(h.query_all_by_label("Timeline").next().is_some());
    assert!(h.query_all_by_label("Frame flame graph").next().is_some());
    for zone in ["tnl", "sort"] {
        assert!(
            h.query_all_by_label(zone).next().is_some(),
            "zone row {zone}"
        );
    }
    assert!(
        h.query_all_by_label("100 µs").next().is_some(),
        "tnl's mean"
    );
    assert!(h.query_all_by_label_contains("DIFF (B").next().is_none());
    // B: the log, paired with A's session by name.
    pick(&mut h, "Trace B", "b.log");
    assert!(
        h.state().profiler.b.is_some(),
        "B: {:?}",
        h.state().profiler.error
    );
    assert_eq!(h.state().profiler.session_b().unwrap().name, "pc-test");
    assert!(h.query_all_by_label_contains("DIFF (B").next().is_some());
    assert!(
        h.query_all_by_label("+50 µs").next().is_some(),
        "tnl's mean delta"
    );
    assert!(h.query_all_by_label("+50%").next().is_some());
    assert!(
        h.query_all_by_label("rsp wait").next().is_some(),
        "B's new zone"
    );
    assert!(h.query_all_by_label("new").next().is_some());
    // A changes on disk: the tab follows it.
    let root = std::env::current_dir().unwrap();
    profiler_fixtures(&root, true);
    wait_for(&mut h, "trace A to reload", |a| {
        a.profiler
            .session()
            .is_some_and(|s| s.names.iter().any(|n| n == "late zone"))
    });
    assert!(h.query_all_by_label("late zone").next().is_some());
    // Estimated vs measured: the scene settings' platform budget.
    h.state_mut().cmds.push(Cmd::Select(None));
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Scene));
    let start = std::time::Instant::now();
    while h
        .query_all_by_label_contains("N64 measured")
        .next()
        .is_none()
    {
        assert!(start.elapsed().as_secs() < 60, "no measured N64 line");
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert!(
        h.query_by_label_contains("N64 measured 1.0 ms (n64-showcase, 3 frames)")
            .is_some()
    );
    press(&mut h, Role::Button, "Profiler");
    let p = &h.state().profiler;
    assert_eq!(p.session().unwrap().name, "n64-showcase");
    assert!(
        p.a.as_ref()
            .unwrap()
            .path
            .ends_with("target/traces/n64-test.json")
    );
    leave_temp_project(&repo);
}

/// The perf reports the Profiler's Reports and A/B views read (issue #79):
/// two N64 scene reports (`target/perf/`, as `cargo xtask test n64 --prof
/// rsp|sample` writes them) and an A/B over four paddings with a control
/// (`target/perf-ab/ab.json`, as `cargo xtask perf ab` writes it), built
/// from log lines through the same collector.
fn perf_fixtures(project: &std::path::Path) {
    use std::collections::BTreeMap;
    use trino_trace::perf::{AbReport, Collector, Symbols};
    let dir = project.join("target/perf");
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = Collector::default();
    for line in [
        "TRINO_PERF_SCENE:hub frames=4 update=300 render=13000 begin=1000 tnl=5200 sort=400 \
         submit=2100 sprites=600 end=900 verts=820 tris_in=900 emitted=300 rsp_draws=41 \
         rsp_tris=1200 memo_hit=37 memo_miss=4 rspq_words=1900 ovl_switch=6 tex_upload=3",
        "TRINO_PERF_SCENE:throne frames=4 update=280 render=9000 begin=800 tnl=3000 verts=500 \
         memo_hit=20 memo_miss=2",
        "TRINO_PROF_RSP:hub empty_drain_us=45",
        "TRINO_PROF_RSP:hub ovl=white verts=10 tris=10 n=2 us=40",
        "TRINO_PROF_RSP:hub ovl=white verts=20 tris=10 n=1 us=30",
        "TRINO_PROF_RSP:hub ovl=white verts=20 tris=30 n=1 us=50",
        "TRINO_PROF_SAMPLE:hub samples=4 lost=0 period_us=50 frames=4 pcs=1 handler_us=0.8 \
         overhead_pct=1.6",
        "TRINO_PROF_SAMPLE:hub 80001004=4",
    ] {
        c.feed("n64", "prof-rsp", 0, line);
    }
    let syms = Symbols::parse("80001000 00000010 T trino_core::render3d::tessellate_joints\n");
    for r in c.finish(Some(&syms), |_| None) {
        std::fs::write(dir.join(format!("n64-{}.json", r.scene)), r.to_json()).unwrap();
    }
    let scene = |frame: u64| {
        let mut f = BTreeMap::new();
        f.insert("render".to_string(), frame);
        f.insert("verts".to_string(), 820);
        BTreeMap::from([("hub".to_string(), f)])
    };
    let runs = vec![
        (
            "base".to_string(),
            [13_000, 13_400, 12_900, 13_200].map(scene).to_vec(),
        ),
        (
            "head".to_string(),
            [12_100, 12_500, 12_000, 12_300].map(scene).to_vec(),
        ),
        (
            "control".to_string(),
            [13_300, 12_800, 13_500, 13_000].map(scene).to_vec(),
        ),
    ];
    let ab = AbReport::build("n64", "main", "HEAD", &[0, 352, 1024, 2048], &runs);
    let ab_dir = project.join("target/perf-ab");
    std::fs::create_dir_all(&ab_dir).unwrap();
    std::fs::write(ab_dir.join("ab.json"), ab.to_json()).unwrap();
}

/// The Profiler's Reports and A/B views (issue #79): the editor's own PC
/// frame shows live; report A (the hub) shows its stages, counters, the
/// RSP fit and the CPU samples; picking B compares them; the A/B view
/// shows each arm's spread over the paddings with the work identical; a
/// report rewritten on disk reloads; Play with the Profiler showing
/// follows the game's trace.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn profiler_reports_ab_and_play_attach() {
    use crate::app::{Cmd, Tab};
    let Some((mut h, _tmp, repo)) =
        temp_project_app_with(egui::vec2(1600.0, 1000.0), |_, p| perf_fixtures(p))
    else {
        return;
    };
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Profiler));
    h.run_steps(4);
    click(&mut h, "Reports");
    h.run_steps(4);
    assert_eq!(h.state().profiler.mode, crate::profiler::Mode::Reports);
    assert!(
        h.query_by_label_contains("PC FRAME (GAME VIEW, LIVE)")
            .is_some()
    );
    wait_for(&mut h, "a live frame", |a| a.profiler.live.last().is_some());
    // Report A opened on its own: the first file (n64-hub.json).
    assert!(
        h.query_by_label_contains("n64 hub · prof-rsp build")
            .is_some()
    );
    for text in ["memo_hit", "rspq_words", "white", "tessellate_joints"] {
        assert!(
            h.query_all_by_label_contains(text).next().is_some(),
            "{text}"
        );
    }
    // B: the throne room, compared stage by stage.
    pick(&mut h, "Report B", "n64-throne.json");
    assert!(h.state().profiler.reports.b.is_some());
    assert!(h.query_all_by_label("B − A").next().is_some());
    assert!(
        h.query_all_by_label("−17").next().is_some(),
        "memo_hit 37 -> 20"
    );
    // The A/B view: three arms, their spreads, identical work.
    click(&mut h, "A/B");
    h.run_steps(4);
    assert!(
        h.query_by_label_contains("control: base + a no-op patch")
            .is_some()
    );
    assert!(
        h.query_by_label_contains("work is identical in every arm")
            .is_some()
    );
    assert!(h.query_by_label_contains("head−base −900 µs").is_some());
    // A report rewritten on disk reloads.
    let root = std::env::current_dir().unwrap();
    let f = root.join("target/perf/n64-hub.json");
    let text = std::fs::read_to_string(&f)
        .unwrap()
        .replace("\"memo_hit\": 37", "\"memo_hit\": 99");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(&f, text).unwrap();
    wait_for(&mut h, "report A to reload", |a| {
        a.profiler
            .reports
            .a
            .as_ref()
            .is_some_and(|(_, r)| r.counters["memo_hit"] == 99)
    });
    // Play with the tab showing: traced, and the tab follows the log.
    let p = &mut h.state_mut().profiler;
    assert!(p.is_shown());
    let log = root.join("target/traces/pc.log");
    p.follow(log.clone());
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    let mut text = String::new();
    let mut ring = trino_core::trace::TraceRing::<16>::new();
    ring.zone("frame", 0, 0, 900);
    ring.write_lines(
        trino_core::trace::TracePlatform::Pc,
        "pc",
        &mut 0,
        &mut text,
    )
    .unwrap();
    std::fs::write(&log, text).unwrap();
    wait_for(&mut h, "the followed trace", |a| {
        a.profiler.follow.is_none() && a.profiler.session().is_some()
    });
    assert_eq!(h.state().profiler.mode, crate::profiler::Mode::Timeline);
    leave_temp_project(&repo);
}

/// Physics 2.0 in the editor (issue #66), both ways: the Project ›
/// Physics page adds a layer and unchecks a pair of the matrix, writing
/// `assets/physics.ron` (what `cargo xtask physics layers` writes); an
/// outside edit of the file shows at once; the Inspector shows a ramp's
/// slope, a mesh collider's model, a collider's layer list and the hero's
/// Character component; the budget HUD's collision line counts the
/// playground; and Play here walks the hero on the engine's controller
/// (its debug overlay's data) up the 30° ramp.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn physics_layers_inspector_and_play_here_controller() {
    use crate::app::{Cmd, Tab};
    use egui::accesskit::Role;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    let read = || {
        trino_scene::physics::PhysicsSettings::from_ron(
            &std::fs::read_to_string(trino_scene::physics::PHYSICS_FILE).unwrap(),
        )
        .unwrap()
    };
    // The Project tab's Physics page.
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Project));
    h.state_mut().shipping.page = crate::shipping::Page::Physics;
    h.run_steps(4);
    assert_eq!(
        h.state().lib.physics.layers,
        ["world", "player", "enemy", "pickup"]
    );
    press(&mut h, Role::Button, "Add layer");
    h.run_steps(2);
    assert_eq!(read().layers.len(), 5, "a layer added on disk");
    press(&mut h, Role::CheckBox, "player × pickup");
    h.run_steps(2);
    assert!(
        !read().collide("player", "pickup"),
        "the pair unchecked on disk"
    );
    assert!(!h.state().lib.physics.collide("player", "pickup"));
    // Edited outside: the page follows.
    let mut outside = read();
    outside.layers[4] = "water".into();
    std::fs::write(trino_scene::physics::PHYSICS_FILE, outside.to_ron()).unwrap();
    wait_for(&mut h, "the outside physics.ron edit", |a| {
        a.lib.physics.layers.get(4).is_some_and(|l| l == "water")
    });

    // The playground in the Inspector.
    h.state_mut()
        .cmds
        .push(Cmd::OpenScene(std::path::PathBuf::from(
            "scenes/physics_playground.scene.ron",
        )));
    h.run_steps(6);
    let select = |h: &mut Harness<'static, App>, name: &str| {
        let path = find(&h.state().doc.scene, name);
        h.state_mut().cmds.push(Cmd::Select(Some(path)));
        h.run_steps(4);
    };
    select(&mut h, "ramp-30");
    assert!(h.query_by_label_contains("Rises toward").is_some());
    // The slope with the entity's scale (2 × 1.732 × 1 over a 2 × 1 × 3 box).
    assert!(h.query_by_label_contains("30.0° slope").is_some());
    assert!(
        h.query_by_label_contains("18.4° slope").is_none(),
        "not the unscaled box's"
    );
    assert!(h.query_by_label_contains("Layer").is_some());
    select(&mut h, "dome");
    assert!(h.query_by_label_contains("Collision of").is_some());
    assert!(
        h.query_by_label_contains("has no `collision` key")
            .is_none(),
        "models/rock bakes its collision"
    );
    assert!(
        h.state()
            .project
            .collision_models()
            .contains(&"models/rock".to_string())
    );
    select(&mut h, "hero");
    assert!(h.query_by_label_contains("Step height").is_some());
    assert!(h.query_by_label_contains("Max slope").is_some());
    // The budget HUD's collision line.
    let line = crate::budget::collision_line(
        &h.state().resolved,
        &h.state().project,
        trino_scene::Target::N64,
    )
    .unwrap();
    assert!(
        line.0.contains("1 one-way")
            && line.0.contains("2 ramps")
            && line.0.contains("mesh 28/4096 tris"),
        "{line:?}"
    );

    // Play here: the hero walks on the engine's controller.
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
    h.state_mut().cmds.push(Cmd::PlayHere);
    h.run_steps(8);
    let session = h.state().game_view.play.as_ref().expect("playing here");
    let (ctl, _) = session
        .game
        .controller_debug()
        .expect("the hero's controller");
    assert!(
        (ctl.cfg.step_height - 0.3).abs() < 1e-6,
        "the scene's character body"
    );
    // Put the hero at the foot of the 30° ramp and walk it up (+Z).
    {
        let game = &mut h.state_mut().game_view.play.as_mut().unwrap().game;
        let mut input = trino_core::InputState::default();
        use trino_core::Game as _;
        for _ in 0..30 {
            game.update(&input, &mut crate::play::NullAudio, 1.0 / 60.0);
        }
        // The camera looks down -Z: stick up walks away from it, toward -Z,
        // up the ramp (turned to rise toward -Z).
        game.teleport_hero(trino_core::Vec3::new(-7.0, 0.0, -2.0));
        input.stick = trino_core::Vec2::new(0.0, 1.0);
        let mut top: f32 = 0.0;
        for _ in 0..240 {
            game.update(&input, &mut crate::play::NullAudio, 1.0 / 60.0);
            top = top.max(game.pos.y);
        }
        let (ctl, rep) = game.controller_debug().unwrap();
        assert!(
            top > 1.73 + 0.85 - 0.05,
            "climbed the ramp onto the deck: {top}"
        );
        assert!(ctl.on_ground && !rep.sliding, "{rep:?}");
    }
    h.state_mut().cmds.push(Cmd::StopPlayHere);
    h.run_steps(2);
    leave_temp_project(&repo);
}

/// Issue #96, item 1: a sound's caption in Assets › Audio. The key from
/// the tables' keys, then `important` and the seconds, each written to the
/// manifest; the texts per language; Preview queues it in the Game view;
/// a new key lands in every language; an outside edit shows at once.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn audio_inspector_authors_a_sounds_caption() {
    use crate::app::{AssetSel, Cmd};
    use egui::accesskit::Role;
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    const TABLE: &str = "[sounds.jump]";
    let table = || {
        let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
        manifest_table(&text, TABLE).to_string()
    };
    h.state_mut()
        .cmds
        .push(Cmd::SelectAsset(AssetSel::Audio("sounds/jump".into())));
    wait_for(&mut h, "the caption section", |a| {
        a.audio.caption_shown().is_some()
    });
    assert!(h.query_by_label("Caption key").is_some());
    assert_eq!(h.state().audio.caption_shown().unwrap().key, None);
    // The key from the string tables: its text in every language.
    pick(&mut h, "Caption key", "cap.coin");
    assert!(table().contains("caption = \"cap.coin\""), "{}", table());
    wait_for(&mut h, "the key to show", |a| {
        a.audio.caption_shown().unwrap().key.as_deref() == Some("cap.coin")
    });
    for text in ["[coin chimes]", "[moeda tilinta]", "[tintineo de moneda]"] {
        assert!(h.query_by_label(text).is_some(), "{text}");
    }
    // Important, a priority and the seconds.
    press(&mut h, Role::CheckBox, "Important");
    assert!(table().contains("important = true"), "{}", table());
    set_value(&mut h, "Caption priority", 180.0);
    set_value(&mut h, "Caption seconds", 2.5);
    h.run_steps(3);
    let t = table();
    assert!(
        t.contains("caption_priority = 180") && t.contains("caption_seconds = 2.5"),
        "{t}"
    );
    // Preview: queued in the Game view's simulation.
    press(&mut h, Role::Button, "Preview");
    h.run_steps(3);
    assert!(!h.state().access.caption.queue.is_empty());
    assert_eq!(
        h.state().access.caption.queue.keys().next(),
        Some("cap.coin")
    );
    // An outside edit of the manifest shows at once.
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    std::fs::write(
        "assets/manifest.toml",
        text.replace(
            "caption = \"cap.coin\"\nimportant",
            "caption = \"cap.star\"\nimportant",
        ),
    )
    .unwrap();
    wait_for(&mut h, "the outside edit", |a| {
        a.audio.caption_shown().unwrap().key.as_deref() == Some("cap.star")
    });
    // A new key: added to every language, then written.
    h.state_mut()
        .cmds
        .push(Cmd::SelectAsset(AssetSel::Audio("sounds/jump".into())));
    pick(&mut h, "Caption key", "New key…");
    h.get_by_label("New caption key").focus();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("cap.jump".into()));
    h.key_press(egui::Key::Enter);
    wait_for(&mut h, "the new key", |a| {
        a.audio.caption_shown().unwrap().key.as_deref() == Some("cap.jump")
    });
    let langs = &h.state().localization.langs;
    for lang in ["en-US", "pt-BR", "es-ES"] {
        assert!(langs.strings(lang).has_own("cap.jump"), "{lang}");
    }
    wait_for(&mut h, "the tables to be written", |_| {
        std::fs::read_to_string("assets/i18n/pt-BR.ron").is_ok_and(|t| t.contains("cap.jump"))
    });
    leave_temp_project(&repo);
}

/// Issue #96, item 2: color-vision icons. A UI node's from the UI tab's
/// inspector (written to its `.ui.ron`, an outside edit followed), the
/// hint on a node in a distinct group's color without one; a sprite's
/// from the texture inspector (written to the manifest).
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn icons_for_color_vision_on_nodes_and_sprites() {
    use crate::app::{AssetSel, Cmd};
    const SCREEN: &str = "ui/selftest.ui.ron";
    // SAFETY: --ignored --test-threads=1 only (module docs).
    unsafe { std::env::set_var("TRINO_EDITOR_UI", "selftest,pc") };
    let started = temp_project_app(egui::vec2(1600.0, 1000.0));
    // SAFETY: as above.
    unsafe { std::env::remove_var("TRINO_EDITOR_UI") };
    let Some((mut h, _tmp, repo)) = started else {
        return;
    };
    let node = |a: &App, id: &str| {
        a.ui_screens
            .def
            .as_ref()?
            .nodes()
            .into_iter()
            .find(|(_, n)| n.id == id)
            .map(|(_, n)| n.clone())
    };
    h.run_steps(3);
    click(&mut h, "Button \"two\"");
    pick(&mut h, "Icon (color vision)", "sprites/cvd_stripes");
    wait_for(&mut h, "the icon to be written", |a| {
        !a.ui_screens.dirty && node(a, "two").is_some_and(|n| n.cvd_icon == "sprites/cvd_stripes")
    });
    let text = std::fs::read_to_string(SCREEN).unwrap();
    assert!(
        text.contains("id: \"two\"")
            && text.matches("cvd_icon: \"sprites/cvd_stripes\"").count() == 2,
        "{text}"
    );
    // Outside: `three` turns castle64's green door color, no icon: the hint.
    let mut def = trino_scene::ui::ScreenDef::from_ron(&text).unwrap();
    let three = def
        .nodes()
        .into_iter()
        .find(|(_, n)| n.id == "three")
        .map(|(p, _)| p)
        .unwrap();
    def.root.at_mut(&three).unwrap().color = Some([95, 205, 95, 255]);
    std::fs::write(SCREEN, def.to_ron()).unwrap();
    wait_for(&mut h, "the outside edit", |a| {
        node(a, "three").is_some_and(|n| n.color.is_some())
    });
    click(&mut h, "Button \"three\"");
    h.run_steps(3);
    assert!(
        h.query_by_label_contains("shows castle64.door.green")
            .is_some(),
        "the hint"
    );
    // A sprite's icon from the texture inspector.
    h.state_mut()
        .cmds
        .push(Cmd::SelectAsset(AssetSel::Sprite("sprites/flag".into())));
    wait_for(&mut h, "the texture inspector", |a| {
        a.textures
            .icon_shown()
            .is_some_and(|(l, _)| l == "sprites/flag")
    });
    pick(&mut h, "Sprite icon (color vision)", "sprites/cvd_stripes");
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    assert!(
        manifest_table(&text, "[sprites.flag]").contains("cvd_icon = \"sprites/cvd_stripes\""),
        "{text}"
    );
    wait_for(&mut h, "the icon to show", |a| {
        a.textures.icon_shown() == Some(("sprites/flag", Some("sprites/cvd_stripes")))
    });
    // The manifest edited outside: the row follows.
    let text = std::fs::read_to_string("assets/manifest.toml").unwrap();
    std::fs::write(
        "assets/manifest.toml",
        text.replacen("cvd_icon = \"sprites/cvd_stripes\"\n", "", 1),
    )
    .unwrap();
    wait_for(&mut h, "the outside edit of the icon", |a| {
        a.textures.icon_shown() == Some(("sprites/flag", None))
    });
    leave_temp_project(&repo);
}

/// Issue #96, items 3 and 4: an entity's color slots (a semantic color of
/// `palettes.ron` or a literal one) and its Outline component, from the
/// Inspector to the scene file; the Game view draws the slot in the
/// simulated mode and the ring in simulated high contrast.
#[test]
#[ignore = "changes cwd/env: run alone with --ignored --test-threads=1"]
fn palette_slots_and_outlines_in_the_inspector() {
    use crate::app::{Cmd, Tab};
    let Some((mut h, _tmp, repo)) = temp_project_app(egui::vec2(1600.0, 1000.0)) else {
        return;
    };
    // A slotted card in front of the scene's camera.
    {
        let app = h.state_mut();
        let cam = app.doc.scene.environment.camera;
        let mut card = trino_scene::Entity::new("card")
            .with_model(trino_scene::ModelComponent::new("models/selftest_slots"));
        let t = cam.target;
        card.transform.pos = [
            t[0] + (cam.eye[0] - t[0]) * 0.5,
            t[1] + (cam.eye[1] - t[1]) * 0.5,
            t[2] + (cam.eye[2] - t[2]) * 0.5,
        ];
        app.doc.scene.entities.push(card);
        let i = app.doc.scene.entities.len() - 1;
        app.cmds.push(Cmd::Select(Some(vec![i])));
    }
    h.run_steps(4);
    let i = h.state().doc.scene.entities.len() - 1;
    let model = |a: &App| a.doc.scene.entities[i].model.clone().unwrap();
    // Slot 1: castle64's red door color; slot 4: a literal color.
    pick(&mut h, "Slot 1 color", "castle64.door.red");
    pick(&mut h, "Slot 4 color", "Color");
    h.run_steps(3);
    let m = model(h.state());
    assert_eq!(
        m.palette,
        vec![
            trino_scene::SlotColorDef {
                slot: 1,
                color: trino_scene::SlotColor::Semantic("castle64.door.red".into()),
            },
            trino_scene::SlotColorDef {
                slot: 4,
                color: trino_scene::SlotColor::Rgb([255, 255, 255]),
            },
        ]
    );
    // The resolved draw: the base color; in the simulated mode, its
    // alternative.
    let red = |a: &App| {
        let item = a.resolved.items.iter().find(|it| it.name == "card")?;
        Some(crate::viewport::view_params(item, &a.access.view()).palette?[0])
    };
    assert_eq!(red(h.state()), Some(trino_core::Color::rgb(230, 95, 80)));
    h.state_mut().access.simulate = true;
    h.state_mut().access.preview.cvd = trino_core::CvdMode::Deuteranopia;
    assert_eq!(red(h.state()), Some(trino_core::Color::rgb(120, 70, 30)));
    // An Outline from Add component; high contrast simulated: one more draw.
    click(&mut h, "Add component");
    click(&mut h, "Outline");
    h.run_steps(3);
    assert_eq!(
        h.state().doc.scene.entities[i].outline,
        Some(trino_scene::OutlineComponent::default())
    );
    h.state_mut().cmds.push(Cmd::FocusTab(Tab::Game));
    h.run_steps(6);
    let before = h.state().game_view.stats.frame.models;
    h.state_mut().access.preview.high_contrast = true;
    h.run_steps(6);
    assert_eq!(
        h.state().game_view.stats.frame.models,
        before + 1,
        "the ring"
    );
    // Saved: the scene file holds both.
    h.state_mut().cmds.push(Cmd::Save);
    h.run_steps(3);
    let text = std::fs::read_to_string("scenes/showcase.scene.ron").unwrap();
    assert!(text.contains("Semantic(\"castle64.door.red\")"), "{text}");
    assert!(text.contains("outline: Some("), "{text}");
    assert!(text.contains("version: 8"), "{text}");
    // Edited outside: the slot's color and the outline's width follow.
    let mut outside = trino_scene::Scene::from_ron(&text).unwrap();
    let card = &mut outside.entities[i];
    card.model.as_mut().unwrap().palette[0].color =
        trino_scene::SlotColor::Semantic("castle64.door.blue".into());
    card.outline.as_mut().unwrap().pixels = 3.0;
    std::fs::write("scenes/showcase.scene.ron", outside.to_ron()).unwrap();
    wait_for(&mut h, "the outside edit", |a| {
        let e = &a.doc.scene.entities[i];
        e.model.as_ref().unwrap().palette[0].color
            == trino_scene::SlotColor::Semantic("castle64.door.blue".into())
            && e.outline.as_ref().is_some_and(|o| o.pixels == 3.0)
    });
    leave_temp_project(&repo);
}
