//! Inspector: edits whatever is selected — an entity (components), an asset
//! (model / fake object / material / prefab / sprite) — or, with nothing
//! selected, the scene's environment (camera, light, fog).
//!
//! Entity edits mutate the document directly (undo coalesces them);
//! material and fake-object edits write their files immediately, so the
//! change is live everywhere (other scenes, the running game's next bake,
//! an AI agent reading the file).

use eframe::egui::{self, RichText, Vec2};
use trino_asset_pipeline::{Pivot, Primitive, Shape};
use trino_core::Caps;
use trino_scene::{
    AnimComponent, Entity, MaterialDef, ModelComponent, Override, PrefabInstance, ShadingDef,
    SpriteComponent,
};

use crate::app::{App, AssetSel, Cmd, card};
use crate::dialogs::Dialog;
use crate::theme::{self, c, icon};
use crate::viewport::AssetPayload;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    // Leave room for the scroll bar so card borders are never clipped.
    ui.set_max_width(ui.available_width() - 10.0);
    if let Some(a) = app.asset_sel.clone() {
        match a {
            AssetSel::Model(l) => model_asset(app, ui, &l),
            AssetSel::Material(l) => material_asset(app, ui, &l),
            AssetSel::Prefab(l) => prefab_asset(app, ui, &l),
            AssetSel::Sprite(l) => sprite_asset(app, ui, &l),
        }
        return;
    }
    match app.doc.selected.clone() {
        Some(path) if app.doc.scene.get(&path).is_some() => entity(app, ui, &path),
        _ => scene_settings(app, ui),
    }
}

// ---------------------------------------------------------------- entity --

fn entity(app: &mut App, ui: &mut egui::Ui, path: &[usize]) {
    let model_names: Vec<(String, bool)> = app
        .project
        .models
        .values()
        .map(|m| (m.logical.clone(), m.is_proxy()))
        .collect();
    let sprite_names: Vec<String> = app.project.sprites.keys().cloned().collect();
    let material_names: Vec<String> = app.lib.materials.keys().cloned().collect();
    let prefab_names: Vec<String> = app.lib.prefabs.keys().cloned().collect();
    let is_prefab_doc = app.doc.is_prefab();

    let App {
        doc,
        project,
        lib,
        cmds,
        material_thumbs,
        ..
    } = app;
    let Some(e) = doc.scene.get_mut(path) else {
        return;
    };

    // Header: icon, name, visibility.
    let (glyph, color) = crate::panels::hierarchy::entity_glyph(e);
    card().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(glyph).size(22.0).color(color));
            ui.add(
                egui::TextEdit::singleline(&mut e.name)
                    .font(egui::TextStyle::Heading)
                    .desired_width(ui.available_width() - 34.0),
            );
            let eye = if e.visible {
                icon::EYE
            } else {
                icon::EYE_SLASH
            };
            if theme::icon_button(ui, eye, "Visible").clicked() {
                e.visible = !e.visible;
            }
        });
        ui.add_space(4.0);
        // Tags as chips.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(icon::TAG).color(c::TEXT_FAINT));
            let mut remove = None;
            for (i, t) in e.tags.iter().enumerate() {
                let r = theme::pill(
                    ui,
                    &format!("{t}  {}", icon::X),
                    c::ACCENT_TEXT,
                    c::ACCENT_SOFT,
                );
                if r.interact(egui::Sense::click())
                    .on_hover_text("Remove tag")
                    .clicked()
                {
                    remove = Some(i);
                }
            }
            if let Some(i) = remove {
                e.tags.remove(i);
            }
            let id = ui.id().with("new-tag");
            let mut buf = ui.data_mut(|d| d.get_temp::<String>(id).unwrap_or_default());
            let r = ui.add(
                egui::TextEdit::singleline(&mut buf)
                    .hint_text("+ tag")
                    .desired_width(70.0),
            );
            if r.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !buf.trim().is_empty()
            {
                e.tags.push(buf.trim().to_string());
                buf.clear();
            }
            ui.data_mut(|d| d.insert_temp(id, buf));
        });
    });
    ui.add_space(8.0);

    // Transform.
    theme::component(
        ui,
        "c-transform",
        icon::ARROWS_OUT_CARDINAL,
        c::TEXT,
        "Transform",
        |ui| {
            let t = &mut e.transform;
            theme::vec3_row(ui, "Position", &mut t.pos, 0.05, "");
            theme::vec3_row(ui, "Rotation", &mut t.rot, 0.5, "°");
            theme::vec3_row(ui, "Scale", &mut t.scale, 0.01, "");
            ui.horizontal(|ui| {
                ui.add_space(64.0);
                if ui
                    .small_button(format!("{} Reset", icon::ARROW_COUNTER_CLOCKWISE))
                    .clicked()
                {
                    *t = trino_scene::Transform::default();
                }
                if ui
                    .small_button("Uniform scale")
                    .on_hover_text("Set Y and Z to X")
                    .clicked()
                {
                    t.scale = [t.scale[0]; 3];
                }
            });
        },
    );

    // Model.
    let mut remove_model = false;
    if let Some(m) = &mut e.model {
        let asset = project.models.get(&m.path);
        let clips: Vec<String> = asset.map(|a| a.clips.clone()).unwrap_or_default();
        let stats = asset.map(|a| (a.tris, a.joints, a.is_proxy()));
        theme::component(ui, "c-model", icon::CUBE, c::MODEL, "Model", |ui| {
            theme::prop(ui, "Mesh", |ui| {
                egui::ComboBox::from_id_salt("model-path")
                    .selected_text(&m.path)
                    .width(ui.available_width() - 30.0)
                    .height(320.0)
                    .show_ui(ui, |ui| {
                        for (name, proxy) in &model_names {
                            let g = if *proxy { icon::GHOST } else { icon::CUBE };
                            if ui
                                .selectable_label(m.path == *name, format!("{g}  {name}"))
                                .clicked()
                            {
                                m.path = name.clone();
                                m.anim = None;
                            }
                        }
                    });
                if theme::icon_button(ui, icon::ARROW_SQUARE_OUT, "Show asset").clicked() {
                    cmds.push(Cmd::SelectAsset(AssetSel::Model(m.path.clone())));
                }
            });
            if let Some((tris, joints, proxy)) = stats {
                ui.horizontal(|ui| {
                    ui.add_space(64.0);
                    ui.label(
                        RichText::new(format!("{} {tris} tris", icon::TRIANGLE))
                            .size(11.5)
                            .color(c::TEXT_FAINT),
                    );
                    if joints > 0 {
                        ui.label(
                            RichText::new(format!("{} {joints} joints", icon::BONE))
                                .size(11.5)
                                .color(c::TEXT_FAINT),
                        );
                    }
                    if proxy {
                        theme::pill(ui, &format!("{} fake", icon::GHOST), c::TEXT_WEAK, c::BG3);
                    }
                });
            } else {
                ui.label(
                    RichText::new(format!(
                        "{} not baked (missing from the manifest?)",
                        icon::WARNING
                    ))
                    .color(c::WARN),
                );
            }

            // Material picker + inline editor.
            theme::prop(ui, "Material", |ui| {
                let current = m.material.clone().unwrap_or_else(|| "Default".into());
                egui::ComboBox::from_id_salt("material")
                    .selected_text(current)
                    .width(ui.available_width() - 30.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(m.material.is_none(), "Default")
                            .clicked()
                        {
                            m.material = None;
                        }
                        for name in &material_names {
                            let sel = m.material.as_deref() == Some(name.as_str());
                            ui.horizontal(|ui| {
                                if let Some((_, tex)) = material_thumbs.get(name) {
                                    ui.add(egui::Image::new((tex.id(), Vec2::splat(18.0))));
                                }
                                if ui.selectable_label(sel, name).clicked() {
                                    m.material = Some(name.clone());
                                }
                            });
                        }
                    });
                if theme::icon_button(ui, icon::PLUS, "New material for this entity").clicked() {
                    cmds.push(Cmd::Open(Dialog::new_material()));
                }
            });
            if let Some(name) = m.material.clone()
                && let Some(def) = lib.materials.get_mut(&name)
            {
                ui.indent("mat-inline", |ui| {
                    egui::CollapsingHeader::new(
                        RichText::new(format!("{}  Edit {name}", icon::PALETTE)).color(c::MATERIAL),
                    )
                    .id_salt("mat-inline-h")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("Shared — edits apply to every user of this material.")
                                .size(11.5)
                                .color(c::TEXT_FAINT),
                        );
                        if material_editor(ui, def, &sprite_names) {
                            cmds.push(Cmd::SaveMaterial(name.clone()));
                        }
                    });
                });
            }

            // Per-entity tint.
            theme::prop(ui, "Tint", |ui| {
                let mut on = m.tint.is_some();
                if ui
                    .checkbox(&mut on, "")
                    .on_hover_text("Multiply over the material's tint")
                    .changed()
                {
                    m.tint = on.then_some([255, 255, 255, 255]);
                }
                if let Some(t) = &mut m.tint {
                    theme::color_rgba(ui, t);
                }
            });

            // Animation.
            if !clips.is_empty() {
                theme::prop(ui, "Animation", |ui| {
                    let current = m
                        .anim
                        .as_ref()
                        .map(|a| a.clip.clone())
                        .unwrap_or_else(|| "None (bind pose)".into());
                    egui::ComboBox::from_id_salt("clip")
                        .selected_text(format!("{}  {current}", icon::FILM_STRIP))
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(m.anim.is_none(), "None (bind pose)")
                                .clicked()
                            {
                                m.anim = None;
                            }
                            for clip in &clips {
                                let sel = m.anim.as_ref().is_some_and(|a| &a.clip == clip);
                                if ui.selectable_label(sel, clip).clicked() {
                                    m.anim = Some(AnimComponent {
                                        clip: clip.clone(),
                                        speed: 1.0,
                                    });
                                }
                            }
                        });
                });
                if let Some(a) = &mut m.anim {
                    theme::prop(ui, "Speed", |ui| {
                        ui.add(egui::Slider::new(&mut a.speed, 0.0..=4.0).suffix("×"));
                    });
                }
            }
            // Distance LOD: cheaper models (fake objects) or nothing, far away.
            egui::CollapsingHeader::new(
                RichText::new(format!("{}  LOD ({})", icon::STACK, m.lod.len()))
                    .color(c::TEXT_WEAK),
            )
            .id_salt("model-lod")
            .default_open(!m.lod.is_empty())
            .show(ui, |ui| {
                let mut remove = None;
                for (i, level) in m.lod.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut level.distance)
                                .speed(0.25)
                                .range(0.0..=10_000.0)
                                .suffix(" m"),
                        )
                        .on_hover_text("Camera distance where this level starts");
                        let label = level.path.clone().unwrap_or_else(|| "None (cull)".into());
                        egui::ComboBox::from_id_salt(("lod-path", i))
                            .selected_text(label)
                            .width(ui.available_width() - 30.0)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(level.path.is_none(), "None (cull)")
                                    .clicked()
                                {
                                    level.path = None;
                                }
                                for (name, proxy) in &model_names {
                                    let g = if *proxy { icon::GHOST } else { icon::CUBE };
                                    let sel = level.path.as_deref() == Some(name.as_str());
                                    if ui.selectable_label(sel, format!("{g}  {name}")).clicked() {
                                        level.path = Some(name.clone());
                                    }
                                }
                            });
                        if theme::icon_button(ui, icon::X, "Remove level").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    m.lod.remove(i);
                }
                if ui
                    .small_button(format!("{} Add level", icon::PLUS))
                    .on_hover_text("Fake objects make good far levels (Create › Fake object)")
                    .clicked()
                {
                    let next = m.lod.last().map_or(15.0, |l| l.distance + 15.0);
                    m.lod.push(trino_scene::LodDef {
                        distance: next,
                        path: None,
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(64.0);
                if ui
                    .small_button(RichText::new(format!("{} Remove", icon::TRASH)).color(c::ERROR))
                    .clicked()
                {
                    remove_model = true;
                }
            });
        });
    }
    if remove_model {
        e.model = None;
    }

    // Sprite.
    let mut remove_sprite = false;
    if let Some(s) = &mut e.sprite {
        theme::component(
            ui,
            "c-sprite",
            icon::IMAGE,
            c::SPRITE,
            "Sprite (2D, screen space)",
            |ui| {
                theme::prop(ui, "Texture", |ui| {
                    egui::ComboBox::from_id_salt("sprite-path")
                        .selected_text(&s.path)
                        .show_ui(ui, |ui| {
                            for name in &sprite_names {
                                if ui.selectable_label(s.path == *name, name).clicked() {
                                    s.path = name.clone();
                                }
                            }
                        });
                });
                theme::prop(ui, "Tint", |ui| theme::color_rgba(ui, &mut s.tint));
                theme::prop(ui, "Flip", |ui| {
                    ui.checkbox(&mut s.flip_x, "X");
                    ui.checkbox(&mut s.flip_y, "Y");
                });
                ui.label(RichText::new("Position X/Y are game pixels (Y down); rotation uses Z. Visible in the Game view.").size(11.5).color(c::TEXT_FAINT));
                if ui
                    .small_button(RichText::new(format!("{} Remove", icon::TRASH)).color(c::ERROR))
                    .clicked()
                {
                    remove_sprite = true;
                }
            },
        );
    }
    if remove_sprite {
        e.sprite = None;
    }

    // Prefab instance.
    let mut remove_prefab = false;
    if let Some(inst) = &mut e.prefab {
        let expanded = lib
            .expand(&PrefabInstance {
                overrides: vec![],
                ..inst.clone()
            })
            .ok()
            .map(|(r, _)| r);
        theme::component(
            ui,
            "c-prefab",
            icon::PACKAGE,
            c::PREFAB,
            "Prefab instance",
            |ui| {
                theme::prop(ui, "Source", |ui| {
                    egui::ComboBox::from_id_salt("prefab-path")
                        .selected_text(&inst.path)
                        .show_ui(ui, |ui| {
                            for name in &prefab_names {
                                if ui.selectable_label(inst.path == *name, name).clicked() {
                                    inst.path = name.clone();
                                    inst.overrides.clear();
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.add_space(64.0);
                    if !is_prefab_doc
                        && ui
                            .button(format!("{} Edit prefab", icon::PENCIL_SIMPLE))
                            .clicked()
                    {
                        cmds.push(Cmd::EditPrefab(inst.path.clone()));
                    }
                    if ui
                        .button(format!("{} Unpack", icon::LINK_BREAK))
                        .on_hover_text("Make real: copy the prefab into the scene and unlink")
                        .clicked()
                    {
                        cmds.push(Cmd::Unpack(path.to_vec()));
                    }
                });
                if let Some(root) = &expanded {
                    ui.add_space(4.0);
                    theme::section_title(
                        ui,
                        icon::SLIDERS_HORIZONTAL,
                        "Overrides (this instance only)",
                    );
                    override_editor(ui, root, "", inst, &material_names);
                } else {
                    ui.label(
                        RichText::new(format!("{} prefab not found", icon::WARNING)).color(c::WARN),
                    );
                }
                if ui
                    .small_button(
                        RichText::new(format!("{} Remove link", icon::TRASH)).color(c::ERROR),
                    )
                    .clicked()
                {
                    remove_prefab = true;
                }
            },
        );
    }
    if remove_prefab {
        e.prefab = None;
    }

    // Collider.
    let mut remove_collider = false;
    if let Some(col) = &mut e.collider {
        let color = if col.trigger { c::TRIGGER } else { c::COLLIDER };
        let has_model = e.model.is_some();
        theme::component(
            ui,
            "c-collider",
            icon::BOUNDING_BOX,
            color,
            "Collider",
            |ui| {
                use trino_scene::ShapeDef;
                theme::prop(ui, "Shape", |ui| {
                    let label = match col.shape {
                        ShapeDef::Auto => "Auto (model box)",
                        ShapeDef::Box { .. } => "Box",
                        ShapeDef::Sphere { .. } => "Sphere",
                        ShapeDef::Capsule { .. } => "Capsule",
                    };
                    egui::ComboBox::from_id_salt("collider-shape")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            let options = [
                                ("Auto (model box)", ShapeDef::Auto),
                                (
                                    "Box",
                                    ShapeDef::Box {
                                        size: [1.0, 1.0, 1.0],
                                    },
                                ),
                                ("Sphere", ShapeDef::Sphere { radius: 0.5 }),
                                (
                                    "Capsule",
                                    ShapeDef::Capsule {
                                        radius: 0.4,
                                        height: 1.8,
                                    },
                                ),
                            ];
                            for (name, shape) in options {
                                let same = core::mem::discriminant(&col.shape)
                                    == core::mem::discriminant(&shape);
                                if ui.selectable_label(same, name).clicked() && !same {
                                    col.shape = shape;
                                }
                            }
                        });
                });
                match &mut col.shape {
                    ShapeDef::Auto => {
                        if !has_model {
                            ui.label(
                                RichText::new(format!(
                                    "{} Auto needs a Model component (falls back to 1×1×1)",
                                    icon::WARNING
                                ))
                                .color(c::WARN),
                            );
                        }
                    }
                    ShapeDef::Box { size } => {
                        theme::vec3_row(ui, "Size", size, 0.05, "");
                    }
                    ShapeDef::Sphere { radius } => {
                        theme::prop(ui, "Radius", |ui| {
                            ui.add(egui::DragValue::new(radius).speed(0.02).range(0.01..=100.0));
                        });
                    }
                    ShapeDef::Capsule { radius, height } => {
                        theme::prop(ui, "Radius", |ui| {
                            ui.add(egui::DragValue::new(radius).speed(0.02).range(0.01..=100.0));
                        });
                        theme::prop(ui, "Height", |ui| {
                            ui.add(egui::DragValue::new(height).speed(0.02).range(0.01..=100.0));
                        });
                    }
                }
                theme::vec3_row(ui, "Offset", &mut col.offset, 0.05, "");
                theme::prop(ui, "Trigger", |ui| {
                    ui.checkbox(&mut col.trigger, "overlap only (coins, doors, zones)");
                });
                if ui
                    .small_button(RichText::new(format!("{} Remove", icon::TRASH)).color(c::ERROR))
                    .clicked()
                {
                    remove_collider = true;
                }
            },
        );
    }
    if remove_collider {
        e.collider = None;
    }

    // Add component.
    ui.add_space(4.0);
    ui.vertical_centered(|ui| {
        ui.menu_button(
            RichText::new(format!("{}  Add component", icon::PLUS)).family(theme::bold()),
            |ui| {
                if e.model.is_none() && ui.button(format!("{}  Model", icon::CUBE)).clicked() {
                    let first = model_names
                        .first()
                        .map(|(n, _)| n.clone())
                        .unwrap_or_else(|| "models/cube".into());
                    e.model = Some(ModelComponent::new(first));
                }
                if e.sprite.is_none() && ui.button(format!("{}  Sprite", icon::IMAGE)).clicked() {
                    let first = sprite_names
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "sprites/player".into());
                    e.sprite = Some(SpriteComponent::new(first));
                }
                if e.prefab.is_none()
                    && !prefab_names.is_empty()
                    && ui
                        .button(format!("{}  Prefab instance", icon::PACKAGE))
                        .clicked()
                {
                    e.prefab = Some(PrefabInstance::new(prefab_names[0].clone()));
                }
                if e.collider.is_none()
                    && ui
                        .button(format!("{}  Collider", icon::BOUNDING_BOX))
                        .clicked()
                {
                    e.collider = Some(trino_scene::ColliderComponent::default());
                }
            },
        );
    });
    ui.add_space(8.0);
    // Text view (what the file stores) — the AI-facing side of the entity.
    egui::CollapsingHeader::new(RichText::new(format!("{}  RON", icon::CODE)).color(c::TEXT_WEAK))
        .id_salt("entity-ron")
        .show(ui, |ui| {
            let text = ron::ser::to_string_pretty(&*e, ron::ser::PrettyConfig::default())
                .unwrap_or_default();
            ui.add(
                egui::TextEdit::multiline(&mut text.as_str())
                    .code_editor()
                    .desired_width(f32::INFINITY),
            );
            if ui.small_button(format!("{} Copy", icon::COPY)).clicked() {
                ui.ctx().copy_text(text);
            }
        });
}

/// Per-instance overrides: one row per prefab part with a model.
fn override_editor(
    ui: &mut egui::Ui,
    e: &Entity,
    target: &str,
    inst: &mut PrefabInstance,
    materials: &[String],
) {
    let row = |ui: &mut egui::Ui, inst: &mut PrefabInstance| {
        let idx = inst.overrides.iter().position(|o| o.target == target);
        let mut o = idx
            .map(|i| inst.overrides[i].clone())
            .unwrap_or_else(|| Override {
                target: target.to_string(),
                ..Override::default()
            });
        let before = o.clone();
        ui.horizontal(|ui| {
            let label = if target.is_empty() {
                "(root)".to_string()
            } else {
                target.to_string()
            };
            ui.add_sized(
                [110.0, 20.0],
                egui::Label::new(RichText::new(label).size(12.0)).truncate(),
            );
            let mut vis = o.visible.unwrap_or(e.visible);
            if ui.checkbox(&mut vis, "").on_hover_text("Visible").changed() {
                o.visible = (vis != e.visible).then_some(vis);
            }
            if e.model.is_some() {
                let cur = o.material.clone().unwrap_or_else(|| "—".into());
                egui::ComboBox::from_id_salt(("ov-mat", target))
                    .selected_text(cur)
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(o.material.is_none(), "— (prefab's)")
                            .clicked()
                        {
                            o.material = None;
                        }
                        for m in materials {
                            if ui
                                .selectable_label(o.material.as_deref() == Some(m), m)
                                .clicked()
                            {
                                o.material = Some(m.clone());
                            }
                        }
                    });
                let mut on = o.tint.is_some();
                if ui
                    .checkbox(&mut on, "")
                    .on_hover_text("Tint override")
                    .changed()
                {
                    o.tint = on.then_some([255, 255, 255, 255]);
                }
                if let Some(t) = &mut o.tint {
                    theme::color_rgba(ui, t);
                }
            }
        });
        if o != before {
            let empty = o.material.is_none() && o.tint.is_none() && o.visible.is_none();
            match (idx, empty) {
                (Some(i), true) => {
                    inst.overrides.remove(i);
                }
                (Some(i), false) => inst.overrides[i] = o,
                (None, false) => inst.overrides.push(o),
                (None, true) => {}
            }
        }
    };
    if e.model.is_some() || target.is_empty() {
        row(ui, inst);
    }
    for child in &e.children {
        let t = if target.is_empty() {
            child.name.clone()
        } else {
            format!("{target}/{}", child.name)
        };
        override_editor(ui, child, &t, inst, materials);
    }
}

/// Material fields. Returns true on change.
/// `textures`: sprite logical paths offered as material textures.
pub fn material_editor(ui: &mut egui::Ui, def: &mut MaterialDef, textures: &[String]) -> bool {
    let mut changed = false;
    theme::prop(ui, "Tint", |ui| {
        changed |= theme::color_rgba(ui, &mut def.tint)
    });
    theme::prop(ui, "Shading", |ui| {
        for s in ShadingDef::ALL {
            let label = match s {
                ShadingDef::Gouraud => "Smooth",
                ShadingDef::Flat => "Flat",
                ShadingDef::Unlit => "Unlit",
            };
            if ui
                .selectable_label(def.shading == s, label)
                .on_hover_text(s.label())
                .clicked()
            {
                def.shading = s;
                changed = true;
            }
        }
    });
    theme::prop(ui, "Emissive", |ui| {
        changed |= theme::color_rgb(ui, &mut def.emissive)
    });
    theme::prop(ui, "Sides", |ui| {
        changed |= ui.checkbox(&mut def.double_sided, "Double-sided").changed();
    });
    theme::prop(ui, "Texture", |ui| {
        let current = def.texture.clone().unwrap_or_else(|| "None".into());
        egui::ComboBox::from_id_salt("mat-texture")
            .selected_text(current)
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        def.texture.is_none(),
                        "None (model default / vertex colors)",
                    )
                    .clicked()
                {
                    def.texture = None;
                    changed = true;
                }
                for t in textures {
                    let sel = def.texture.as_deref() == Some(t.as_str());
                    if ui.selectable_label(sel, t).clicked() && !sel {
                        def.texture = Some(t.clone());
                        changed = true;
                    }
                }
            })
            .response
            .on_hover_text(
                "Sampled by models with UVs (manifest `texture = ...`). Power-of-two \
                 sizes; on N64 one texture must fit 4 KB TMEM (32x32 RGBA16, 64x64 CI4).",
            );
    });
    changed
}

// ----------------------------------------------------------------- assets --

fn big_thumb(ui: &mut egui::Ui, tex: Option<&egui::TextureHandle>, glyph: &str) {
    ui.vertical_centered(|ui| {
        egui::Frame::new()
            .fill(c::BG0)
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| match tex {
                Some(t) => {
                    ui.add(egui::Image::new((t.id(), Vec2::splat(150.0))));
                }
                None => {
                    ui.add_sized(
                        [150.0, 150.0],
                        egui::Label::new(RichText::new(glyph).size(48.0).color(c::TEXT_FAINT)),
                    );
                }
            });
    });
    ui.add_space(6.0);
}

fn header(ui: &mut egui::Ui, glyph: &str, color: egui::Color32, title: &str, kind: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(glyph).size(20.0).color(color));
        ui.vertical(|ui| {
            ui.label(RichText::new(title).heading());
            ui.label(RichText::new(kind).size(11.5).color(c::TEXT_FAINT));
        });
    });
    ui.add_space(6.0);
}

fn budget_bar(ui: &mut egui::Ui, label: &str, used: f32, budget: f32) {
    let frac = (used / budget).clamp(0.0, 1.0);
    let col = if frac > 0.9 {
        c::ERROR
    } else if frac > 0.6 {
        c::WARN
    } else {
        c::OK
    };
    theme::prop(ui, label, |ui| {
        ui.add(
            egui::ProgressBar::new(frac)
                .desired_width(ui.available_width() - 4.0)
                .fill(col)
                .text(format!("{used:.0} / {budget:.0}")),
        );
    });
}

fn model_asset(app: &mut App, ui: &mut egui::Ui, logical: &str) {
    let Some(m) = app.project.models.get(logical) else {
        theme::empty_state(ui, icon::CUBE, "Model not found", logical);
        return;
    };
    let glyph = if m.is_proxy() {
        icon::GHOST
    } else {
        icon::CUBE
    };
    header(
        ui,
        glyph,
        c::MODEL,
        &m.name,
        if m.is_proxy() {
            "Fake object (placeholder model)"
        } else {
            "Model"
        },
    );
    big_thumb(ui, m.thumb.as_ref(), glyph);
    let (name, source, tris, verts, joints, clips) = (
        m.name.clone(),
        m.source.clone(),
        m.tris,
        m.verts,
        m.joints,
        m.clips.clone(),
    );
    let size = [
        m.max[0] - m.min[0],
        m.max[1] - m.min[1],
        m.max[2] - m.min[2],
    ];
    let mut proxy = m.proxy.clone();

    card().show(ui, |ui| {
        egui::Grid::new("model-stats")
            .num_columns(2)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                for (k, v) in [
                    ("Logical path", logical.to_string()),
                    ("Source", source.clone()),
                    ("Triangles", tris.to_string()),
                    ("Vertices", verts.to_string()),
                    (
                        "Size (m)",
                        format!("{:.2} × {:.2} × {:.2}", size[0], size[1], size[2]),
                    ),
                    ("Joints", joints.to_string()),
                    (
                        "Clips",
                        if clips.is_empty() {
                            "—".into()
                        } else {
                            clips.join(", ")
                        },
                    ),
                ] {
                    ui.label(RichText::new(k).color(c::TEXT_WEAK));
                    ui.add(egui::Label::new(RichText::new(&v).monospace()).truncate())
                        .on_hover_text(&v);
                    ui.end_row();
                }
            });
        ui.add_space(4.0);
        budget_bar(
            ui,
            "N64 tris",
            tris as f32,
            Caps::N64.max_tris_per_frame as f32,
        );
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if theme::primary_button(ui, format!("{}  Add to scene", icon::PLUS)).clicked() {
            app.cmds.push(Cmd::AddAsset(
                AssetPayload::Model(logical.to_string()),
                None,
            ));
        }
        if ui.button(format!("{}  Copy path", icon::COPY)).clicked() {
            ui.ctx().copy_text(logical.to_string());
        }
    });
    ui.add_space(8.0);

    if let Some(p) = &mut proxy {
        theme::component(
            ui,
            "c-proxy",
            icon::GHOST,
            c::TEXT,
            "Fake object parameters",
            |ui| {
                ui.label(
                    RichText::new("Edits rewrite the .prim.ron file and hot-reload everywhere.")
                        .size(11.5)
                        .color(c::TEXT_FAINT),
                );
                if primitive_editor(ui, p) {
                    app.cmds
                        .push(Cmd::SaveProxy(logical.to_string(), p.clone()));
                }
            },
        );
        theme::component(
            ui,
            "c-replace",
            icon::ARROWS_CLOCKWISE,
            c::TEXT,
            "Replace with real art",
            |ui| {
                ui.label(RichText::new("Point this model at a .glb under assets/shared/. Every entity, material and line of code using it stays connected (same handle).").size(11.5).color(c::TEXT_FAINT));
                let id = ui.id().with("retarget");
                let mut buf = ui.data_mut(|d| {
                    d.get_temp::<String>(id)
                        .unwrap_or_else(|| format!("models/{name}.glb"))
                });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut buf)
                            .desired_width(ui.available_width() - 70.0),
                    );
                    if ui.button("Apply").clicked() {
                        app.cmds.push(Cmd::RetargetModel(name.clone(), buf.clone()));
                    }
                });
                ui.data_mut(|d| d.insert_temp(id, buf));
            },
        );
    }
}

/// Fields for a placeholder primitive. Returns true on change.
pub fn primitive_editor(ui: &mut egui::Ui, p: &mut Primitive) -> bool {
    let mut changed = false;
    theme::prop(ui, "Shape", |ui| {
        ui.horizontal_wrapped(|ui| {
            for shape in Shape::all_defaults() {
                let on = std::mem::discriminant(&shape) == std::mem::discriminant(&p.shape);
                let g = crate::panels::hierarchy::shape_glyph(&shape);
                if theme::toggle_icon(
                    ui,
                    on,
                    g,
                    &crate::panels::hierarchy::capitalize(shape.kind()),
                )
                .clicked()
                    && !on
                {
                    p.shape = shape;
                    changed = true;
                }
            }
        });
    });
    let drag = |ui: &mut egui::Ui, v: &mut f32| {
        ui.add(
            egui::DragValue::new(v)
                .speed(0.02)
                .range(0.01..=500.0)
                .suffix(" m"),
        )
        .changed()
    };
    match &mut p.shape {
        Shape::Box { size } | Shape::Ramp { size } => {
            changed |= theme::vec3_row(ui, "Size", size, 0.02, "");
            for v in size.iter_mut() {
                *v = v.max(0.01);
            }
        }
        Shape::Plane { size } => {
            theme::prop(ui, "Size", |ui| {
                changed |= drag(ui, &mut size[0]);
                changed |= drag(ui, &mut size[1]);
            });
        }
        Shape::Sphere { radius } => theme::prop(ui, "Radius", |ui| changed |= drag(ui, radius)),
        Shape::Cylinder { radius, height } | Shape::Cone { radius, height } => {
            theme::prop(ui, "Radius", |ui| changed |= drag(ui, radius));
            theme::prop(ui, "Height", |ui| changed |= drag(ui, height));
        }
        Shape::Capsule { radius, height } => {
            theme::prop(ui, "Radius", |ui| changed |= drag(ui, radius));
            theme::prop(ui, "Height", |ui| changed |= drag(ui, height));
            *height = height.max(*radius * 2.0);
        }
        Shape::Torus { radius, tube } => {
            theme::prop(ui, "Radius", |ui| changed |= drag(ui, radius));
            theme::prop(ui, "Tube", |ui| changed |= drag(ui, tube));
        }
    }
    theme::prop(ui, "Color", |ui| {
        changed |= theme::color_rgba(ui, &mut p.color)
    });
    theme::prop(ui, "Top color", |ui| {
        let mut on = p.top_color.is_some();
        if ui
            .checkbox(&mut on, "")
            .on_hover_text("Vertical gradient")
            .changed()
        {
            p.top_color = on.then_some([255, 255, 255, 255]);
            changed = true;
        }
        if let Some(t) = &mut p.top_color {
            changed |= theme::color_rgba(ui, t);
        }
    });
    theme::prop(ui, "Pivot", |ui| {
        for (v, label) in [(Pivot::Center, "Center"), (Pivot::Bottom, "Bottom")] {
            if ui.selectable_label(p.pivot == v, label).clicked() && p.pivot != v {
                p.pivot = v;
                changed = true;
            }
        }
    });
    theme::prop(ui, "Segments", |ui| {
        changed |= ui.add(egui::Slider::new(&mut p.segments, 3..=48)).changed();
    });
    theme::prop(ui, "Normals", |ui| {
        changed |= ui.checkbox(&mut p.smooth, "Smooth").changed();
    });
    changed
}

fn material_asset(app: &mut App, ui: &mut egui::Ui, logical: &str) {
    let users = app.material_users(logical);
    let sprite_names: Vec<String> = app.project.sprites.keys().cloned().collect();
    let thumb = app.material_thumbs.get(logical).map(|(_, t)| t.clone());
    let Some(def) = app.lib.materials.get_mut(logical) else {
        theme::empty_state(ui, icon::PALETTE, "Material not found", logical);
        return;
    };
    header(
        ui,
        icon::PALETTE,
        c::MATERIAL,
        logical.trim_start_matches("materials/"),
        "Material",
    );
    big_thumb(ui, thumb.as_ref(), icon::PALETTE);
    let mut changed = false;
    theme::component(ui, "c-mat", icon::PALETTE, c::MATERIAL, "Surface", |ui| {
        changed = material_editor(ui, def, &sprite_names);
    });
    card().show(ui, |ui| {
        ui.label(RichText::new(format!("Used by {users} entit{} in this scene", if users == 1 { "y" } else { "ies" })).color(c::TEXT_WEAK));
        ui.label(RichText::new(format!("assets/materials/{}.material.ron", logical.trim_start_matches("materials/"))).monospace().size(11.5).color(c::TEXT_FAINT));
        ui.label(RichText::new("Everything here is evaluated by the engine's software T&L: identical on PC, N64 and 3DS.").size(11.5).color(c::TEXT_FAINT));
    });
    if changed {
        app.cmds.push(Cmd::SaveMaterial(logical.to_string()));
    }
}

fn prefab_asset(app: &mut App, ui: &mut egui::Ui, logical: &str) {
    let Some(prefab) = app.lib.prefabs.get(logical).cloned() else {
        theme::empty_state(ui, icon::PACKAGE, "Prefab not found", logical);
        return;
    };
    header(
        ui,
        icon::PACKAGE,
        c::PREFAB,
        logical.trim_start_matches("prefabs/"),
        "Prefab",
    );
    let mut instances = 0;
    app.doc.scene.walk(|_, e| {
        if e.prefab.as_ref().is_some_and(|p| p.path == logical) {
            instances += 1;
        }
    });
    card().show(ui, |ui| {
        ui.label(
            RichText::new(format!(
                "{instances} instance(s) in this scene — all update when the prefab is saved."
            ))
            .color(c::TEXT_WEAK),
        );
        ui.add_space(4.0);
        fn tree(ui: &mut egui::Ui, e: &Entity, depth: usize) {
            let (g, col) = crate::panels::hierarchy::entity_glyph(e);
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 14.0);
                ui.label(RichText::new(g).color(col));
                ui.label(&e.name);
            });
            for c in &e.children {
                tree(ui, c, depth + 1);
            }
        }
        tree(ui, &prefab.root, 0);
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if theme::primary_button(ui, format!("{}  Add to scene", icon::PLUS)).clicked() {
            app.cmds.push(Cmd::AddAsset(
                AssetPayload::Prefab(logical.to_string()),
                None,
            ));
        }
        if !app.doc.is_prefab()
            && ui
                .button(format!("{}  Edit prefab", icon::PENCIL_SIMPLE))
                .clicked()
        {
            app.cmds.push(Cmd::EditPrefab(logical.to_string()));
        }
    });
}

fn sprite_asset(app: &mut App, ui: &mut egui::Ui, logical: &str) {
    let Some(s) = app.project.sprites.get(logical) else {
        theme::empty_state(ui, icon::IMAGE, "Sprite not found", logical);
        return;
    };
    header(
        ui,
        icon::IMAGE,
        c::SPRITE,
        logical.trim_start_matches("sprites/"),
        "Sprite",
    );
    big_thumb(ui, s.thumb.as_ref(), icon::IMAGE);
    let tmem = Caps::N64.validate_texture(s.width as u16, s.height as u16, 1);
    card().show(ui, |ui| {
        ui.label(format!("{} × {} px", s.width, s.height));
        match tmem {
            Ok(()) => ui.label(
                RichText::new(format!("{} fits N64 TMEM (CI8)", icon::CHECK_CIRCLE)).color(c::OK),
            ),
            Err(e) => {
                ui.label(RichText::new(format!("{} N64 TMEM: {e:?}", icon::WARNING)).color(c::WARN))
            }
        };
    });
    ui.add_space(8.0);
    if theme::primary_button(ui, format!("{}  Add to scene", icon::PLUS)).clicked() {
        app.cmds.push(Cmd::AddAsset(
            AssetPayload::Sprite(logical.to_string()),
            None,
        ));
    }
}

// ---------------------------------------------------------------- scene --

fn scene_settings(app: &mut App, ui: &mut egui::Ui) {
    let in_prefab = app.doc.is_prefab();
    header(
        ui,
        if in_prefab {
            icon::PACKAGE
        } else {
            icon::CUBE_TRANSPARENT
        },
        c::ACCENT_TEXT,
        &app.doc.title(),
        if in_prefab {
            "Prefab — select a part to edit it"
        } else {
            "Scene settings — select an entity to edit it"
        },
    );
    let cam_now = app.scene_view.cam;
    let env = &mut app.doc.scene.environment;

    theme::component(ui, "env-look", icon::PALETTE, c::TEXT, "Background", |ui| {
        theme::prop(ui, "Clear", |ui| theme::color_rgb(ui, &mut env.clear));
    });
    theme::component(ui, "env-light", icon::SUN, c::WARN, "Light", |ui| {
        let d = env.light.dir;
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
        let mut elev = (-d[1] / len).clamp(-1.0, 1.0).asin().to_degrees();
        let mut azim = d[0].atan2(d[2]).to_degrees();
        let mut changed = false;
        theme::prop(ui, "Azimuth", |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut azim, -180.0..=180.0).suffix("°"))
                .changed()
        });
        theme::prop(ui, "Elevation", |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut elev, -10.0..=90.0).suffix("°"))
                .changed()
        });
        if changed {
            let (a, e) = (azim.to_radians(), elev.to_radians());
            env.light.dir = [e.cos() * a.sin(), -e.sin(), e.cos() * a.cos()];
        }
        theme::prop(ui, "Color", |ui| theme::color_rgb(ui, &mut env.light.color));
        theme::prop(ui, "Ambient", |ui| {
            ui.add(egui::Slider::new(&mut env.light.ambient, 0.0..=1.0));
        });
        theme::prop(ui, "Amb. color", |ui| {
            theme::color_rgb(ui, &mut env.light.ambient_color)
        });
    });
    theme::component(ui, "env-fog", icon::CLOUD_FOG, c::TEXT_WEAK, "Fog", |ui| {
        let mut on = env.fog.is_some();
        if ui.checkbox(&mut on, "Distance fog (N64 classic)").changed() {
            env.fog = on.then_some(trino_scene::FogDef {
                color: env.clear,
                near: 12.0,
                far: 40.0,
            });
        }
        if let Some(f) = &mut env.fog {
            theme::prop(ui, "Color", |ui| theme::color_rgb(ui, &mut f.color));
            theme::prop(ui, "Start", |ui| {
                ui.add(egui::DragValue::new(&mut f.near).speed(0.2).suffix(" m"));
            });
            theme::prop(ui, "Full", |ui| {
                ui.add(egui::DragValue::new(&mut f.far).speed(0.2).suffix(" m"));
            });
            if ui.small_button("Match background").clicked() {
                f.color = env.clear;
            }
        }
    });
    theme::component(
        ui,
        "env-cam",
        icon::VIDEO_CAMERA,
        c::TEXT,
        "Game camera",
        |ui| {
            theme::vec3_row(ui, "Eye", &mut env.camera.eye, 0.05, "");
            theme::vec3_row(ui, "Target", &mut env.camera.target, 0.05, "");
            theme::prop(ui, "FOV", |ui| {
                ui.add(egui::Slider::new(&mut env.camera.fov, 20.0..=110.0).suffix("°"));
            });
            if ui
                .button(format!("{}  Set from the Scene view", icon::CAMERA))
                .clicked()
            {
                let e = cam_now.eye();
                let t = cam_now.target;
                env.camera.eye = [e.x, e.y, e.z];
                env.camera.target = [t.x, t.y, t.z];
                env.camera.fov = cam_now.fov.to_degrees();
            }
        },
    );
    if !app.resolved.warnings.is_empty() {
        theme::component(ui, "env-warn", icon::WARNING, c::WARN, "Issues", |ui| {
            for w in &app.resolved.warnings {
                ui.label(RichText::new(w).color(c::WARN).size(12.0));
            }
        });
    }
}
