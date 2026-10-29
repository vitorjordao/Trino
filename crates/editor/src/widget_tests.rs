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
use crate::panels::inspector::{material_editor, primitive_editor, water_editor};
use crate::theme::icon;

struct MatState {
    def: MaterialDef,
    changes: usize,
}

/// The material's water section: off by default, on with the engine's
/// defaults and every slider, off again; a nudged slider reports a change
/// and the material file round-trips it.
#[test]
fn water_editor_turns_a_surface_on_and_edits_it() {
    let mut h = Harness::new_ui_state(
        |ui, s: &mut MatState| {
            if water_editor(ui, &mut s.def.water) {
                s.changes += 1;
            }
        },
        MatState {
            def: MaterialDef::default(),
            changes: 0,
        },
    );
    h.run();
    assert!(h.query_by_label("Wave height").is_none(), "off: no sliders");
    h.get_by_label("Water surface").click();
    h.run();
    assert_eq!(h.state().def.water, Some(trino_scene::WaterDef::default()));
    for label in [
        "Flow u",
        "Flow v",
        "Detail flow u",
        "Detail flow v",
        "Wave height",
        "Wave frequency",
        "Ripples",
    ] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    let sliders: Vec<_> = h.get_all_by_role(Role::Slider).collect();
    assert_eq!(sliders.len(), 7);
    // Keyboard on the wave height slider (the fifth): up a step.
    sliders[4].focus();
    h.run();
    h.key_press(eframe::egui::Key::ArrowRight);
    h.run();
    let w = h.state().def.water.unwrap();
    assert!(
        w.wave_amplitude > trino_scene::WaterDef::default().wave_amplitude,
        "{w:?}"
    );
    let def = &h.state().def;
    assert_eq!(&MaterialDef::from_ron(&def.to_ron()).unwrap(), def);
    h.get_by_label("Water surface").click();
    h.run();
    assert_eq!(h.state().def.water, None);
    assert!(h.state().changes >= 3);
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
    h.get_all_by_role(Role::ComboBox).next().unwrap().click();
    h.run();
    h.get_by_label("sprites/planks").click();
    h.run();
    assert_eq!(h.state().def.texture.as_deref(), Some("sprites/planks"));
    h.get_all_by_role(Role::ComboBox).next().unwrap().click();
    h.run();
    h.get_by_label("None (model default / vertex colors)")
        .click();
    h.run();
    assert_eq!(h.state().def.texture, None);
    // Filter, and a detail layer from the second combo box.
    h.get_by_label("Point").click();
    h.run();
    assert_eq!(h.state().def.filter, trino_scene::FilterDef::Point);
    h.get_all_by_role(Role::ComboBox).nth(1).unwrap().click();
    h.run();
    h.get_by_label("sprites/brick").click();
    h.run();
    let d = h.state().def.detail.clone().expect("a detail layer");
    assert_eq!((d.texture.as_str(), d.scale_log2), ("sprites/brick", 2));
    assert!(h.state().changes >= 7, "every edit reports a change");
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
    platforms_harness_sized(width, 900.0)
}

fn platforms_harness_sized(width: f32, height: f32) -> Harness<'static, PlatState> {
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
        .with_size(eframe::egui::vec2(width, height))
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
    // The Web follows the PC (its column shows the PC's hidden flag)
    // until it says otherwise: visible there is a Web variant of its own.
    h.get_by_label("Hidden on Web").click();
    h.run();
    assert_eq!(
        h.state().e.platforms.web,
        Some(trino_scene::Variant::default()),
        "the Web keeps an empty variant: as written, not as on PC"
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
    assert!(
        h.state().e.platforms.web.is_none(),
        "with nothing on the PC to override, the Web's empty variant goes too"
    );
    // What the inspector edited is what the file stores.
    let text = ron::to_string(&h.state().e).unwrap();
    let back: trino_scene::Entity = ron::from_str(&text).unwrap();
    assert_eq!(back, h.state().e);
    assert!(h.state().changes >= 6);
}

/// The four columns side by side in a wide inspector, stacked in a narrow
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
            &mut h,
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

/// The consoles' tiers (issue #54): collapsed under their console until
/// "Tiers" shows one; its column starts from the console's variant, says
/// it follows it, writes a variant of its own, and goes back to following.
#[test]
fn platform_tier_columns_follow_their_console_until_they_have_their_own() {
    use trino_scene::Target;
    let mut h = platforms_harness(520.0);
    assert!(
        h.query_by_label("N64+Pak model").is_none(),
        "collapsed by default"
    );
    // An N64 variant: the low tree.
    h.get_by_label("N64 model").click();
    h.run();
    h.get_by_label("models/tree-low-poly-for-consoles").click();
    h.run();
    h.get_by_label("Show N64 + Expansion Pak").click();
    h.run();
    assert!(h.query_by_label("N64+Pak model").is_some());
    assert!(
        h.query_by_label("New 3DS model").is_none(),
        "one switch per tier"
    );
    assert!(h.query_by_label("as on N64").is_some(), "follows the N64");
    assert_eq!(
        h.state().e.platforms.get(Target::N64Expanded),
        h.state().e.platforms.n64.as_ref()
    );
    // The Pak's RAM affords the full tree: a variant of its own, the
    // N64's kept.
    h.get_by_label("N64+Pak model").click();
    h.run();
    h.get_by_label("models/tree").click();
    h.run();
    let p = &h.state().e.platforms;
    assert_eq!(
        p.n64_expanded.as_ref().unwrap().model.as_deref(),
        Some("models/tree")
    );
    assert_eq!(
        p.n64.as_ref().unwrap().model.as_deref(),
        Some("models/tree-low-poly-for-consoles")
    );
    assert!(h.query_by_label("as on N64").is_none());
    assert!(h.query_by_label("own").is_some());
    // Hidden on the N64 only: the Pak keeps its own look.
    h.get_by_label("Hidden on N64").click();
    h.run();
    assert_eq!(
        h.state().e.platforms.n64.as_ref().unwrap().visible,
        Some(false)
    );
    assert_eq!(
        h.state().e.platforms.n64_expanded.as_ref().unwrap().visible,
        None
    );
    // What the inspector edited is what the file stores.
    let text = ron::to_string(&h.state().e).unwrap();
    assert!(text.contains("n64_expanded"));
    let back: trino_scene::Entity = ron::from_str(&text).unwrap();
    assert_eq!(back, h.state().e);
    // Back to the N64 look (the N64's reset, then the tier's): the tier
    // follows again and its column stays shown.
    let reset = h
        .get_all_by_label(icon::ARROW_COUNTER_CLOCKWISE)
        .nth(1)
        .expect("the tier's reset")
        .rect();
    h.get_all_by_label(icon::ARROW_COUNTER_CLOCKWISE)
        .find(|n| n.rect() == reset)
        .unwrap()
        .click();
    h.run();
    assert!(h.state().e.platforms.n64_expanded.is_none());
    assert!(h.query_by_label("as on N64").is_some());
    assert!(h.query_by_label("N64+Pak model").is_some());
}

/// With both tiers shown, the Platforms tab still fits: six columns in a
/// wide inspector, stacked blocks in a narrow one, the tier right after its
/// console either way.
#[test]
fn platform_tier_columns_fit_wide_and_narrow_inspectors() {
    use crate::panels::platforms::{COLUMNS_MIN_WIDTH, TIER_COLUMN_WIDTH};
    for width in [260.0, 340.0, 420.0, 520.0, 640.0] {
        let mut h = platforms_harness_sized(width, 1500.0);
        h.get_by_label("Show N64 + Expansion Pak").click();
        h.run();
        h.get_by_label("Show New 3DS").click();
        h.run();
        h.get_by_label("Scale on New 3DS").click();
        h.run();
        assert_eq!(
            h.state().e.platforms.n3ds_new.as_ref().unwrap().scale,
            Some([1.0; 3])
        );
        crate::layout_audit::assert_clean(
            &mut h,
            eframe::egui::vec2(width, 1500.0),
            false,
            &format!("Platforms tab with tiers at {width} px"),
        );
        let n64 = h.get_by_label("N64 model").rect();
        let pak = h.get_by_label("N64+Pak model").rect();
        let n3ds = h.get_by_label("3DS model").rect();
        if width >= COLUMNS_MIN_WIDTH + 2.0 * TIER_COLUMN_WIDTH + 16.0 {
            assert!((n64.top() - pak.top()).abs() < 1.0, "columns at {width}");
            assert!(n64.left() < pak.left() && pak.left() < n3ds.left());
        } else {
            assert!(pak.top() > n64.bottom(), "stacked at {width}");
            assert!(n3ds.top() > pak.bottom(), "the Pak before the 3DS");
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
            &mut h,
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
            &mut h,
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

/// A LOD level's start on a tier (issue #54): hidden until "Tiers" shows
/// it, then "=N64" (the console's start) until it has its own.
#[test]
fn lod_tier_rows_follow_their_console_until_they_have_their_own() {
    use trino_scene::{LodAt, Target};
    let chain = vec![trino_scene::LodDef {
        distance: 30.0,
        impostor: Some("impostors/tree".into()),
        platforms: trino_scene::LodPlatforms {
            n64: Some(LodAt::At(45.0)),
            ..Default::default()
        },
        ..Default::default()
    }];
    let mut h = lod_harness(320.0, chain.clone());
    assert!(h.query_by_label("LOD 1 on N64+Pak").is_none());
    h.get_by_label("Show N64 + Expansion Pak").click();
    h.run();
    assert!(h.query_by_label("LOD 1 on New 3DS").is_none());
    assert_eq!(
        h.state().lod[0].distance_on(Target::N64Expanded),
        Some(45.0),
        "as on the N64"
    );
    // The Pak skips the impostor: never there, the N64 keeps its 45 m.
    h.get_by_label("LOD 1 on N64+Pak").click();
    h.run();
    h.get_by_label("never").click();
    h.run();
    let level = &h.state().lod[0];
    assert_eq!(level.platforms.n64_expanded, Some(LodAt::Never));
    assert_eq!(level.platforms.n64, Some(LodAt::At(45.0)));
    assert_eq!(level.distance_on(Target::N64Expanded), None);
    let text = ron::to_string(&h.state().lod).unwrap();
    assert!(text.contains("n64_expanded"));
    assert!(h.state().changes >= 1);
    // Both tiers shown, in narrow and wide inspectors.
    for width in [260.0, 320.0, 480.0, 720.0] {
        let mut h = lod_harness(width, chain.clone());
        h.get_by_label("Show N64 + Expansion Pak").click();
        h.run();
        h.get_by_label("Show New 3DS").click();
        h.run();
        crate::layout_audit::assert_clean(
            &mut h,
            eframe::egui::vec2(width, 700.0),
            false,
            &format!("LOD panel with tiers at {width} px"),
        );
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
    h.get_all_by_role(Role::ComboBox).next().unwrap().click();
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
        let mut h = pfx_harness(width);
        crate::layout_audit::assert_clean(
            &mut h,
            eframe::egui::vec2(width, 900.0),
            false,
            &format!("particle editor at {width} px"),
        );
    }
}

/// A particle cap per tier (issue #54): the New 3DS row shows under
/// "Tiers", takes the 3DS's cap until it has its own, and writes its own.
#[test]
fn particle_tier_caps_follow_their_console_until_they_have_their_own() {
    use trino_asset_pipeline::particles::PlatformCap;
    use trino_scene::Target;
    let mut h = pfx_harness(340.0);
    h.state_mut().def.platforms.n3ds = Some(PlatformCap {
        max: Some(10),
        disabled: false,
    });
    h.run();
    assert!(h.query_by_label("Off on New 3DS").is_none());
    h.get_by_label("Show New 3DS").click();
    h.run();
    assert!(
        h.query_by_label_contains("New 3DS (as on 3DS): up to 10")
            .is_some()
    );
    assert_eq!(h.state().def.cap(Target::N3dsNew), 10);
    // Off on the New 3DS only.
    h.get_by_label("Off on New 3DS").click();
    h.run();
    let def = &h.state().def;
    assert_eq!(def.cap(Target::N3dsNew), 0);
    assert_eq!(def.cap(Target::N3ds), 10);
    assert!(def.platforms.n3ds_new.unwrap().disabled);
    assert!(h.query_by_label_contains("New 3DS: off").is_some());
    let text = h.state().def.to_ron();
    assert!(text.contains("n3ds_new"));
    assert_eq!(
        trino_asset_pipeline::particles::ParticlesDef::from_ron(&text).unwrap(),
        h.state().def
    );
    for width in [260.0, 340.0, 480.0] {
        let mut h = pfx_harness(width);
        h.get_by_label("Show N64 + Expansion Pak").click();
        h.run();
        h.get_by_label("Show New 3DS").click();
        h.run();
        crate::layout_audit::assert_clean(
            &mut h,
            eframe::egui::vec2(width, 900.0),
            false,
            &format!("particle editor with tiers at {width} px"),
        );
    }
}

struct AxisState {
    cam: crate::gizmo::OrbitCam,
    events: Vec<crate::gizmo::AxisEvent>,
    fonts: bool,
}

const AXIS_CENTER: eframe::egui::Pos2 = eframe::egui::pos2(120.0, 120.0);

fn axis_proj(cam: &crate::gizmo::OrbitCam) -> crate::gizmo::Proj {
    let rect = eframe::egui::Rect::from_min_size(
        eframe::egui::Pos2::ZERO,
        eframe::egui::vec2(240.0, 240.0),
    );
    crate::gizmo::Proj::new(&cam.camera(), (240, 240), rect)
}

/// The orientation widget, driven with the pointer: a tip click snaps (and
/// the app's camera flips on a second one), a drag orbits, the centre and
/// the label switch the projection.
#[test]
fn axis_widget_snaps_orbits_and_toggles_the_projection() {
    use crate::gizmo::{AxisEvent, OrbitCam, axis_tips};
    let mut h = Harness::builder()
        .with_size(eframe::egui::vec2(240.0, 240.0))
        .build_ui_state(
            |ui, s: &mut AxisState| {
                if !s.fonts {
                    crate::theme::install(ui.ctx());
                    s.fonts = true;
                    return;
                }
                let proj = axis_proj(&s.cam);
                if let Some(e) = crate::gizmo::axis_widget(ui, &proj, AXIS_CENTER, s.cam.ortho) {
                    s.events.push(e);
                }
            },
            AxisState {
                cam: OrbitCam::default(),
                events: Vec::new(),
                fonts: false,
            },
        );
    h.run();
    assert!(h.query_by_label("View orientation").is_some());
    let tip = |h: &Harness<'_, AxisState>, idx: usize| {
        axis_tips(&axis_proj(&h.state().cam).view, AXIS_CENTER)
            .into_iter()
            .find(|t| t.0 == idx)
            .unwrap()
            .1
    };
    let click = |h: &mut Harness<'_, AxisState>, p: eframe::egui::Pos2| {
        h.hover_at(p);
        h.run();
        h.drag_at(p);
        h.run();
        h.drop_at(p);
        h.run();
    };

    // Click +X: snap to it.
    let p = tip(&h, 0);
    click(&mut h, p);
    assert_eq!(h.state().events, vec![AxisEvent::Snap(0)]);

    // Drag across the disc: orbit events adding up to the pointer's travel.
    h.state_mut().events.clear();
    let start = AXIS_CENTER + eframe::egui::vec2(-20.0, 22.0);
    h.hover_at(start);
    h.run();
    h.drag_at(start);
    h.run();
    for k in 1..=6 {
        h.hover_at(start + eframe::egui::vec2(5.0 * k as f32, 0.0));
        h.run();
    }
    h.drop_at(start + eframe::egui::vec2(30.0, 0.0));
    h.run();
    let orbit: eframe::egui::Vec2 = h
        .state()
        .events
        .iter()
        .map(|e| match e {
            AxisEvent::Orbit(d) => *d,
            other => panic!("a drag only orbits: {other:?}"),
        })
        .fold(eframe::egui::Vec2::ZERO, |a, b| a + b);
    assert!(orbit.x > 20.0 && orbit.y.abs() < 1e-3, "{orbit:?}");

    // The centre and the label both switch the projection.
    h.state_mut().events.clear();
    click(&mut h, AXIS_CENTER);
    assert_eq!(h.state().events, vec![AxisEvent::ToggleOrtho]);
    h.state_mut().cam.ortho = true;
    h.run();
    h.get_by_label("Projection: Ortho (5)").click();
    h.run();
    assert_eq!(
        h.state().events,
        vec![AxisEvent::ToggleOrtho, AxisEvent::ToggleOrtho]
    );
}

/// Framing a big object backs the Scene view's eye off; its fog follows,
/// so the framed object isn't painted the fog's color. Near, the scene's
/// fog is unchanged, and no fog stays no fog.
#[test]
fn scene_view_fog_follows_a_far_orbit() {
    use crate::viewport::scene_view_fog;
    use trino_core::{Color, Fog};
    let fog = Fog {
        color: Color::rgb(130, 180, 230),
        near: 16.0,
        far: 42.0,
    };
    let near = scene_view_fog(fog, 8.0);
    assert_eq!((near.near, near.far), (16.0, 42.0));
    let far = scene_view_fog(fog, 45.0);
    assert_eq!((far.near, far.far), (49.0, 75.0));
    assert!(far.far > 45.0, "the target stays clear of full fog");
    let none = Fog { far: 0.0, ..fog };
    assert_eq!(scene_view_fog(none, 45.0).far, 0.0);
}
