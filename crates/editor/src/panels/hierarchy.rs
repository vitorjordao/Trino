//! Hierarchy: the scene's entity tree (Blender outliner / Unity hierarchy).
//! Drag rows onto each other to parent, onto the empty area to unparent;
//! double-click or F2 renames; right-click for everything else.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use trino_asset_pipeline::{Primitive, Shape};
use trino_scene::{Entity, PrefabInstance};

use crate::app::{App, Cmd};
use crate::dialogs::Dialog;
use crate::theme::{self, c, icon};
use crate::viewport::AssetPayload;

/// Payload for dragging hierarchy rows.
#[derive(Clone, Debug)]
struct RowDrag(Vec<usize>);

struct Row {
    path: Vec<usize>,
    depth: usize,
    name: String,
    glyph: &'static str,
    color: Color32,
    has_children: bool,
    visible: bool,
    prefab: Option<String>,
    /// Display-only rows: parts inside a prefab instance.
    ghost: bool,
}

pub fn entity_glyph(e: &Entity) -> (&'static str, Color32) {
    if e.prefab.is_some() {
        (icon::PACKAGE, c::PREFAB)
    } else if e.model.is_some() {
        (icon::CUBE, c::MODEL)
    } else if e.sprite.is_some() {
        (icon::IMAGE, c::SPRITE)
    } else if !e.children.is_empty() {
        (icon::FOLDER, c::TEXT_WEAK)
    } else {
        (icon::MAP_PIN, c::TEXT_WEAK)
    }
}

fn collect(app: &App, filter: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let needle = filter.to_lowercase();
    fn matches(e: &Entity, needle: &str) -> bool {
        e.name.to_lowercase().contains(needle) || e.children.iter().any(|c| matches(c, needle))
    }
    fn rec(
        app: &App,
        list: &[Entity],
        path: &mut Vec<usize>,
        depth: usize,
        needle: &str,
        rows: &mut Vec<Row>,
    ) {
        for (i, e) in list.iter().enumerate() {
            if !needle.is_empty() && !matches(e, needle) {
                continue;
            }
            path.push(i);
            let (glyph, color) = entity_glyph(e);
            // Prefab instances start collapsed (their parts are read-only).
            let expanded = if e.prefab.is_some() {
                app.expanded.contains(path)
            } else {
                !app.collapsed.contains(path) || !needle.is_empty()
            };
            rows.push(Row {
                path: path.clone(),
                depth,
                name: e.name.clone(),
                glyph,
                color,
                has_children: !e.children.is_empty() || e.prefab.is_some(),
                visible: e.visible,
                prefab: e.prefab.as_ref().map(|p| p.path.clone()),
                ghost: false,
            });
            if expanded {
                // Prefab contents (read-only preview).
                if let Some(inst) = &e.prefab
                    && let Ok((root, _)) = app.lib.expand(inst)
                {
                    for part in &root.children {
                        ghost_rows(part, path, depth + 1, rows);
                    }
                }
                rec(app, &e.children, path, depth + 1, needle, rows);
            }
            path.pop();
        }
    }
    fn ghost_rows(e: &Entity, owner: &[usize], depth: usize, rows: &mut Vec<Row>) {
        let (glyph, _) = entity_glyph(e);
        rows.push(Row {
            path: owner.to_vec(),
            depth,
            name: e.name.clone(),
            glyph,
            color: c::PREFAB.gamma_multiply(0.7),
            has_children: false,
            visible: e.visible,
            prefab: None,
            ghost: true,
        });
        for child in &e.children {
            ghost_rows(child, owner, depth + 1, rows);
        }
    }
    rec(
        app,
        &app.doc.scene.entities,
        &mut Vec::new(),
        0,
        &needle,
        &mut rows,
    );
    rows
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let w = ui.available_width() - 34.0;
        ui.allocate_ui(Vec2::new(w, 26.0), |ui| {
            theme::search_box(ui, &mut app.hierarchy_filter, "Search entities…");
        });
        ui.menu_button(RichText::new(icon::PLUS).size(16.0), |ui| {
            create_menu(app, ui, None);
        })
        .response
        .on_hover_text("Create");
    });
    ui.add_space(6.0);

    let filter = app.hierarchy_filter.clone();
    let rows = collect(app, &filter);
    if rows.is_empty() {
        if app.doc.scene.entities.is_empty() {
            theme::empty_state(
                ui,
                icon::TREE_STRUCTURE,
                "Empty scene",
                "Create something with + or Shift+A",
            );
        } else {
            theme::empty_state(ui, icon::MAGNIFYING_GLASS, "No matches", "Try another name");
        }
    }

    let mut toggles: Vec<Vec<usize>> = Vec::new();
    let mut expand_toggle: Option<(Vec<usize>, bool)> = None;
    let mut rename_commit: Option<(Vec<usize>, String)> = None;
    let selected = app.doc.selected.clone();
    let extra = app.doc.extra.clone();
    let row_h = 24.0;
    for row in &rows {
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), row_h),
            Sense::click_and_drag(),
        );
        let is_sel =
            !row.ghost && (selected.as_ref() == Some(&row.path) || extra.contains(&row.path));
        // Painted by hand below: tell accessibility (screen readers, the
        // kittest UI tests) what the row is.
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, is_sel, &row.name)
        });
        let id = ui.id().with(("row", &row.path, row.ghost, &row.name));
        let t_sel = ui
            .ctx()
            .animate_bool_with_time(id.with("sel"), is_sel, 0.15);
        let t_hov = ui
            .ctx()
            .animate_bool_with_time(id.with("hov"), resp.hovered(), 0.1);
        let painter = ui.painter().clone();
        if t_sel > 0.0 || t_hov > 0.0 {
            let bg = theme::mix(c::BG3.gamma_multiply(t_hov), c::ACCENT_SOFT, t_sel);
            painter.rect_filled(rect, CornerRadius::same(5), bg);
        }
        if t_sel > 0.0 {
            painter.rect_filled(
                Rect::from_min_size(
                    rect.min + Vec2::new(0.0, 4.0),
                    Vec2::new(3.0, rect.height() - 8.0),
                ),
                CornerRadius::same(2),
                c::ACCENT.gamma_multiply(t_sel),
            );
        }
        // Drop target highlight.
        if !row.ghost && resp.dnd_hover_payload::<RowDrag>().is_some() {
            painter.rect_stroke(
                rect,
                CornerRadius::same(5),
                Stroke::new(1.5, c::ACCENT),
                egui::StrokeKind::Inside,
            );
        }

        let x0 = rect.min.x + 6.0 + row.depth as f32 * 14.0;
        let cy = rect.center().y;
        // Expander caret.
        if row.has_children && !row.ghost {
            let open = if row.prefab.is_some() {
                app.expanded.contains(&row.path)
            } else {
                !app.collapsed.contains(&row.path) || !filter.is_empty()
            };
            let caret = Rect::from_center_size(Pos2::new(x0 + 6.0, cy), Vec2::splat(16.0));
            let cresp = ui.interact(caret, id.with("caret"), Sense::click());
            painter.text(
                caret.center(),
                egui::Align2::CENTER_CENTER,
                if open {
                    icon::CARET_DOWN
                } else {
                    icon::CARET_RIGHT
                },
                egui::FontId::proportional(11.0),
                if cresp.hovered() {
                    Color32::WHITE
                } else {
                    c::TEXT_FAINT
                },
            );
            if cresp.clicked() {
                expand_toggle = Some((row.path.clone(), row.prefab.is_some()));
            }
        }
        let alpha = if row.visible { 1.0 } else { 0.4 };
        painter.text(
            Pos2::new(x0 + 20.0, cy),
            egui::Align2::LEFT_CENTER,
            row.glyph,
            egui::FontId::proportional(14.0),
            row.color.gamma_multiply(alpha),
        );

        let renaming = app
            .renaming
            .as_ref()
            .filter(|(p, _)| *p == row.path && !row.ghost)
            .is_some();
        let text_x = x0 + 40.0;
        if renaming {
            let edit_rect = Rect::from_min_max(
                Pos2::new(text_x - 4.0, rect.min.y + 2.0),
                Pos2::new(rect.max.x - 30.0, rect.max.y - 2.0),
            );
            let (_, buf) = app.renaming.as_mut().unwrap();
            let r = ui.put(
                edit_rect,
                egui::TextEdit::singleline(buf).margin(Vec2::new(4.0, 1.0)),
            );
            r.request_focus();
            if r.lost_focus() {
                let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if !cancel {
                    rename_commit = Some((row.path.clone(), buf.clone()));
                }
                app.renaming = None;
            }
        } else {
            let color = if row.ghost {
                c::PREFAB.gamma_multiply(0.75)
            } else if is_sel {
                Color32::WHITE
            } else {
                c::TEXT.gamma_multiply(alpha)
            };
            let clip = Rect::from_min_max(
                Pos2::new(text_x, rect.min.y),
                Pos2::new(rect.max.x - 46.0, rect.max.y),
            );
            let font = if row.ghost {
                egui::FontId::proportional(12.5)
            } else {
                egui::FontId::proportional(13.5)
            };
            painter.with_clip_rect(clip).text(
                Pos2::new(text_x, cy),
                egui::Align2::LEFT_CENTER,
                &row.name,
                font,
                color,
            );
        }

        // Right side: prefab badge + visibility eye.
        if !row.ghost {
            let eye = Rect::from_center_size(Pos2::new(rect.max.x - 14.0, cy), Vec2::splat(18.0));
            let show_eye = resp.hovered() || !row.visible || ui.rect_contains_pointer(eye);
            if show_eye {
                let er = ui.interact(eye, id.with("eye"), Sense::click());
                painter.text(
                    eye.center(),
                    egui::Align2::CENTER_CENTER,
                    if row.visible {
                        icon::EYE
                    } else {
                        icon::EYE_SLASH
                    },
                    egui::FontId::proportional(13.0),
                    if er.hovered() {
                        Color32::WHITE
                    } else {
                        c::TEXT_FAINT
                    },
                );
                if er.clicked() {
                    toggles.push(row.path.clone());
                }
            }
            if row.prefab.is_some() {
                painter.text(
                    Pos2::new(rect.max.x - 34.0, cy),
                    egui::Align2::CENTER_CENTER,
                    icon::LINK,
                    egui::FontId::proportional(12.0),
                    c::PREFAB.gamma_multiply(0.8),
                );
            }
        }

        if row.ghost {
            if resp.clicked() {
                app.cmds.push(Cmd::Select(Some(row.path.clone())));
            }
            continue;
        }
        if resp.clicked() {
            let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            app.cmds.push(if additive {
                Cmd::ToggleSelect(row.path.clone())
            } else {
                Cmd::Select(Some(row.path.clone()))
            });
        }
        if resp.double_clicked() {
            app.renaming = Some((row.path.clone(), row.name.clone()));
        }
        if resp.drag_started() {
            resp.dnd_set_drag_payload(RowDrag(row.path.clone()));
        }
        if let Some(p) = resp.dnd_release_payload::<RowDrag>()
            && p.0 != row.path
        {
            app.cmds.push(Cmd::Reparent {
                from: p.0.clone(),
                to: Some(row.path.clone()),
            });
        }
        if let Some(p) = resp.dnd_release_payload::<AssetPayload>() {
            // Asset dropped on a row: create it as a child.
            app.cmds
                .push(Cmd::AddAssetUnder((*p).clone(), row.path.clone()));
        }
        let path = row.path.clone();
        let prefab = row.prefab.clone();
        resp.context_menu(|ui| {
            context_menu(app, ui, &path, prefab.as_deref());
        });
    }

    // Empty area: drop to unparent, click to deselect.
    let rest = ui.available_rect_before_wrap();
    let rest = Rect::from_min_size(rest.min, Vec2::new(rest.width(), rest.height().max(40.0)));
    let resp = ui.interact(rest, ui.id().with("hier-empty"), Sense::click());
    if resp.dnd_hover_payload::<RowDrag>().is_some() {
        ui.painter().rect_stroke(
            rest.shrink(2.0),
            CornerRadius::same(6),
            Stroke::new(1.0, c::ACCENT.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rest.center_top() + Vec2::new(0.0, 14.0),
            egui::Align2::CENTER_CENTER,
            "Drop here to move to the root",
            egui::FontId::proportional(12.0),
            c::TEXT_WEAK,
        );
    }
    if let Some(p) = resp.dnd_release_payload::<RowDrag>() {
        app.cmds.push(Cmd::Reparent {
            from: p.0.clone(),
            to: None,
        });
    }
    if resp.clicked() {
        app.cmds.push(Cmd::Select(None));
    }
    resp.context_menu(|ui| create_menu(app, ui, None));

    for p in toggles {
        if let Some(e) = app.doc.scene.get_mut(&p) {
            e.visible = !e.visible;
        }
    }
    match expand_toggle {
        Some((p, true)) if !app.expanded.remove(&p) => {
            app.expanded.insert(p);
        }
        Some((p, false)) if !app.collapsed.remove(&p) => {
            app.collapsed.insert(p);
        }
        _ => {}
    }
    if let Some((p, name)) = rename_commit
        && let Some(e) = app.doc.scene.get_mut(&p)
        && !name.trim().is_empty()
    {
        e.name = name.trim().to_string();
    }
}

fn context_menu(app: &mut App, ui: &mut egui::Ui, path: &[usize], prefab: Option<&str>) {
    ui.set_min_width(200.0);
    if ui
        .button(format!("{}  Rename", icon::PENCIL_SIMPLE))
        .clicked()
    {
        let name = app
            .doc
            .scene
            .get(path)
            .map(|e| e.name.clone())
            .unwrap_or_default();
        app.renaming = Some((path.to_vec(), name));
    }
    if ui.button(format!("{}  Duplicate", icon::COPY)).clicked() {
        app.cmds.push(Cmd::Duplicate(path.to_vec()));
    }
    if ui
        .button(format!("{}  Frame in view", icon::CROSSHAIR))
        .clicked()
    {
        app.cmds.push(Cmd::Focus(path.to_vec()));
    }
    if ui.button(format!("{}  Copy as RON", icon::CODE)).clicked() {
        app.cmds.push(Cmd::Copy(path.to_vec()));
    }
    ui.menu_button(format!("{}  Create child", icon::PLUS), |ui| {
        create_menu(app, ui, Some(path.to_vec()));
    });
    ui.separator();
    match prefab {
        Some(p) => {
            if ui
                .button(format!("{}  Edit prefab", icon::PENCIL_SIMPLE))
                .clicked()
            {
                app.cmds.push(Cmd::EditPrefab(p.to_string()));
            }
            if ui
                .button(format!("{}  Unpack (make real)", icon::LINK_BREAK))
                .clicked()
            {
                app.cmds.push(Cmd::Unpack(path.to_vec()));
            }
        }
        None => {
            if ui
                .button(format!("{}  Create prefab from this…", icon::PACKAGE))
                .clicked()
            {
                let name = app
                    .doc
                    .scene
                    .get(path)
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                app.cmds
                    .push(Cmd::Open(Dialog::make_prefab(path.to_vec(), &name)));
            }
        }
    }
    if path.len() > 1
        && ui
            .button(format!("{}  Move to root", icon::ARROW_U_UP_LEFT))
            .clicked()
    {
        app.cmds.push(Cmd::Reparent {
            from: path.to_vec(),
            to: None,
        });
    }
    ui.separator();
    if ui
        .button(RichText::new(format!("{}  Delete", icon::TRASH)).color(c::ERROR))
        .clicked()
    {
        app.cmds.push(Cmd::Delete(path.to_vec()));
    }
}

/// Palette of pleasant placeholder colors, cycled per new fake object.
pub const PROXY_COLORS: [[u8; 4]; 6] = [
    [228, 120, 92, 255],
    [96, 176, 236, 255],
    [130, 210, 120, 255],
    [240, 196, 90, 255],
    [180, 136, 236, 255],
    [220, 222, 228, 255],
];

/// The Create menu (main menu, hierarchy "+", context menus).
pub fn create_menu(app: &mut App, ui: &mut egui::Ui, parent: Option<Vec<usize>>) {
    ui.set_min_width(210.0);
    if ui.button(format!("{}  Empty", icon::MAP_PIN)).clicked() {
        app.cmds.push(Cmd::Create {
            parent: parent.clone(),
            entity: Box::new(Entity::new("empty").at(at(app))),
        });
    }
    if ui
        .button(format!("{}  Spawn point", icon::PERSON))
        .clicked()
    {
        let mut e = Entity::new("spawn").at(at(app));
        e.tags.push("spawn".into());
        app.cmds.push(Cmd::Create {
            parent: parent.clone(),
            entity: Box::new(e),
        });
    }
    ui.separator();
    ui.menu_button(format!("{}  Fake object", icon::GHOST), |ui| {
        ui.label(
            RichText::new("Placeholder meshes — swap for real art later")
                .size(11.5)
                .color(c::TEXT_FAINT),
        );
        for (i, shape) in Shape::all_defaults().into_iter().enumerate() {
            let glyph = shape_glyph(&shape);
            if ui
                .button(format!("{glyph}  {}", capitalize(shape.kind())))
                .clicked()
            {
                let name = app.project.unique_model_name(shape.kind());
                let prim = Primitive {
                    color: PROXY_COLORS[(app.project.models.len() + i) % PROXY_COLORS.len()],
                    shape,
                    ..Primitive::default()
                };
                app.cmds.push(Cmd::CreateProxy(name, prim, true));
            }
        }
        ui.separator();
        if ui
            .button(format!("{}  Custom…  (Shift+A)", icon::SLIDERS_HORIZONTAL))
            .clicked()
        {
            app.cmds.push(Cmd::Open(Dialog::fake_object(&app.project)));
        }
    });
    ui.menu_button(format!("{}  Model", icon::CUBE), |ui| {
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for m in app.project.models.values() {
                    let glyph = if m.is_proxy() {
                        icon::GHOST
                    } else {
                        icon::CUBE
                    };
                    if ui.button(format!("{glyph}  {}", m.name)).clicked() {
                        let mut e = Entity::new(m.name.clone())
                            .with_model(trino_scene::ModelComponent::new(m.logical.clone()));
                        e.transform.pos = at(app);
                        app.cmds.push(Cmd::Create {
                            parent: parent.clone(),
                            entity: Box::new(e),
                        });
                    }
                }
            });
    });
    ui.menu_button(format!("{}  Prefab", icon::PACKAGE), |ui| {
        if app.lib.prefabs.is_empty() {
            ui.label(
                RichText::new("No prefabs yet — right-click an entity › Create prefab")
                    .color(c::TEXT_FAINT),
            );
        }
        for l in app.lib.prefabs.keys() {
            if ui.button(format!("{}  {l}", icon::PACKAGE)).clicked() {
                let mut e = Entity::new(l.rsplit('/').next().unwrap_or("prefab")).at(at(app));
                e.prefab = Some(PrefabInstance::new(l.clone()));
                app.cmds.push(Cmd::Create {
                    parent: parent.clone(),
                    entity: Box::new(e),
                });
            }
        }
    });
    ui.menu_button(format!("{}  Sprite (2D)", icon::IMAGE), |ui| {
        for s in app.project.sprites.values() {
            if ui
                .button(format!("{}  {}", icon::IMAGE, s.logical))
                .clicked()
            {
                let mut e = Entity::new(s.logical.rsplit('/').next().unwrap_or("sprite"));
                e.sprite = Some(trino_scene::SpriteComponent::new(s.logical.clone()));
                e.transform.pos = [144.0, 104.0, 0.0];
                app.cmds.push(Cmd::Create {
                    parent: parent.clone(),
                    entity: Box::new(e),
                });
            }
        }
    });
    ui.separator();
    if ui.button(format!("{}  Material…", icon::PALETTE)).clicked() {
        app.cmds.push(Cmd::Open(Dialog::new_material()));
    }
}

fn at(app: &App) -> [f32; 3] {
    let t = app.scene_view.cam.target;
    [(t.x * 2.0).round() / 2.0, 0.0, (t.z * 2.0).round() / 2.0]
}

pub fn shape_glyph(shape: &Shape) -> &'static str {
    match shape {
        Shape::Box { .. } => icon::CUBE,
        Shape::Sphere { .. } => icon::SPHERE,
        Shape::Cylinder { .. } => icon::CYLINDER,
        Shape::Cone { .. } => icon::TRIANGLE,
        Shape::Capsule { .. } => icon::PILL,
        Shape::Plane { .. } => icon::SQUARE,
        Shape::Ramp { .. } => icon::STAIRS,
        Shape::Torus { .. } => icon::CIRCLE,
    }
}

pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
