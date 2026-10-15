//! Headless widget tests (egui_kittest without a renderer, so no GPU and no
//! `#[ignore]`): the inspector's material and fake-object editors and the
//! Console panel, driven by accessibility labels like a user would.

use eframe::egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use trino_asset_pipeline::{Pivot, Primitive, Shape};
use trino_scene::{MaterialDef, ShadingDef};

use crate::log::{Level, Log};
use crate::panels::hierarchy::shape_glyph;
use crate::panels::inspector::{material_editor, primitive_editor};
use crate::theme::icon;

struct MatState {
    def: MaterialDef,
    changes: usize,
}

#[test]
fn material_editor_sets_shading_sides_and_texture() {
    let textures = vec!["sprites/brick".to_string(), "sprites/planks".to_string()];
    let mut h = Harness::new_ui_state(
        |ui, s: &mut MatState| {
            if material_editor(ui, &mut s.def, &textures) {
                s.changes += 1;
            }
        },
        MatState {
            def: MaterialDef::default(),
            changes: 0,
        },
    );
    h.run();
    assert_eq!(h.state().changes, 0, "no edit, no change");

    h.get_by_label("Flat").click();
    h.run();
    assert_eq!(h.state().def.shading, ShadingDef::Flat);
    h.get_by_label("Unlit").click();
    h.run();
    assert_eq!(h.state().def.shading, ShadingDef::Unlit);

    h.get_by_label("Double-sided").click();
    h.run();
    assert!(h.state().def.double_sided);

    // Pick a texture from the combo box, then clear it again.
    h.get_by_role(Role::ComboBox).click();
    h.run();
    h.get_by_label("sprites/planks").click();
    h.run();
    assert_eq!(h.state().def.texture.as_deref(), Some("sprites/planks"));
    h.get_by_role(Role::ComboBox).click();
    h.run();
    h.get_by_label("None (model default / vertex colors)")
        .click();
    h.run();
    assert_eq!(h.state().def.texture, None);
    assert!(h.state().changes >= 5, "every edit reports a change");
    // The edited definition still round-trips through its file format.
    let def = &h.state().def;
    assert_eq!(&MaterialDef::from_ron(&def.to_ron()).unwrap(), def);
}

struct PrimState {
    prim: Primitive,
    changes: usize,
    fonts: bool,
}

#[test]
fn primitive_editor_switches_shape_pivot_and_keeps_sizes_valid() {
    let mut h = Harness::new_ui_state(
        |ui, s: &mut PrimState| {
            // The editor's bold font family must exist before a frame uses it.
            if !s.fonts {
                crate::theme::install(ui.ctx());
                s.fonts = true;
                return;
            }
            if primitive_editor(ui, &mut s.prim) {
                s.changes += 1;
            }
        },
        PrimState {
            prim: Primitive {
                shape: Shape::Box {
                    size: [-3.0, 2.0, 0.0],
                },
                ..Primitive::default()
            },
            changes: 0,
            fonts: false,
        },
    );
    h.run();
    // Degenerate box sizes are clamped to a minimum.
    match h.state().prim.shape {
        Shape::Box { size } => assert_eq!(size, [0.01, 2.0, 0.01]),
        ref other => panic!("{other:?}"),
    }

    h.get_by_label(shape_glyph(&Shape::Sphere { radius: 1.0 }))
        .click();
    h.run();
    assert!(matches!(h.state().prim.shape, Shape::Sphere { .. }));

    h.get_by_label("Bottom").click();
    h.run();
    assert_eq!(h.state().prim.pivot, Pivot::Bottom);

    // A capsule is never shorter than its two caps.
    h.state_mut().prim.shape = Shape::Capsule {
        radius: 2.0,
        height: 1.0,
    };
    h.run();
    match h.state().prim.shape {
        Shape::Capsule { radius, height } => assert_eq!((radius, height), (2.0, 4.0)),
        ref other => panic!("{other:?}"),
    }

    // Every other shape renders its own fields without losing them.
    for shape in Shape::all_defaults() {
        h.state_mut().prim.shape = shape.clone();
        h.run();
        assert_eq!(
            std::mem::discriminant(&h.state().prim.shape),
            std::mem::discriminant(&shape)
        );
    }
    assert!(h.state().changes >= 2);
}

#[test]
fn console_counts_filters_and_clears() {
    let mut log = Log::new();
    log.info("baked 3 assets");
    log.warn("scene has a missing model");
    log.error("cannot parse prefab");
    log.push(Level::Ok, "saved");
    for i in 0..6 {
        log.toast(Level::Info, format!("toast {i}"));
    }
    assert_eq!(log.counts(), (1, 1));
    assert_eq!(log.last().unwrap().text, "toast 5");
    assert_eq!(log.entries.len(), 10, "toasts are logged too");

    // Toasts are drawn on a context (at most four at a time).
    let ctx = eframe::egui::Context::default();
    let mut shown = false;
    let _ = ctx.run_ui(Default::default(), |ui| shown = log.show_toasts(ui.ctx()));
    assert!(shown, "fresh toasts are on screen");

    let mut h = Harness::new_ui_state(|ui, log: &mut Log| log.ui(ui), log);
    h.run();
    assert!(h.query_by_label("cannot parse prefab").is_some());
    // The Error filter hides error lines.
    h.get_by_label(&format!("{} 1", Level::Error.glyph()))
        .click();
    h.run();
    assert!(!h.state().filter[3]);
    assert!(h.query_by_label("cannot parse prefab").is_none());
    assert!(h.query_by_label("baked 3 assets").is_some());
    // Clear empties the console.
    h.get_by_label(icon::BROOM).click();
    h.run();
    assert!(h.state().entries.is_empty());
    assert_eq!(h.state().counts(), (0, 0));
}
