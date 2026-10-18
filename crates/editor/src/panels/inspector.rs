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
use trino_scene::{
    AnimComponent, Entity, MaterialDef, ModelComponent, Override, PrefabInstance, ShadingDef,
    SpriteComponent, Target, VisComponent,
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
            AssetSel::Particles(n) => crate::panels::particles::asset(app, ui, &n),
            AssetSel::Audio(l) => audio_asset(app, ui, &l),
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
    let model_infos: Vec<crate::panels::platforms::ModelInfo> = app
        .project
        .models
        .values()
        .map(|m| crate::panels::platforms::ModelInfo {
            logical: m.logical.clone(),
            verts: m.verts,
            tris: m.tris,
        })
        .collect();
    let sprite_names: Vec<String> = app.project.sprites.keys().cloned().collect();
    let sound_names: Vec<String> = app.project.sounds.clone();
    let material_names: Vec<String> = app.lib.materials.keys().cloned().collect();
    let prefab_names: Vec<String> = app.lib.prefabs.keys().cloned().collect();
    let is_prefab_doc = app.doc.is_prefab();
    let focus = app.inspect_focus.clone();
    // The entity's render path on each platform (the budget's estimate
    // from the scene camera; issue #51's "Render path" row).
    let render_paths = app.budgets.as_ref().zip(app.doc.scene.draw_name(path)).map(
        |((_, [n64, n3ds, pc, web, ..]), name)| {
            [
                ("PC", trino_scene::Target::Pc, pc.path_of(&name)),
                ("N64", trino_scene::Target::N64, n64.path_of(&name)),
                ("3DS", trino_scene::Target::N3ds, n3ds.path_of(&name)),
                ("Web", trino_scene::Target::Web, web.path_of(&name)),
            ]
        },
    );
    let group_names = app.groups.names.clone();
    let mut cell_names: Vec<String> = Vec::new();
    app.doc.scene.walk(|_, e| {
        if matches!(e.vis, Some(VisComponent::Cell { .. })) {
            cell_names.push(e.name.clone());
        }
    });

    let impostor_names: Vec<String> = app.lib.impostors.keys().cloned().collect();
    let App {
        doc,
        project,
        lib,
        cmds,
        material_thumbs,
        lod_preview,
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
                let w = ui.available_width() - 38.0;
                theme::fixed_width(ui, w, |ui| {
                    egui::ComboBox::from_id_salt("model-path")
                        .selected_text(&m.path)
                        .width(w)
                        .truncate()
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
            if let Some(paths) = &render_paths {
                render_path_row(ui, paths);
            }

            // Material picker + inline editor.
            theme::prop(ui, "Material", |ui| {
                let current = m.material.clone().unwrap_or_else(|| "Default".into());
                let w = ui.available_width() - 38.0;
                theme::fixed_width(ui, w, |ui| {
                    egui::ComboBox::from_id_salt("material")
                        .selected_text(current)
                        .width(w)
                        .truncate()
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
                        if def.water.is_some()
                            && ui
                                .button(format!("{}  Drop a ripple", icon::DROP))
                                .on_hover_text(
                                    "Start a ripple in the middle of this surface: the \
                                     views ring it out like a game's splash would.",
                                )
                                .clicked()
                        {
                            cmds.push(Cmd::DropRipple(path.to_vec()));
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
            // Distance LOD: lighter meshes, an impostor, or nothing, far away.
            let open = !m.lod.is_empty() || focus.as_deref() == Some("lod");
            let lod_top = ui.cursor().min;
            egui::CollapsingHeader::new(
                RichText::new(format!("{}  LOD ({})", icon::STACK, m.lod.len()))
                    .color(c::TEXT_WEAK),
            )
            .id_salt("model-lod")
            .default_open(open)
            .show(ui, |ui| {
                let base = model_infos.iter().find(|i| i.logical == m.path);
                let mut preview = lod_preview
                    .as_ref()
                    .filter(|(p, _)| p.as_slice() == path)
                    .map(|(_, d)| *d);
                crate::panels::lod::ui(
                    ui,
                    &mut m.lod,
                    &crate::panels::lod::Choices {
                        models: &model_infos,
                        impostors: &impostor_names,
                        base,
                    },
                    &mut preview,
                );
                *lod_preview = preview.map(|d| (path.to_vec(), d));
            });
            if focus.as_deref() == Some("lod") {
                ui.scroll_to_rect(
                    egui::Rect::from_min_size(lod_top, Vec2::new(1.0, 1.0)),
                    Some(egui::Align::TOP),
                );
            }
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
                    if !inst.overrides.is_empty()
                        && ui
                            .button(format!("{} Apply overrides to prefab", icon::UPLOAD_SIMPLE))
                            .on_hover_text(
                                "Write these overrides into the prefab itself: every \
                                 instance gets them, this one's overrides are cleared",
                            )
                            .clicked()
                    {
                        cmds.push(Cmd::ApplyOverrides(path.to_vec()));
                    }
                    // In-place prefab editing: the source, from the scene.
                    egui::CollapsingHeader::new(
                        RichText::new(format!(
                            "{}  Edit prefab parts in place (all instances)",
                            icon::PENCIL_SIMPLE
                        ))
                        .color(c::PREFAB),
                    )
                    .id_salt("prefab-in-place")
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(
                                "Changes write prefabs/… on the spot — every instance in \
                                 every scene follows.",
                            )
                            .size(11.5)
                            .color(c::TEXT_FAINT),
                        );
                        if let Some(prefab) = lib.prefabs.get_mut(&inst.path)
                            && part_source_editor(ui, &mut prefab.root, "", &material_names)
                        {
                            cmds.push(Cmd::SavePrefab(inst.path.clone()));
                        }
                    });
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
                    ui.checkbox(&mut col.trigger, "overlap only").on_hover_text(
                        "A trigger: reports overlaps (coins, doors, zones) \
                                        instead of blocking",
                    );
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

    // Sound source.
    let mut remove_sound = false;
    if let Some(snd) = &mut e.sound {
        theme::component(
            ui,
            "c-sound",
            icon::SPEAKER_HIGH,
            c::SPRITE,
            "Sound source (positional loop)",
            |ui| {
                theme::prop(ui, "Sound", |ui| {
                    egui::ComboBox::from_id_salt("sound-path")
                        .selected_text(&snd.path)
                        .show_ui(ui, |ui| {
                            for name in &sound_names {
                                if ui.selectable_label(snd.path == *name, name).clicked() {
                                    snd.path = name.clone();
                                }
                            }
                        });
                });
                theme::prop(ui, "Radius", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut snd.radius)
                            .speed(0.1)
                            .range(0.1..=500.0)
                            .suffix(" m"),
                    );
                });
                theme::prop(ui, "Volume", |ui| {
                    ui.add(egui::Slider::new(&mut snd.volume, 0.0..=1.0));
                });
                ui.label(
                    RichText::new(
                        "Loops near the listener, panned by side (Ambience). On N64 the sound \
                         needs `loop = true` in the manifest.",
                    )
                    .size(11.5)
                    .color(c::TEXT_FAINT),
                );
                if ui
                    .small_button(RichText::new(format!("{} Remove", icon::TRASH)).color(c::ERROR))
                    .clicked()
                {
                    remove_sound = true;
                }
            },
        );
    }
    if remove_sound {
        e.sound = None;
    }

    // Interior visibility (cell / portal).
    let mut remove_vis = false;
    if let Some(vis) = &mut e.vis {
        let is_cell = matches!(vis, VisComponent::Cell { .. });
        theme::component(
            ui,
            "c-vis",
            if is_cell {
                icon::CUBE_TRANSPARENT
            } else {
                icon::DOOR_OPEN
            },
            if is_cell { c::CELL } else { c::PORTAL },
            if is_cell {
                "Cell (room)"
            } else {
                "Portal (doorway)"
            },
            |ui| {
                match vis {
                    VisComponent::Cell { size, groups } => {
                        theme::vec3_row(ui, "Size", size, 0.1, "");
                        theme::prop(ui, "Groups", |ui| {
                            // `boot` is always loaded: never a cell's.
                            let names: Vec<String> = group_names
                                .iter()
                                .filter(|g| *g != trino_asset_pipeline::groups::BOOT)
                                .cloned()
                                .collect();
                            if names.is_empty() && groups.is_empty() {
                                ui.label(
                                    RichText::new("no groups yet (Assets › Groups)")
                                        .size(11.5)
                                        .color(c::TEXT_FAINT),
                                );
                            } else if let Some(next) =
                                crate::groups::toggles(ui, "cell-groups", &names, groups, &[])
                            {
                                *groups = next;
                            }
                        });
                        ui.label(
                            RichText::new(
                                "Children belong to this room and draw only while a chain of \
                                 portals makes it visible from the camera's room. Its groups \
                                 stream in while it (or a room a doorway away) is in view.",
                            )
                            .size(11.5)
                            .color(c::TEXT_FAINT),
                        );
                    }
                    VisComponent::Portal { size, a, b } => {
                        theme::vec3_row(ui, "Size", size, 0.05, "");
                        for (label, value) in [("Cell A", a), ("Cell B", b)] {
                            theme::prop(ui, label, |ui| {
                                egui::ComboBox::from_id_salt(("portal-cell", label))
                                    .selected_text(value.as_str())
                                    .show_ui(ui, |ui| {
                                        for name in &cell_names {
                                            if ui.selectable_label(value == name, name).clicked() {
                                                *value = name.clone();
                                            }
                                        }
                                    });
                            });
                        }
                    }
                }
                if ui
                    .small_button(RichText::new(format!("{} Remove", icon::TRASH)).color(c::ERROR))
                    .clicked()
                {
                    remove_vis = true;
                }
            },
        );
    }
    if remove_vis {
        e.vis = None;
    }

    // Per-platform variants (PC / N64 / 3DS / Web columns, and the
    // consoles' tiers).
    let variants = Target::EVERY
        .iter()
        .filter(|t| e.platforms.own(**t).is_some())
        .count();
    let title = if variants == 0 {
        "Platforms".to_string()
    } else {
        format!(
            "Platforms ({variants} variant{})",
            if variants == 1 { "" } else { "s" }
        )
    };
    let focus_platforms = focus.as_deref() == Some("platforms");
    let top = ui.cursor().min;
    theme::component_open(
        ui,
        "c-platforms",
        icon::DEVICES,
        c::ACCENT_TEXT,
        &title,
        variants > 0 || focus_platforms,
        |ui| {
            crate::panels::platforms::ui(
                ui,
                e,
                &crate::panels::platforms::Choices {
                    models: &model_infos,
                    materials: &material_names,
                },
            );
        },
    );
    if focus_platforms {
        ui.scroll_to_rect(
            egui::Rect::from_min_size(top, Vec2::new(1.0, ui.clip_rect().height() - 8.0)),
            Some(egui::Align::TOP),
        );
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
                if e.sound.is_none()
                    && ui
                        .button(format!("{}  Sound source", icon::SPEAKER_HIGH))
                        .clicked()
                {
                    let first = sound_names
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "sounds/beep".into());
                    e.sound = Some(trino_scene::SoundComponent::new(first));
                }
                if e.vis.is_none() {
                    if ui
                        .button(format!("{}  Cell (room)", icon::CUBE_TRANSPARENT))
                        .clicked()
                    {
                        e.vis = Some(VisComponent::Cell {
                            size: [8.0, 4.0, 8.0],
                            groups: Vec::new(),
                        });
                    }
                    if ui
                        .button(format!("{}  Portal (doorway)", icon::DOOR_OPEN))
                        .clicked()
                    {
                        let name = |i: usize| cell_names.get(i).cloned().unwrap_or_default();
                        e.vis = Some(VisComponent::Portal {
                            size: [1.5, 2.5, 0.2],
                            a: name(0),
                            b: name(1),
                        });
                    }
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

/// The prefab's own parts (in-place editing): transform and material per
/// part, recursively. Returns true when something changed.
fn part_source_editor(
    ui: &mut egui::Ui,
    e: &mut Entity,
    target: &str,
    materials: &[String],
) -> bool {
    let mut changed = false;
    let label = if target.is_empty() {
        format!("{} (root)", e.name)
    } else {
        target.to_string()
    };
    egui::CollapsingHeader::new(RichText::new(label).size(12.0))
        .id_salt(("part-src", target))
        .show(ui, |ui| {
            let before = e.transform;
            theme::vec3_row(ui, "Position", &mut e.transform.pos, 0.05, "");
            theme::vec3_row(ui, "Rotation", &mut e.transform.rot, 0.5, "°");
            theme::vec3_row(ui, "Scale", &mut e.transform.scale, 0.01, "");
            changed |= e.transform != before;
            if let Some(m) = &mut e.model {
                theme::prop(ui, "Material", |ui| {
                    let cur = m.material.clone().unwrap_or_else(|| "Default".into());
                    egui::ComboBox::from_id_salt(("part-mat", target))
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(m.material.is_none(), "Default")
                                .clicked()
                            {
                                m.material = None;
                                changed = true;
                            }
                            for name in materials {
                                let sel = m.material.as_deref() == Some(name.as_str());
                                if ui.selectable_label(sel, name).clicked() && !sel {
                                    m.material = Some(name.clone());
                                    changed = true;
                                }
                            }
                        });
                });
            }
        });
    for child in &mut e.children {
        let t = if target.is_empty() {
            child.name.clone()
        } else {
            format!("{target}/{}", child.name)
        };
        changed |= part_source_editor(ui, child, &t, materials);
    }
    changed
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
    theme::prop(ui, "Gloss", |ui| {
        changed |= ui
            .add(egui::Slider::new(&mut def.gloss, 0..=255))
            .on_hover_text(
                "Specular highlights (0 = matte). PC per-pixel lighting only; \
                 consoles ignore it.",
            )
            .changed();
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
    theme::prop(ui, "Filter", |ui| {
        for f in trino_scene::FilterDef::ALL {
            if ui
                .selectable_label(def.filter == f, f.label())
                .on_hover_text(match f {
                    trino_scene::FilterDef::Default => {
                        "The target's own: 3-point on N64, bilinear on 3DS, nearest on PC"
                    }
                    trino_scene::FilterDef::Point => "Nearest texel: crisp pixels",
                    trino_scene::FilterDef::Bilinear => "Smooth (the RDP's 3-point on N64)",
                })
                .clicked()
            {
                def.filter = f;
                changed = true;
            }
        }
    });
    theme::prop(ui, "Detail", |ui| {
        let current = def
            .detail
            .as_ref()
            .map(|d| d.texture.clone())
            .unwrap_or_else(|| "None".into());
        egui::ComboBox::from_id_salt("mat-detail")
            .selected_text(current)
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(def.detail.is_none(), "None").clicked() {
                    def.detail = None;
                    changed = true;
                }
                for t in textures {
                    let sel = def.detail.as_ref().is_some_and(|d| d.texture == *t);
                    if ui.selectable_label(sel, t).clicked() && !sel {
                        let scale_log2 = def.detail.as_ref().map_or(2, |d| d.scale_log2);
                        def.detail = Some(trino_scene::DetailDef {
                            texture: t.clone(),
                            scale_log2,
                        });
                        changed = true;
                    }
                }
            })
            .response
            .on_hover_text(
                "A texture multiplied over the base one, tiling 2^scale times per base \
                 repeat. N64: both share 4 KB of TMEM (a small I4 detail).",
            );
    });
    if let Some(d) = &mut def.detail {
        theme::prop(ui, "Detail scale", |ui| {
            changed |= ui
                .add(
                    egui::Slider::new(
                        &mut d.scale_log2,
                        0..=trino_core::DetailTexture::MAX_SCALE_LOG2,
                    )
                    .custom_formatter(|v, _| format!("×{}", 1u32 << v as u32)),
                )
                .changed();
        });
    }
    changed |= water_editor(ui, &mut def.water);
    changed
}

/// The material's water surface (ADR 0015): on or off, and its flow, swell
/// and ripple response. Every change hot-reloads the views.
pub fn water_editor(ui: &mut egui::Ui, water: &mut Option<trino_scene::WaterDef>) -> bool {
    use trino_scene::WaterDef;
    let mut changed = false;
    theme::prop(ui, "Water", |ui| {
        let mut on = water.is_some();
        if ui
            .checkbox(&mut on, "Water surface")
            .on_hover_text(
                "Swell, ripples, the two layers flowing, Fresnel. PC: per pixel with \
                 glint and shore foam; N64 and 3DS: per vertex, the layers blended by \
                 the combiner.",
            )
            .changed()
        {
            *water = on.then(WaterDef::default);
            changed = true;
        }
    });
    let Some(w) = water else {
        return changed;
    };
    let rows: [(&str, &mut f32, (f32, f32), &str); 7] = [
        (
            "Flow u",
            &mut w.flow_speed.0,
            WaterDef::FLOW_RANGE,
            "Texture repeats per second across",
        ),
        (
            "Flow v",
            &mut w.flow_speed.1,
            WaterDef::FLOW_RANGE,
            "Texture repeats per second along",
        ),
        (
            "Detail flow u",
            &mut w.detail_flow.0,
            WaterDef::FLOW_RANGE,
            "The detail layer's own flow",
        ),
        (
            "Detail flow v",
            &mut w.detail_flow.1,
            WaterDef::FLOW_RANGE,
            "The detail layer's own flow",
        ),
        (
            "Wave height",
            &mut w.wave_amplitude,
            WaterDef::AMPLITUDE_RANGE,
            "Swell height, meters",
        ),
        (
            "Wave frequency",
            &mut w.wave_frequency,
            WaterDef::FREQUENCY_RANGE,
            "Swell crests per meter",
        ),
        (
            "Ripples",
            &mut w.ripple_responsiveness,
            WaterDef::RESPONSE_RANGE,
            "How far ripples push the surface (0 = ignore them)",
        ),
    ];
    for (label, v, (lo, hi), tip) in rows {
        theme::prop(ui, label, |ui| {
            changed |= ui
                .add(egui::Slider::new(v, lo..=hi).max_decimals(2))
                .on_hover_text(tip)
                .changed();
        });
    }
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

pub fn budget_bar(ui: &mut egui::Ui, label: &str, used: f32, budget: f32) {
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
    let baked_ao = m.ao.clone();

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
        // What one draw costs the N64's CPU T&L (calibrated), against a
        // 30 fps frame.
        budget_bar(
            ui,
            "N64 µs/draw",
            trino_core::perf::N64_COST.mesh_draw_us(verts as u32, tris as u32) as f32,
            trino_core::perf::FRAME_30FPS_US as f32,
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
    crate::panels::impostor::section(app, ui, logical, tris, verts);
    baked_ao_section(app, ui, &name, baked_ao.as_ref());

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

/// Model › Ambient occlusion (baked): the manifest's `ao` for this model.
/// The bake darkens its vertex colors where the model hides the sky from
/// itself — how the N64 and 3DS get AO, at no run-time cost.
fn baked_ao_section(
    app: &mut App,
    ui: &mut egui::Ui,
    name: &str,
    baked: Option<&trino_asset_pipeline::AoBake>,
) {
    let top = ui.cursor().min;
    if app.inspect_focus.as_deref() == Some("ao") {
        ui.scroll_to_rect(
            egui::Rect::from_min_size(top, Vec2::new(1.0, 1.0)),
            Some(egui::Align::TOP),
        );
    }
    theme::component(
        ui,
        "c-ao-bake",
        icon::CIRCLE_HALF_TILT,
        c::TEXT,
        "Ambient occlusion (baked)",
        |ui| {
            ui.label(
                RichText::new(
                    "Bakes occlusion into the vertex colors: free on the N64 and 3DS, \
                     the same picture on every target. Rewrites the manifest and rebakes.",
                )
                .size(11.5)
                .color(c::TEXT_FAINT),
            );
            let id = ui.id().with(("ao-bake", name));
            let mut draft = ui.data_mut(|d| {
                d.get_temp::<trino_asset_pipeline::AoBake>(id)
                    .unwrap_or_else(|| baked.cloned().unwrap_or_default())
            });
            theme::prop(ui, "Strength", |ui| {
                ui.add(egui::Slider::new(&mut draft.strength, 0.0..=1.0));
            });
            theme::prop(ui, "Distance", |ui| {
                ui.add(
                    egui::DragValue::new(&mut draft.distance)
                        .range(0.0..=100.0)
                        .speed(0.02)
                        .suffix(" m"),
                )
                .on_hover_text("How far occluders count. 0 = a fifth of the model's size");
            });
            theme::prop(ui, "Rays", |ui| {
                ui.add(egui::Slider::new(&mut draft.rays, 16..=256).logarithmic(true));
            });
            ui.checkbox(
                &mut draft.ground,
                "Ground contact (floor at its lowest point)",
            );
            ui.horizontal(|ui| {
                let label = if baked.is_some() {
                    "Re-bake AO"
                } else {
                    "Bake AO to vertex colors"
                };
                if theme::primary_button(ui, format!("{}  {label}", icon::CIRCLE_HALF_TILT))
                    .clicked()
                {
                    app.cmds
                        .push(Cmd::SetModelAo(name.to_string(), Some(draft.clone())));
                }
                if baked.is_some() && ui.button(format!("{}  Remove", icon::TRASH)).clicked() {
                    app.cmds.push(Cmd::SetModelAo(name.to_string(), None));
                }
            });
            if let Some(b) = baked {
                ui.label(
                    RichText::new(format!("Baked: ao = {}", b.to_toml()))
                        .monospace()
                        .size(11.0)
                        .color(c::TEXT_WEAK),
                );
            }
            ui.data_mut(|d| d.insert_temp(id, draft));
        },
    );
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
    let font = logical.starts_with("fonts/");
    header(
        ui,
        if font { icon::TEXT_AA } else { icon::IMAGE },
        c::SPRITE,
        logical.split_once('/').map_or(logical, |(_, n)| n),
        if font { "Font atlas" } else { "Sprite" },
    );
    big_thumb(ui, s.thumb.as_ref(), icon::IMAGE);
    let size = format!("{} × {} px", s.width, s.height);
    card().show(ui, |ui| {
        ui.label(size);
    });
    ui.add_space(8.0);
    if !font && theme::primary_button(ui, format!("{}  Add to scene", icon::PLUS)).clicked() {
        app.cmds.push(Cmd::AddAsset(
            AssetPayload::Sprite(logical.to_string()),
            None,
        ));
    }
    ui.add_space(8.0);
    crate::panels::texture::section(app, ui, logical);
}

fn audio_asset(app: &mut App, ui: &mut egui::Ui, logical: &str) {
    let music = logical.starts_with("music/");
    header(
        ui,
        if music {
            icon::MUSIC_NOTES
        } else {
            icon::SPEAKER_HIGH
        },
        c::ACCENT_TEXT,
        logical.split_once('/').map_or(logical, |(_, n)| n),
        if music { "Music track" } else { "Sound" },
    );
    crate::panels::audio::section(app, ui, logical);
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
    let ao_top = ui.cursor().min;
    theme::component(
        ui,
        "env-ao",
        icon::CIRCLE_HALF_TILT,
        c::TEXT_WEAK,
        "Ambient Occlusion",
        |ui| ambient_occlusion(ui, &mut env.ao),
    );
    if app.inspect_focus.as_deref() == Some("ao") {
        ui.scroll_to_rect(
            egui::Rect::from_min_size(ao_top, Vec2::new(1.0, 1.0)),
            Some(egui::Align::TOP),
        );
    }
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
    if let Some((_, budgets)) = &app.budgets {
        if std::mem::take(&mut app.scroll_to_budget)
            || app.inspect_focus.as_deref() == Some("budget")
        {
            ui.scroll_to_rect(
                egui::Rect::from_min_size(ui.cursor().min, Vec2::new(1.0, 1.0)),
                Some(egui::Align::TOP),
            );
        }
        let picked = theme::component(
            ui,
            "env-budget",
            icon::GAUGE,
            c::TEXT,
            "Platform budget (scene camera)",
            |ui| {
                let open = app.inspect_focus.as_deref() == Some("budget");
                let game = app.game_view.target();
                let picked = budget_section(ui, budgets, &app.project.tmem, open, game);
                texture_memory(ui, &mut app.textures, &app.project);
                crate::panels::audio::budget(ui, &mut app.audio, &app.project);
                picked
            },
        )
        .flatten();
        // An unseen draw's line, or an entity under a reason, selects it.
        if let Some(name) = picked {
            let mut found = app.doc.scene.find_draw_name(&name);
            app.doc.scene.walk(|p, e| {
                if e.name == name && found.is_none() {
                    found = Some(p.to_vec());
                }
            });
            if let Some(path) = found {
                app.cmds.push(Cmd::Select(Some(path)));
            }
        }
    }
    asset_groups(app, ui);
    if !app.resolved.warnings.is_empty() {
        theme::component(ui, "env-warn", icon::WARNING, c::WARN, "Issues", |ui| {
            for w in &app.resolved.warnings {
                ui.label(RichText::new(w).color(c::WARN).size(12.0));
            }
        });
    }
}

/// Scene settings › Asset groups (issue #56): the groups the scene
/// `requires` (boot always), the cells' own, what `cargo xtask assets
/// check` finds, and each console tier's share with them all loaded.
fn asset_groups(app: &mut App, ui: &mut egui::Ui) {
    if app.doc.is_prefab() {
        return;
    }
    let top = ui.cursor().min;
    let names = app.groups.names.clone();
    let rev = app.view_input().rev;
    let problems = app.groups.scene_problems(&app.doc.scene, &app.lib, rev);
    let plans = app
        .groups
        .plans(ui.ctx(), &app.project.assets_dir(), app.project.generation)
        .and_then(|p| p.as_ref().ok())
        .cloned();
    let scene = &mut app.doc.scene;
    let open_memory = theme::component(
        ui,
        "env-groups",
        icon::STACK,
        c::TEXT,
        "Asset groups",
        |ui| {
            theme::prop(ui, "Requires", |ui| {
                let boot = trino_asset_pipeline::groups::BOOT;
                if let Some(next) =
                    crate::groups::toggles(ui, "requires", &names, &scene.requires, &[boot])
                {
                    scene.requires = next;
                }
            });
            let cells = scene.cell_groups();
            if !cells.is_empty() {
                for (cell, groups) in &cells {
                    ui.label(
                        RichText::new(format!(
                            "{}  {cell} streams {}",
                            icon::CUBE_TRANSPARENT,
                            groups.join(", ")
                        ))
                        .size(11.5)
                        .color(c::TEXT_WEAK),
                    );
                }
            }
            for p in &problems {
                ui.label(
                    RichText::new(format!("{} {p}", icon::WARNING))
                        .size(11.5)
                        .color(c::WARN),
                );
            }
            let all = trino_asset_pipeline::groups::scene_groups(scene);
            let refs: Vec<&str> = all.iter().map(String::as_str).collect();
            if let Some(plans) = &plans {
                ui.horizontal_wrapped(|ui| {
                    // Rows as tall as the text when the tiers wrap.
                    ui.spacing_mut().interact_size.y = 16.0;
                    for plan in plans {
                        let used = plan.resident(&refs);
                        let pct = used * 100 / plan.budget().max(1);
                        let color = if used > plan.budget() {
                            c::ERROR
                        } else {
                            c::TEXT_WEAK
                        };
                        // One unit per tier: wraps between tiers only.
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{} {} {pct}%",
                                    trino_asset_pipeline::memory::platform_label(plan.platform),
                                    plan.tier.key()
                                ))
                                .size(11.5)
                                .color(color),
                            )
                            .wrap_mode(egui::TextWrapMode::Extend),
                        )
                        .on_hover_text(format!(
                            "{} of {} for assets",
                            trino_asset_pipeline::memory::show_bytes(used),
                            trino_asset_pipeline::memory::show_bytes(plan.budget())
                        ));
                    }
                });
            }
            ui.button(format!("{}  Memory tab", icon::MEMORY)).clicked()
        },
    );
    if open_memory == Some(true) {
        app.cmds.push(Cmd::FocusTab(crate::app::Tab::Memory));
    }
    if app.inspect_focus.as_deref() == Some("groups") {
        ui.scroll_to_rect(
            egui::Rect::from_min_size(top, Vec2::new(1.0, 1.0)),
            Some(egui::Align::TOP),
        );
    }
}

/// The Model component's "Render path" row: per platform, whether the
/// entity's draw runs on the hardware path (PC GPU T&L, N64 RSP, 3DS
/// PICA200), on the CPU T&L (with the rule it failed, on hover and below)
/// or as an impostor, from the scene camera.
fn render_path_row(
    ui: &mut egui::Ui,
    paths: &[(
        &str,
        trino_scene::Target,
        Option<trino_scene::budget::RenderPath>,
    ); 4],
) {
    use trino_scene::budget::RenderPath;
    let faint = |text: String| RichText::new(text).size(11.5).color(c::TEXT_FAINT);
    // Indented like the tris line, within the Inspector's width (a
    // `vertical` in a `horizontal` would grow it instead of wrapping).
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 64,
            ..Default::default()
        })
        .show(ui, |ui| {
            ui.label(faint("Render path".into()));
            // Wrapped, never squeezed: four pills don't fit a 960 px window's
            // Inspector on one line.
            ui.horizontal_wrapped(|ui| {
                for (name, target, path) in paths {
                    let (text, fg) = match path {
                        Some(RenderPath::Hardware) => (
                            format!("{name} {}", RenderPath::hardware_name(*target)),
                            c::OK,
                        ),
                        Some(RenderPath::Cpu(_)) => (format!("{name} CPU"), c::WARN),
                        Some(RenderPath::Impostor) => (format!("{name} impostor"), c::TEXT_WEAK),
                        None => (format!("{name} not drawn"), c::TEXT_FAINT),
                    };
                    // A button-shaped pill: its size is known before it is
                    // placed, so the wrapped row can move it to the next line.
                    let pill = ui.add(
                        egui::Button::new(RichText::new(text).size(11.5).color(fg))
                            .fill(c::BG3)
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(99.0)
                            .sense(egui::Sense::hover()),
                    );
                    match path {
                        Some(p) => pill.on_hover_text(p.text(*target)),
                        None => pill.on_hover_text(
                            "Culled, hidden on this platform or outside the scene camera",
                        ),
                    };
                }
            });
            // Why each CPU platform misses its hardware path, a line each.
            for (name, _, path) in paths {
                if let Some(RenderPath::Cpu(reason)) = path {
                    ui.add(egui::Label::new(faint(format!("{name}: {reason}"))).truncate());
                }
            }
        });
}

/// One reason a platform's hardware path rejects draws, as a collapsed
/// line that opens on the entities it holds back; clicking one selects it
/// (`picked`, a draw name). `open`: start opened (the PC's reasons under the
/// `TRINO_EDITOR_INSPECT=budget` hook, for screenshots).
fn reason_list(
    ui: &mut egui::Ui,
    picked: &mut Option<String>,
    budget: &trino_scene::budget::FrameBudget,
    prefix: &str,
    label: &str,
    n: u32,
    open: bool,
) {
    let names = budget.off_path(label);
    egui::CollapsingHeader::new(
        RichText::new(format!("{prefix}: {n} × {label}"))
            .size(12.0)
            .color(c::TEXT_FAINT),
    )
    .id_salt(("budget-reason", prefix, label))
    .default_open(open)
    .show(ui, |ui| {
        for name in names {
            let r = ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("{} {name}", icon::CROSSHAIR))
                            .size(12.0)
                            .color(c::TEXT_WEAK),
                    )
                    .frame(false)
                    .truncate(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Select it");
            if r.clicked() {
                *picked = Some(name.to_string());
            }
        }
    });
}

/// The project's texture memory per platform (every sprite and font
/// atlas: a console loads them all at boot), measured like the bake on a
/// worker: the PC's VRAM with what BCn saves, the 3DS's share of its
/// texture memory, the N64's largest TMEM load.
fn texture_memory(
    ui: &mut egui::Ui,
    textures: &mut crate::textures::Textures,
    project: &crate::project::Project,
) {
    use crate::textures::bytes;
    let line = |ui: &mut egui::Ui, text: String, color: egui::Color32| {
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).truncate())
    };
    let key = crate::textures::key(&project.manifest_path(), project.generation);
    let Some(t) = textures.totals(ui.ctx(), &project.assets_dir(), key) else {
        line(ui, "Textures: measuring…".into(), c::TEXT_FAINT);
        return;
    };
    let saved = 100.0 - t.pc_bytes as f64 * 100.0 / t.pc_rgba8.max(1) as f64;
    line(
        ui,
        format!(
            "Textures PC {} VRAM (−{saved:.0}% vs RGBA8)",
            bytes(t.pc_bytes)
        ),
        c::TEXT_WEAK,
    )
    .on_hover_text(format!(
        "{} textures: {} as RGBA8, {} with their PC formats",
        t.textures,
        bytes(t.pc_rgba8),
        bytes(t.pc_bytes)
    ));
    let pct = t.n3ds_bytes as f64 * 100.0 / t.n3ds_budget.max(1) as f64;
    line(
        ui,
        format!(
            "Textures 3DS {} of {} ({pct:.1}%)",
            bytes(t.n3ds_bytes),
            bytes(t.n3ds_budget)
        ),
        if pct > 100.0 {
            c::ERROR
        } else if pct > 75.0 {
            c::WARN
        } else {
            c::TEXT_WEAK
        },
    );
    if let Some((logical, b)) = &t.n64_max {
        line(
            ui,
            format!("N64 largest TMEM load {b} / 4096 ({logical})"),
            if *b > 4096 { c::WARN } else { c::TEXT_WEAK },
        )
        .on_hover_text(format!(
            "{logical}: {b} bytes. A 3D draw loads its whole texture into the 4 KB TMEM; \
             2D blits tile"
        ));
    }
    let web_pct = t.web_vram as f64 * 100.0 / t.web_budget.max(1) as f64;
    line(
        ui,
        format!(
            "Textures Web {} download · {} VRAM",
            bytes(t.web_download),
            bytes(t.web_vram)
        ),
        if web_pct > 100.0 {
            c::ERROR
        } else {
            c::TEXT_WEAK
        },
    )
    .on_hover_text(format!(
        "{} textures as KTX2 (Basis Universal) or RGBA8: what a browser downloads, and \
         its GPU memory once transcoded ({web_pct:.2}% of the Web's {})",
        t.textures,
        bytes(t.web_budget)
    ));
    let failing: usize = t.errors.iter().sum();
    if failing > 0 {
        line(
            ui,
            format!(
                "{failing} texture bakes fail (PC {}, N64 {}, 3DS {}, Web {}): cargo xtask \
                 texture report",
                t.errors[0], t.errors[1], t.errors[2], t.errors[3]
            ),
            c::ERROR,
        );
    }
}

/// The scene's frame per console: the N64's CPU and RSP time with every
/// reason a draw misses the RSP, the 3DS's triangles, and each draw sent
/// for nothing (`cargo xtask scene budget` prints the same). Returns the
/// name of an unseen draw the user clicked.
/// The scene's screen-space ambient occlusion (`Environment::ao`): what
/// the PC draws live; the consoles use the AO baked into each model (the
/// model inspector's "Ambient occlusion (baked)").
fn ambient_occlusion(ui: &mut egui::Ui, ao: &mut Option<trino_scene::AmbientOcclusionDef>) {
    use trino_scene::AoQualityDef;
    let mut on = ao.is_some_and(|a| a.enabled);
    if ui
        .checkbox(&mut on, "Screen-space AO (PC)")
        .on_hover_text(
            "Darkens creases and where models meet, live on the PC. The N64 and 3DS \
             can't afford it: bake AO into their models (Assets › model › Ambient \
             occlusion).",
        )
        .changed()
    {
        // Keep the tuning when turning it off.
        match ao {
            Some(a) => a.enabled = on,
            None => *ao = Some(trino_scene::AmbientOcclusionDef::default()),
        }
    }
    let Some(a) = ao.as_mut().filter(|a| a.enabled) else {
        return;
    };
    theme::prop(ui, "Radius", |ui| {
        ui.add(
            egui::Slider::new(&mut a.radius, 0.05..=4.0)
                .logarithmic(true)
                .suffix(" m"),
        )
        .on_hover_text("How far around a point occluders count");
    });
    theme::prop(ui, "Intensity", |ui| {
        ui.add(egui::Slider::new(&mut a.intensity, 0.0..=2.0));
    });
    theme::prop(ui, "Bias", |ui| {
        ui.add(
            egui::Slider::new(&mut a.bias, 0.0..=0.25)
                .suffix(" m")
                .max_decimals(3),
        )
        .on_hover_text("Raise it if flat or curved surfaces shadow themselves");
    });
    theme::prop(ui, "Quality", |ui| {
        let name = |q: AoQualityDef| match q {
            AoQualityDef::Low => "Low (8 samples)",
            AoQualityDef::Medium => "Medium (16 samples)",
            AoQualityDef::High => "High (32 samples)",
        };
        egui::ComboBox::from_id_salt("ao-quality")
            .selected_text(name(a.quality))
            .show_ui(ui, |ui| {
                for q in [AoQualityDef::Low, AoQualityDef::Medium, AoQualityDef::High] {
                    ui.selectable_value(&mut a.quality, q, name(q));
                }
            });
    });
    if ui.small_button("Reset to defaults").clicked() {
        *a = trino_scene::AmbientOcclusionDef::default();
    }
}

/// The scene camera's frame on every platform (the Environment's "Platform
/// budget"); `game` is the Game view's target, whose tier column is lit.
/// The consoles' hardware tiers side by side (issue #54, ADR 0020): the
/// N64 and the N64 + Expansion Pak, the 3DS and the New 3DS, then each
/// tier's RAM line and what the expanded tier's hardware is.
fn tier_tables(ui: &mut egui::Ui, budgets: &crate::budget::Budgets, game: Target) {
    let small = |text: &str, color| RichText::new(text).size(12.0).color(color);
    for console in [Target::N64, Target::N3ds] {
        let Some(table) = crate::budget::tier_table(budgets, console) else {
            continue;
        };
        ui.add_space(2.0);
        egui::Grid::new(("budget-tiers", console.key()))
            .num_columns(3)
            .spacing([14.0, 1.0])
            .show(ui, |ui| {
                ui.label(small("Tier", c::TEXT_FAINT));
                for t in table.targets {
                    let color = if t == game {
                        c::ACCENT_TEXT
                    } else {
                        c::TEXT_WEAK
                    };
                    ui.label(small(t.label(), color));
                }
                ui.end_row();
                // RAM has its own lines below, in KB.
                for (label, base, expanded) in table.rows.iter().filter(|r| r.0 != "RAM") {
                    ui.label(small(label, c::TEXT_WEAK));
                    ui.label(small(base, c::TEXT));
                    ui.label(small(expanded, c::TEXT));
                    ui.end_row();
                }
            });
        for (i, t) in table.targets.into_iter().enumerate() {
            let Some(text) = trino_scene::budget::ram_line(crate::budget::budget_of(budgets, t))
            else {
                continue;
            };
            let color = if table.ram[i] > 1.0 {
                c::ERROR
            } else if table.ram[i] > 0.75 {
                c::WARN
            } else {
                c::TEXT_WEAK
            };
            ui.add(egui::Label::new(small(&text, color)).truncate());
        }
        if let Some(text) = trino_scene::budget::hardware_line(table.targets[1]) {
            ui.add(egui::Label::new(small(&text, c::TEXT_FAINT)).truncate());
        }
    }
}

fn budget_section(
    ui: &mut egui::Ui,
    budgets: &crate::budget::Budgets,
    tmem: &std::collections::BTreeMap<u32, Option<u32>>,
    open: bool,
    game: Target,
) -> Option<String> {
    let line = |ui: &mut egui::Ui, text: String, color: egui::Color32| {
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).truncate());
    };
    let [n64, n3ds, pc, web, ..] = budgets;
    let mut picked = None;
    let pct = n64.n64_frame_percent();
    line(
        ui,
        format!(
            "N64 ≈{:.1} ms ({pct:.0}% of 30 fps)",
            n64.n64_frame_us() as f32 / 1000.0
        ),
        if pct > 100.0 {
            c::ERROR
        } else if pct > 75.0 {
            c::WARN
        } else {
            c::OK
        },
    );
    line(
        ui,
        format!(
            "CPU {:.1} ms on {} draws · RSP {:.1} ms on {} draws",
            n64.n64_cpu_us() as f32 / 1000.0,
            n64.cpu_draws,
            n64.n64_rsp_us() as f32 / 1000.0,
            n64.rsp_draws
        ),
        c::TEXT_WEAK,
    );
    for (r, n) in &n64.off_rsp {
        reason_list(ui, &mut picked, n64, "off the RSP", r.label(), *n, false);
    }
    if n64.billboards > 0 {
        line(
            ui,
            format!("{} impostor billboards", n64.billboards),
            c::TEXT_FAINT,
        );
    }
    line(
        ui,
        format!(
            "3DS {} tris · {} draws · {} billboards",
            n3ds.tris(),
            n3ds.draws,
            n3ds.billboards
        ),
        c::TEXT_WEAK,
    );
    for (r, n) in &n3ds.off_gpu {
        reason_list(
            ui,
            &mut picked,
            n3ds,
            "off the 3DS GPU",
            r.label(),
            *n,
            false,
        );
    }
    // The PC native profile's GPU T&L split (issue #51): the draws the
    // GPU path takes, and each reason with the entities it keeps on the
    // CPU.
    line(ui, trino_scene::budget::pc_gpu_line(pc), c::TEXT_WEAK);
    for (r, n) in &pc.off_pc_gpu {
        reason_list(ui, &mut picked, pc, "off the PC GPU", r.label(), *n, open);
    }
    // The Web (issue #47): the PC's GPU split on the browser's GPU, and
    // the calls a browser pays for, warned past a phone's budget.
    line(ui, trino_scene::budget::web_line(web), c::TEXT_WEAK);
    for w in trino_scene::budget::web_warnings(web) {
        line(ui, w, c::WARN);
    }
    tier_tables(ui, budgets, game);
    // Water (ADR 0015): what the N64 tier spends on each surface.
    let tmem = |id: trino_core::SpriteId| tmem.get(&id.0).copied().flatten();
    for text in trino_scene::budget::water_lines(n64, &tmem) {
        let bad = text.contains("over TMEM") || text.contains("palette layer");
        line(ui, text, if bad { c::ERROR } else { c::TEXT_WEAK });
    }
    let mut wasted: Vec<&String> = n64.wasted.iter().chain(&n3ds.wasted).collect();
    wasted.sort();
    wasted.dedup();
    if !wasted.is_empty() {
        line(
            ui,
            format!("{} draws sent but unseen (outside the view):", wasted.len()),
            c::WARN,
        );
        for w in wasted {
            let r = ui
                .add(
                    egui::Label::new(
                        RichText::new(format!("unseen: {w}"))
                            .size(12.0)
                            .color(c::WARN),
                    )
                    .truncate()
                    .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Select it");
            if r.clicked() {
                picked = Some(w.clone());
            }
        }
    }
    picked
}
