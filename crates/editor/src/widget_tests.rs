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

struct PlatState {
    e: trino_scene::Entity,
    changes: usize,
    fonts: bool,
}

fn platforms_harness(width: f32) -> Harness<'static, PlatState> {
    use crate::panels::platforms::{Choices, ModelInfo};
    let models: &'static [ModelInfo] = Box::leak(Box::new([
        ModelInfo {
            logical: "models/tree".into(),
            verts: 400,
            tris: 600,
        },
        ModelInfo {
            logical: "models/tree-low-poly-for-consoles".into(),
            verts: 40,
            tris: 48,
        },
    ]));
    let materials: &'static [String] = Box::leak(Box::new(["materials/leaf-flat".to_string()]));
    let mut e = trino_scene::Entity::new("tree");
    e.model = Some(trino_scene::ModelComponent::new("models/tree"));
    let mut h = Harness::builder()
        .with_size(eframe::egui::vec2(width, 900.0))
        .build_ui_state(
            move |ui, s: &mut PlatState| {
                if !s.fonts {
                    crate::theme::install(ui.ctx());
                    s.fonts = true;
                    return;
                }
                let choices = Choices { models, materials };
                if crate::panels::platforms::ui(ui, &mut s.e, &choices) {
                    s.changes += 1;
                }
            },
            PlatState {
                e,
                changes: 0,
                fonts: false,
            },
        );
    h.run();
    h
}

#[test]
fn platforms_tab_edits_variants_per_platform() {
    let mut h = platforms_harness(520.0);
    assert_eq!(h.state().changes, 0);
    // The N64 column picks the low model; the 3DS stays "Same as PC".
    h.get_by_label("N64 model").click();
    h.run();
    h.get_by_label("models/tree-low-poly-for-consoles").click();
    h.run();
    let v = h.state().e.platforms.n64.clone().expect("an N64 variant");
    assert_eq!(
        v.model.as_deref(),
        Some("models/tree-low-poly-for-consoles")
    );
    assert!(h.state().e.platforms.n3ds.is_none());
    // The cost line follows the variant's model (48 tris, not 600).
    assert!(h.query_by_label_contains("48 tris").is_some());

    // "Not on 3DS" drops it there; "Hidden on PC" hides it on PC.
    h.get_by_label("Not on 3DS").click();
    h.run();
    assert!(h.state().e.platforms.n3ds.as_ref().unwrap().disabled);
    h.get_by_label("Hidden on PC").click();
    h.run();
    assert_eq!(
        h.state().e.platforms.pc.as_ref().unwrap().visible,
        Some(false)
    );
    // A scale of its own, then back.
    h.get_by_label("Scale on N64").click();
    h.run();
    assert_eq!(
        h.state().e.platforms.n64.as_ref().unwrap().scale,
        Some([1.0; 3])
    );
    h.get_by_label("Scale on N64").click();
    h.run();
    assert_eq!(h.state().e.platforms.n64.as_ref().unwrap().scale, None);

    // Unticking "Hidden on PC" leaves a no-op variant, which is pruned.
    h.get_by_label("Hidden on PC").click();
    h.run();
    assert!(
        h.state().e.platforms.pc.is_none(),
        "same as written: no block"
    );
    // What the inspector edited is what the file stores.
    let text = ron::to_string(&h.state().e).unwrap();
    let back: trino_scene::Entity = ron::from_str(&text).unwrap();
    assert_eq!(back, h.state().e);
    assert!(h.state().changes >= 6);
}

/// The three columns side by side in a wide inspector, stacked in a narrow
/// one — and in both, nothing runs off the panel or overlaps.
#[test]
fn platforms_tab_fits_wide_and_narrow_inspectors() {
    for width in [240.0, 300.0, 520.0, 900.0] {
        let mut h = platforms_harness(width);
        // Fill every column so the longest labels show.
        h.get_by_label("N64 model").click();
        h.run();
        h.get_by_label("models/tree-low-poly-for-consoles").click();
        h.run();
        h.get_by_label("Scale on 3DS").click();
        h.run();
        crate::layout_audit::assert_clean(
            &h,
            eframe::egui::vec2(width, 900.0),
            false,
            &format!("Platforms tab at {width} px"),
        );
        let n64 = h.get_by_label("N64 model").rect();
        let n3ds = h.get_by_label("3DS model").rect();
        if width >= crate::panels::platforms::COLUMNS_MIN_WIDTH + 16.0 {
            assert!((n64.top() - n3ds.top()).abs() < 1.0, "columns at {width}");
        } else {
            assert!(n3ds.top() > n64.bottom(), "stacked at {width}");
        }
    }
}

struct LodState {
    lod: Vec<trino_scene::LodDef>,
    preview: Option<f32>,
    changes: usize,
    fonts: bool,
}

fn lod_harness(width: f32, lod: Vec<trino_scene::LodDef>) -> Harness<'static, LodState> {
    use crate::panels::platforms::ModelInfo;
    let models: &'static [ModelInfo] = Box::leak(Box::new([
        ModelInfo {
            logical: "models/tree".into(),
            verts: 400,
            tris: 600,
        },
        ModelInfo {
            logical: "models/tree-light".into(),
            verts: 60,
            tris: 80,
        },
    ]));
    let impostors: &'static [String] = Box::leak(Box::new(["impostors/tree".to_string()]));
    let mut h = Harness::builder()
        .with_size(eframe::egui::vec2(width, 700.0))
        .build_ui_state(
            move |ui, s: &mut LodState| {
                if !s.fonts {
                    crate::theme::install(ui.ctx());
                    s.fonts = true;
                    return;
                }
                let choices = crate::panels::lod::Choices {
                    models,
                    impostors,
                    base: models.first(),
                };
                if crate::panels::lod::ui(ui, &mut s.lod, &choices, &mut s.preview) {
                    s.changes += 1;
                }
            },
            LodState {
                lod,
                preview: None,
                changes: 0,
                fonts: false,
            },
        );
    h.run();
    h
}

/// The LOD chain editor: add levels, make one an impostor, skip it on the
/// N64, give the N64 its own cull distance, and preview a distance.
#[test]
fn lod_panel_builds_a_chain_with_per_platform_distances() {
    let mut h = lod_harness(320.0, Vec::new());
    h.get_by_label_contains("Add level").click();
    h.run();
    h.get_by_label_contains("Add level").click();
    h.run();
    assert_eq!(h.state().lod.len(), 2);
    assert_eq!(
        (h.state().lod[0].distance, h.state().lod[1].distance),
        (15.0, 30.0)
    );
    // Level 1: the light mesh; level 2: the impostor.
    h.get_by_label("LOD 1 draws").click();
    h.run();
    h.get_by_label_contains("models/tree-light").click();
    h.run();
    h.get_by_label("LOD 2 draws").click();
    h.run();
    h.get_by_label_contains(" impostors/tree").click();
    h.run();
    let s = h.state();
    assert_eq!(s.lod[0].path.as_deref(), Some("models/tree-light"));
    assert_eq!(s.lod[1].impostor.as_deref(), Some("impostors/tree"));
    assert!(s.lod[1].path.is_none());
    // The impostor never on the N64.
    h.get_by_label("LOD 2 on N64").click();
    h.run();
    h.get_by_label("never").click();
    h.run();
    assert_eq!(
        h.state().lod[1].platforms.n64,
        Some(trino_scene::LodAt::Never)
    );
    assert_eq!(h.state().lod[1].distance_on(trino_scene::Target::N64), None);
    assert_eq!(
        h.state().lod[1].distance_on(trino_scene::Target::N3ds),
        Some(30.0)
    );
    // Costs per level.
    assert!(h.query_by_label_contains("80 tris").is_some());
    assert!(h.query_by_label_contains("impostor · 2 tris").is_some());

    // Preview: on, then the label says which level draws there.
    h.get_by_label("Preview LOD").click();
    h.run();
    assert_eq!(h.state().preview, Some(0.0));
    h.state_mut().preview = Some(31.0);
    h.run();
    assert!(
        h.query_by_label_contains("at 31.0 m: LOD 2 — impostor")
            .is_some(),
        "the preview names the impostor level"
    );
    h.state_mut().preview = Some(20.0);
    h.run();
    assert!(h.query_by_label_contains("at 20.0 m: LOD 1").is_some());
    h.state_mut().preview = Some(5.0);
    h.run();
    assert!(
        h.query_by_label_contains("the full mesh (600 tris)")
            .is_some()
    );
    // The edited chain is what the scene file stores.
    let text = ron::to_string(&h.state().lod).unwrap();
    let back: Vec<trino_scene::LodDef> = ron::from_str(&text).unwrap();
    assert_eq!(back, h.state().lod);
    assert!(h.state().changes >= 5);
}

#[test]
fn lod_and_impostor_panels_fit_narrow_inspectors() {
    let chain = vec![
        trino_scene::LodDef {
            distance: 12.0,
            path: Some("models/tree-light".into()),
            ..Default::default()
        },
        trino_scene::LodDef {
            distance: 30.0,
            impostor: Some("impostors/tree".into()),
            platforms: trino_scene::LodPlatforms {
                n64: Some(trino_scene::LodAt::At(45.0)),
                ..Default::default()
            },
            ..Default::default()
        },
    ];
    for width in [260.0, 320.0, 480.0] {
        let mut h = lod_harness(width, chain.clone());
        h.state_mut().preview = Some(25.0);
        h.run();
        crate::layout_audit::assert_clean(
            &h,
            eframe::egui::vec2(width, 700.0),
            false,
            &format!("LOD panel at {width} px"),
        );
    }
    struct ImpState {
        def: trino_asset_pipeline::impostor::ImpostorDef,
        fonts: bool,
    }
    for width in [260.0, 320.0] {
        let mut h = Harness::builder()
            .with_size(eframe::egui::vec2(width, 400.0))
            .build_ui_state(
                |ui, s: &mut ImpState| {
                    if !s.fonts {
                        crate::theme::install(ui.ctx());
                        s.fonts = true;
                        return;
                    }
                    crate::panels::impostor::settings_editor(ui, &mut s.def);
                },
                ImpState {
                    def: trino_asset_pipeline::impostor::ImpostorDef::new("models/tree"),
                    fonts: false,
                },
            );
        h.run();
        crate::layout_audit::assert_clean(
            &h,
            eframe::egui::vec2(width, 400.0),
            false,
            &format!("impostor settings at {width} px"),
        );
        // The N64 tile follows the PC tile until it is set.
        assert!(
            h.query_by_label_contains("N64 64x32 CI4 fits TMEM")
                .is_some()
        );
        h.state_mut().def.views = 4;
        h.run();
        assert!(
            h.query_by_label_contains("N64 64x64 CI4 fits TMEM")
                .is_some()
        );
        h.state_mut().def.n64_tile = Some(64);
        h.run();
        assert!(h.query_by_label_contains("past TMEM!").is_some());
    }
}

struct PfxState {
    def: trino_asset_pipeline::particles::ParticlesDef,
    changes: usize,
    fonts: bool,
}

fn pfx_harness(width: f32) -> Harness<'static, PfxState> {
    let sprites: &'static [String] = Box::leak(Box::new([
        "sprites/spark".to_string(),
        "sprites/smoke-puff-with-a-long-name".to_string(),
    ]));
    let mut h = Harness::builder()
        .with_size(eframe::egui::vec2(width, 900.0))
        .build_ui_state(
            move |ui, s: &mut PfxState| {
                if !s.fonts {
                    crate::theme::install(ui.ctx());
                    s.fonts = true;
                    return;
                }
                if crate::panels::particles::editor(ui, &mut s.def, sprites) {
                    s.changes += 1;
                }
            },
            PfxState {
                def: trino_asset_pipeline::particles::ParticlesDef::new("sprites/spark"),
                changes: 0,
                fonts: false,
            },
        );
    h.run();
    h
}

/// The particle editor: a sprite, the caps per platform (own cap, off), and
/// what the file stores; it fits narrow inspectors.
#[test]
fn particle_editor_sets_fields_and_per_platform_caps() {
    use trino_asset_pipeline::Platform;
    let mut h = pfx_harness(340.0);
    h.get_by_role(Role::ComboBox).click();
    h.run();
    h.get_by_label("sprites/smoke-puff-with-a-long-name")
        .click();
    h.run();
    assert_eq!(h.state().def.sprite, "sprites/smoke-puff-with-a-long-name");
    h.get_by_label("Own cap on N64").click();
    h.run();
    assert_eq!(h.state().def.cap(Platform::N64), 32, "starts at the max");
    h.state_mut().def.platforms.n64.as_mut().unwrap().max = Some(6);
    h.run();
    assert!(
        h.query_by_label_contains("N64: up to 6 · 12 tris")
            .is_some()
    );
    h.get_by_label("Off on 3DS").click();
    h.run();
    assert_eq!(h.state().def.cap(Platform::N3ds), 0);
    assert!(h.query_by_label_contains("3DS: off").is_some());
    // Unticking leaves no platform block.
    h.get_by_label("Off on 3DS").click();
    h.run();
    assert!(h.state().def.platforms.n3ds.is_none());
    let text = h.state().def.to_ron();
    assert_eq!(
        trino_asset_pipeline::particles::ParticlesDef::from_ron(&text).unwrap(),
        h.state().def
    );
    assert!(h.state().changes >= 3);
    for width in [260.0, 340.0, 480.0] {
        let h = pfx_harness(width);
        crate::layout_audit::assert_clean(
            &h,
            eframe::egui::vec2(width, 900.0),
            false,
            &format!("particle editor at {width} px"),
        );
    }
}
