//! The UI tab (issue #58): the project's screens (`ui/*.ui.ron`) edited
//! the way a target shows them. The screen, the target (N64, 3DS, the PC
//! at 1080p, 4K or 21:9, the Web) and the language at the top; the node
//! tree on the left (add, duplicate, delete, move, drag to reorder or
//! reparent); the preview — the target's renderer drawing the screen, the
//! pad's focus order, the selected node and overflowing texts over it, a
//! click selecting the node under the pointer — with `ui check`'s issues
//! under it (a click selects the node); every setting of the selected
//! node and of the screen on the right. Edits are written once the user
//! settles, an edit on disk shows here live (`App::sync_files`).

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use trino_core::ui::{UiStrings, flags};
use trino_scene::ui::{
    Align, Anchor, Dir, ImageFit, Justify, Kind, Level, NodeDef, Overflow, Platform, Sample,
    Scaling, ScreenDef, Side, Size, TextAlign, VAlign, screen_name,
};

use crate::app::App;
use crate::theme::{self, c, icon};
use crate::ui_screens::{self as screens, Drop, TARGETS};

/// Payload for dragging tree rows.
#[derive(Clone, Debug)]
struct NodeDrag(Vec<usize>);

/// A tree button's edit: the screen and the selection in, the new
/// selection out (`None`: nothing to do).
type TreeEdit = fn(&mut ScreenDef, &[usize]) -> Option<Vec<usize>>;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let lang_before = app.ui_screens.lang.clone();
    toolbar(app, ui);
    // The language picked here is the editor's preview language (#59).
    if app.ui_screens.lang != lang_before {
        app.localization.preview = app.ui_screens.lang.clone();
    }
    if app.ui_screens.new_name.is_some() {
        new_screen_form(app, ui);
    }
    ui.add_space(4.0);
    if app.ui_screens.def.is_none() {
        theme::empty_state(
            ui,
            icon::LAYOUT,
            "No UI screen open",
            "New screen writes ui/<name>.ui.ron (or write one by hand or with an agent)",
        );
        return;
    }
    let avail = ui.available_rect_before_wrap();
    let gap = 6.0;
    let view_rect = if avail.width() >= 760.0 {
        // Wide: the tree | the preview over the issues | the inspector.
        let tree_w = (avail.width() * 0.24).clamp(150.0, 260.0);
        let side_w = (avail.width() * 0.3).clamp(190.0, 340.0);
        let tree_rect = Rect::from_min_size(avail.min, Vec2::new(tree_w, avail.height()));
        let side_rect = Rect::from_min_max(Pos2::new(avail.max.x - side_w, avail.min.y), avail.max);
        let center = Rect::from_min_max(
            Pos2::new(tree_rect.max.x + gap, avail.min.y),
            Pos2::new(side_rect.min.x - gap, avail.max.y),
        );
        // The issues take what they need, up to a third of the column.
        let rows = app.ui_screens.issues.len().max(1) as f32;
        let issues_h = (34.0 + rows * 18.0).clamp(56.0, (center.height() / 3.0).max(56.0));
        let view_rect = Rect::from_min_max(
            center.min,
            Pos2::new(center.max.x, center.max.y - issues_h - gap),
        );
        let issues_rect =
            Rect::from_min_max(Pos2::new(center.min.x, view_rect.max.y + gap), center.max);
        column(ui, tree_rect, "ui-tree", |ui| tree(app, ui));
        column(ui, side_rect, "ui-inspector", |ui| inspector(app, ui));
        column(ui, issues_rect, "ui-issues", |ui| issues(app, ui));
        view_rect
    } else {
        // Narrow: the preview over the tree and the issues | the inspector.
        let side_w = (avail.width() * 0.42).clamp(170.0, 260.0);
        let side_rect = Rect::from_min_max(Pos2::new(avail.max.x - side_w, avail.min.y), avail.max);
        let left = Rect::from_min_max(avail.min, Pos2::new(side_rect.min.x - gap, avail.max.y));
        let view_h = (left.height() * 0.5).max(96.0);
        let view_rect = Rect::from_min_size(left.min, Vec2::new(left.width(), view_h));
        let rest = Rect::from_min_max(Pos2::new(left.min.x, view_rect.max.y + gap), left.max);
        column(ui, rest, "ui-tree", |ui| {
            tree(app, ui);
            ui.add_space(8.0);
            issues(app, ui);
        });
        column(ui, side_rect, "ui-inspector", |ui| inspector(app, ui));
        view_rect
    };
    // Laid out and drawn after this frame's edits: the preview shows them.
    let rs = app.render_state.clone();
    app.ui_screens.access = app.access.preview;
    app.ui_screens.refresh(ui.ctx(), &rs, &app.project);
    preview(app, ui, view_rect);
    ui.allocate_rect(avail, Sense::hover());
}

/// A scrolling column of the tab in `rect`.
fn column(ui: &mut egui::Ui, rect: Rect, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            ui.set_max_width(ui.available_width() - 10.0);
            add(ui);
        });
}

fn named(r: &egui::Response, typ: egui::WidgetType, name: &str) {
    r.widget_info(|| egui::WidgetInfo::labeled(typ, true, name));
}

// ------------------------------------------------------------- toolbar --

fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    let s = &mut app.ui_screens;
    let mut open = None;
    ui.horizontal_wrapped(|ui| {
        let name = s.name();
        let w = 140.0f32.min(ui.available_width() - 8.0);
        theme::fixed_width(ui, w, |ui| {
            let r = egui::ComboBox::from_id_salt("ui-screen")
                .selected_text(if name.is_empty() { "—" } else { &name })
                .width(w)
                .truncate()
                .show_ui(ui, |ui| {
                    for f in &s.files {
                        let on = s.path.as_ref() == Some(f);
                        if ui.selectable_label(on, screen_name(f)).clicked() && !on {
                            open = Some(f.clone());
                        }
                    }
                })
                .response;
            named(&r, egui::WidgetType::ComboBox, "UI screen");
        });
        if theme::named_icon_button(ui, icon::FILE_PLUS, "New screen").clicked() {
            s.new_name = match s.new_name {
                Some(_) => None,
                None => Some(String::new()),
            };
        }
        ui.separator();
        let w = 104.0f32.min(ui.available_width() - 8.0);
        theme::fixed_width(ui, w, |ui| {
            let r = egui::ComboBox::from_id_salt("ui-target")
                .selected_text(TARGETS[s.target].label)
                .width(w)
                .truncate()
                .show_ui(ui, |ui| {
                    for (i, t) in TARGETS.iter().enumerate() {
                        let size = match t.size {
                            Some((w, h)) => format!("{w}×{h}"),
                            None if i == 1 => "400×240 + 320×240".into(),
                            None => "320×240".into(),
                        };
                        let r = ui
                            .selectable_label(s.target == i, t.label)
                            .on_hover_text(size);
                        if r.clicked() {
                            s.target = i;
                        }
                    }
                })
                .response
                .on_hover_text("The target the preview and the issues are for");
            named(&r, egui::WidgetType::ComboBox, "UI target");
        });
        let w = 112.0f32.min(ui.available_width() - 8.0);
        theme::fixed_width(ui, w, |ui| {
            let r = egui::ComboBox::from_id_salt("ui-lang")
                .selected_text(s.lang.as_deref().unwrap_or("Default text"))
                .width(w)
                .truncate()
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(s.lang.is_none(), "Default text")
                        .clicked()
                    {
                        s.lang = None;
                    }
                    for l in s.langs.names() {
                        let on = s.lang.as_deref() == Some(l.as_str());
                        if ui.selectable_label(on, &l).clicked() {
                            s.lang = Some(l);
                        }
                    }
                })
                .response
                .on_hover_text("The string table the texts come from (assets/i18n/)");
            named(&r, egui::WidgetType::ComboBox, "UI language");
        });
        ui.separator();
        for (on, glyph, name, tip) in [
            (
                &mut s.show_focus,
                icon::LIST_NUMBERS,
                "Show focus order",
                "Number the controls in the order the pad's Down (then Right) visits them",
            ),
            (
                &mut s.show_overflow,
                icon::WARNING,
                "Show overflow",
                "Highlight texts past their box",
            ),
        ] {
            let r = theme::toggle_icon(ui, *on, glyph, tip);
            let state = *on;
            r.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, state, name)
            });
            if r.clicked() {
                *on = !*on;
            }
        }
        let (text, color) = if s.dirty {
            ("unsaved".to_string(), c::WARN)
        } else if let Some(p) = &s.path {
            (p.display().to_string(), c::TEXT_FAINT)
        } else {
            (String::new(), c::TEXT_FAINT)
        };
        ui.add(egui::Label::new(RichText::new(text).size(11.5).color(color)).truncate());
    });
    if let Some(f) = open {
        s.open(f);
    }
    for e in [&s.error, &s.langs_error].into_iter().flatten() {
        ui.add(
            egui::Label::new(
                RichText::new(format!("{} {e}", icon::WARNING))
                    .size(11.5)
                    .color(c::ERROR),
            )
            .wrap(),
        );
    }
}

fn new_screen_form(app: &mut App, ui: &mut egui::Ui) {
    let root = app.project.root.clone();
    let s = &mut app.ui_screens;
    let (mut create, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        let Some(name) = s.new_name.as_mut() else {
            return;
        };
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new("ui/").color(c::TEXT_WEAK));
        let w = (ui.available_width() - 150.0).clamp(60.0, 180.0);
        let r = ui.add(
            egui::TextEdit::singleline(name)
                .hint_text("name")
                .desired_width(w),
        );
        named(&r, egui::WidgetType::TextEdit, "New screen name");
        create |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.label(RichText::new(".ui.ron").color(c::TEXT_WEAK));
        let r = ui
            .button(format!("{}  Create", icon::CHECK))
            .on_hover_text("A backdrop, a title and a Back button to start from");
        named(&r, egui::WidgetType::Button, "Create screen");
        create |= r.clicked();
        cancel |= theme::named_icon_button(ui, icon::X, "Cancel new screen").clicked();
    });
    if create {
        let name = s.new_name.clone().unwrap_or_default();
        match s.create(&root, &name) {
            Ok(()) => s.new_name = None,
            Err(e) => app.log.toast(crate::log::Level::Error, e),
        }
    } else if cancel {
        s.new_name = None;
    }
}

// ---------------------------------------------------------------- tree --

pub fn kind_glyph(kind: &Kind) -> &'static str {
    match kind {
        Kind::Overlay => icon::SELECTION_ALL,
        Kind::Stack { .. } => icon::ROWS,
        Kind::Grid { .. } => icon::GRID_FOUR,
        Kind::Scroll { .. } => icon::SCROLL,
        Kind::List { .. } => icon::LIST_BULLETS,
        Kind::Text => icon::TEXT_T,
        Kind::Image { .. } => icon::IMAGE,
        Kind::Rect { .. } => icon::RECTANGLE,
        Kind::Button => icon::CURSOR_CLICK,
        Kind::Toggle => icon::TOGGLE_LEFT,
        Kind::Slider { .. } => icon::SLIDERS,
        Kind::Model3d { .. } => icon::CUBE,
    }
}

/// Where a drop on `rect` at height `y` puts the node: before or after a
/// row, or into a container (its middle half).
fn drop_zone(rect: Rect, y: f32, path: &[usize], container: bool) -> Drop {
    let t = (y - rect.min.y) / rect.height().max(1.0);
    if path.is_empty() || (container && (0.25..=0.75).contains(&t)) {
        Drop::Into(path.to_vec())
    } else if t < 0.5 {
        Drop::Before(path.to_vec())
    } else {
        Drop::After(path.to_vec())
    }
}

fn tree(app: &mut App, ui: &mut egui::Ui) {
    let s = &mut app.ui_screens;
    let Some(def) = &s.def else {
        return;
    };
    let mut add = None;
    let mut action: Option<TreeEdit> = None;
    let sel = s.selected.clone();
    let movable = sel.as_ref().is_some_and(|p| !p.is_empty());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let r = ui
            .menu_button(format!("{}  Add", icon::PLUS), |ui| {
                for kind in Kind::all() {
                    let name = kind.name();
                    let r = ui.button(format!("{}  {name}", kind_glyph(&kind)));
                    named(&r, egui::WidgetType::Button, &format!("Add {name}"));
                    if r.clicked() {
                        add = Some(kind);
                    }
                }
            })
            .response
            .on_hover_text("Add a node: into the selected container, else after the selected node");
        named(&r, egui::WidgetType::Button, "Add node");
        for (glyph, name, f) in [
            (icon::COPY, "Duplicate node", screens::duplicate as TreeEdit),
            (icon::TRASH, "Delete node", screens::delete),
            (icon::ARROW_UP, "Move node up", |d, p| {
                screens::shift(d, p, -1)
            }),
            (icon::ARROW_DOWN, "Move node down", |d, p| {
                screens::shift(d, p, 1)
            }),
        ] {
            let r = ui
                .add_enabled(
                    movable,
                    egui::Button::new(RichText::new(glyph).size(15.0))
                        .frame_when_inactive(false)
                        .min_size(Vec2::new(26.0, 24.0)),
                )
                .on_hover_text(name);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, movable, name));
            if r.clicked() {
                action = Some(f);
            }
        }
    });
    ui.add_space(4.0);

    let name = s.name();
    let flagged: Vec<(usize, Level)> = s
        .issues
        .iter()
        .filter_map(|i| i.node.map(|n| (n, i.level)))
        .collect();
    let mut select = None;
    let mut moved = None;
    let row_h = 22.0;
    for (i, (path, n)) in def.nodes().into_iter().enumerate() {
        let label = if path.is_empty() {
            format!("{name} (root)")
        } else {
            n.label()
        };
        let (rect, resp) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), row_h),
            Sense::click_and_drag(),
        );
        let is_sel = sel.as_deref() == Some(path.as_slice());
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, is_sel, &label)
        });
        let painter = ui.painter();
        if is_sel {
            painter.rect_filled(rect, CornerRadius::same(5), c::ACCENT_SOFT);
        } else if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(5), c::BG3);
        }
        let container = n.kind.is_container();
        if resp.dnd_hover_payload::<NodeDrag>().is_some()
            && let Some(pos) = ui.ctx().pointer_latest_pos()
        {
            let stroke = Stroke::new(2.0, c::ACCENT);
            match drop_zone(rect, pos.y, &path, container) {
                Drop::Into(_) => painter.rect_stroke(
                    rect,
                    CornerRadius::same(5),
                    stroke,
                    egui::StrokeKind::Inside,
                ),
                Drop::Before(_) => painter.hline(rect.x_range(), rect.min.y, stroke),
                Drop::After(_) => painter.hline(rect.x_range(), rect.max.y, stroke),
            };
        }
        let x0 = rect.min.x + 6.0 + path.len() as f32 * 12.0;
        let cy = rect.center().y;
        painter.text(
            Pos2::new(x0, cy),
            egui::Align2::LEFT_CENTER,
            if path.is_empty() {
                icon::LAYOUT
            } else {
                kind_glyph(&n.kind)
            },
            egui::FontId::proportional(13.0),
            if n.kind.is_focusable() {
                c::ACCENT_TEXT
            } else {
                c::TEXT_WEAK
            },
        );
        let flag = flagged
            .iter()
            .filter(|(k, _)| *k == i)
            .map(|(_, l)| *l)
            .min();
        let clip = Rect::from_min_max(
            Pos2::new(x0 + 18.0, rect.min.y),
            Pos2::new(
                rect.max.x - if flag.is_some() { 20.0 } else { 4.0 },
                rect.max.y,
            ),
        );
        let faint = n.screen == Side::Bottom || !n.visible_on.is_empty();
        painter.with_clip_rect(clip).text(
            Pos2::new(x0 + 18.0, cy),
            egui::Align2::LEFT_CENTER,
            &label,
            egui::FontId::proportional(13.0),
            if is_sel {
                Color32::WHITE
            } else if faint {
                c::TEXT.gamma_multiply(0.75)
            } else {
                c::TEXT
            },
        );
        if let Some(level) = flag {
            painter.text(
                Pos2::new(rect.max.x - 10.0, cy),
                egui::Align2::CENTER_CENTER,
                icon::WARNING,
                egui::FontId::proportional(12.0),
                if level == Level::Error {
                    c::ERROR
                } else {
                    c::WARN
                },
            );
        }
        if resp.clicked() {
            select = Some(path.clone());
        }
        if resp.drag_started() && !path.is_empty() {
            resp.dnd_set_drag_payload(NodeDrag(path.clone()));
        }
        if let Some(p) = resp.dnd_release_payload::<NodeDrag>()
            && let Some(pos) = ui.ctx().pointer_latest_pos()
        {
            moved = Some((p.0.clone(), drop_zone(rect, pos.y, &path, container)));
        }
    }
    if let Some(p) = select {
        s.selected = Some(p);
    }
    if let Some(kind) = add {
        s.edit_tree(|d, sel| {
            let node = screens::new_node(d, kind);
            Some(screens::add(d, sel, node))
        });
    }
    if let Some(f) = action {
        s.edit_tree(|d, sel| f(d, sel?).map(Some));
    }
    if let Some((from, to)) = moved {
        s.edit_tree(|d, _| screens::move_node(d, &from, &to).map(Some));
    }
}

// ------------------------------------------------------------ preview --

fn preview(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    let s = &mut app.ui_screens;
    let sel = s.selected_index();
    let (show_focus, show_overflow) = (s.show_focus, s.show_overflow);
    let target = &TARGETS[s.target];
    let resp = ui.allocate_rect(rect, Sense::click());
    named(&resp, egui::WidgetType::Image, "UI preview");
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(6), c::BG0);
    let Some(p) = s.preview.as_mut() else {
        return;
    };
    let ppp = ui.ctx().pixels_per_point();
    let placed = screens::place(rect.shrink(8.0), ppp, p.size, p.bottom_size);
    p.placed = Some(placed);
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    if placed.bottom.is_some() {
        // The 3DS's bezel around the narrower touch screen.
        painter.rect_filled(placed.all, CornerRadius::same(2), Color32::BLACK);
    }
    // Whole pixels when shown at their size or larger, smoothed smaller.
    let smooth = placed.scale < 1.0;
    painter.image(p.texture(smooth), placed.top, uv, Color32::WHITE);
    if let (Some(b), Some(t)) = (placed.bottom, p.bottom_texture(smooth)) {
        painter.image(t, b, uv, Color32::WHITE);
    }
    // The screens' edges (a dark screen on the dark panel).
    for r in [Some(placed.top), placed.bottom].into_iter().flatten() {
        painter.rect_stroke(
            r,
            CornerRadius::ZERO,
            Stroke::new(1.0, c::BORDER),
            egui::StrokeKind::Outside,
        );
    }
    let hovered = resp.hover_pos().and_then(|pos| p.slot_at(pos));
    let slots = p.ui.slots().to_vec();
    for (i, slot) in slots.iter().enumerate() {
        if p.hidden(i) {
            continue;
        }
        let Some(r) = p.slot_rect(i) else {
            continue;
        };
        if show_overflow && slot.has(flags::OVERFLOW) {
            painter.rect_filled(r, CornerRadius::ZERO, c::ERROR.gamma_multiply(0.22));
            painter.rect_stroke(
                r,
                CornerRadius::ZERO,
                Stroke::new(1.5, c::ERROR),
                egui::StrokeKind::Outside,
            );
        }
        if Some(slot.node as usize) == sel {
            painter.rect_stroke(
                r,
                CornerRadius::ZERO,
                Stroke::new(2.0, c::ACCENT),
                egui::StrokeKind::Outside,
            );
        } else if hovered == Some(i) {
            painter.rect_stroke(
                r,
                CornerRadius::ZERO,
                Stroke::new(1.0, Color32::from_white_alpha(140)),
                egui::StrokeKind::Outside,
            );
        }
    }
    if show_focus {
        // Smaller badges on a small picture.
        let radius = (placed.top.height() / 30.0).clamp(4.0, 8.0);
        for (k, &i) in p.order.iter().enumerate() {
            let Some(r) = p.slot_rect(i) else {
                continue;
            };
            let at = r.left_top() + Vec2::new(-2.0, -2.0);
            painter.circle_filled(at, radius, c::WARN);
            painter.circle_stroke(at, radius, Stroke::new(1.0, c::BG0));
            painter.text(
                at,
                egui::Align2::CENTER_CENTER,
                (k + 1).to_string(),
                egui::FontId::new(radius * 1.3, theme::bold()),
                c::BG0,
            );
        }
    }
    // The target, its size, the node under the pointer.
    let (w, h) = p.size;
    let mut caption = match p.bottom_size {
        Some((bw, bh)) => format!("{} · {w}×{h} + {bw}×{bh}", target.label),
        None => format!("{} · {w}×{h}", target.label),
    };
    let node_of = |i: usize| p.ui.slots().get(i).map(|s| s.node as usize);
    let labels = s
        .def
        .as_ref()
        .map(|d| {
            d.nodes()
                .into_iter()
                .map(|(_, n)| n.label())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(label) = hovered.and_then(node_of).and_then(|n| labels.get(n)) {
        caption = format!("{caption} · {label}");
    }
    if placed.all.max.y + 18.0 < rect.max.y {
        painter.text(
            rect.left_bottom() + Vec2::new(8.0, -6.0),
            egui::Align2::LEFT_BOTTOM,
            caption,
            egui::FontId::proportional(11.5),
            c::TEXT_WEAK,
        );
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(node) = p.slot_at(pos).and_then(node_of)
    {
        s.selected = p.paths.get(node).cloned();
    }
}

// ------------------------------------------------------------- issues --

fn issues(app: &mut App, ui: &mut egui::Ui) {
    let s = &mut app.ui_screens;
    let target = TARGETS[s.target].label;
    let lang = s.lang.as_deref().unwrap_or("default text");
    let overflow = s.overflowing().len();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{}  Issues · {target}, {lang}", icon::WARNING))
                .family(theme::bold())
                .color(c::TEXT),
        );
        let st = s.stats;
        let mut text = format!("{} nodes · {} draws", st.nodes, st.draws);
        if overflow > 0 {
            text = format!("{text} · {overflow} overflowing");
        }
        ui.label(RichText::new(text).size(11.5).color(if overflow > 0 {
            c::WARN
        } else {
            c::TEXT_FAINT
        }));
    });
    if s.issues.is_empty() {
        ui.label(
            RichText::new(format!("{}  No issues on {target}", icon::CHECK_CIRCLE)).color(c::OK),
        );
        return;
    }
    let mut select = None;
    for issue in &s.issues {
        let (glyph, color) = match issue.level {
            Level::Error => (icon::X_CIRCLE, c::ERROR),
            Level::Warning => (icon::WARNING, c::WARN),
        };
        let r = ui
            .add(
                egui::Label::new(
                    RichText::new(format!("{glyph} {}: {}", issue.label, issue.message))
                        .size(12.0)
                        .color(color),
                )
                .truncate()
                .sense(Sense::click()),
            )
            .on_hover_text(issue.to_string());
        if r.clicked()
            && let Some(n) = issue.node
        {
            select = Some(n);
        }
    }
    if let Some(n) = select {
        s.select_index(n);
    }
}

// ----------------------------------------------------------- inspector --

/// What the inspector's pickers offer.
struct Choices {
    fonts: Vec<String>,
    sprites: Vec<String>,
    models: Vec<String>,
    /// Controls with an id (focus and nav targets).
    controls: Vec<String>,
}

fn inspector(app: &mut App, ui: &mut egui::Ui) {
    let s = &mut app.ui_screens;
    let Some(def) = &s.def else {
        return;
    };
    let choices = Choices {
        fonts: app.project.fonts.keys().cloned().collect(),
        sprites: app
            .project
            .sprites
            .keys()
            .filter(|k| k.starts_with("sprites/"))
            .cloned()
            .collect(),
        models: app.project.models.keys().cloned().collect(),
        controls: def
            .nodes()
            .into_iter()
            .filter(|(_, n)| n.kind.is_focusable() && !n.id.is_empty())
            .map(|(_, n)| n.id.clone())
            .collect(),
    };
    let mut edited = def.clone();
    match s.selected.as_ref().and_then(|p| edited.root.at_mut(p)) {
        Some(n) => {
            let translated = s.lang.as_ref().filter(|_| !n.key.is_empty()).map(|l| {
                (
                    l.clone(),
                    s.langs.strings(l).get(&n.key).map(str::to_string),
                )
            });
            node_sections(ui, n, &choices, translated);
        }
        None => {
            ui.label(
                RichText::new("Select a node in the tree or the preview")
                    .size(12.0)
                    .color(c::TEXT_FAINT),
            );
            ui.add_space(6.0);
        }
    }
    screen_sections(ui, &mut edited, &choices);
    if s.def.as_ref() != Some(&edited) {
        s.def = Some(edited);
        s.changed();
    }
}

fn text(ui: &mut egui::Ui, name: &str, v: &mut String, hint: &str) {
    let r = ui.add(
        egui::TextEdit::singleline(v)
            .hint_text(hint)
            .desired_width(ui.available_width()),
    );
    named(&r, egui::WidgetType::TextEdit, name);
}

fn num<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    name: &str,
    v: &mut N,
    range: std::ops::RangeInclusive<N>,
) {
    let r = ui.add(egui::DragValue::new(v).range(range).speed(0.25));
    named(&r, egui::WidgetType::DragValue, name);
}

fn check(ui: &mut egui::Ui, name: &str, v: &mut bool, text: &str) {
    let r = ui.checkbox(v, text);
    let on = *v;
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, name));
}

/// A combo of `options` (value, label) named `name`.
fn combo<T: PartialEq + Clone>(ui: &mut egui::Ui, name: &str, v: &mut T, options: &[(T, String)]) {
    let current = options
        .iter()
        .find(|(o, _)| o == v)
        .map_or("—", |(_, l)| l.as_str());
    let w = ui.available_width().min(170.0);
    let r = egui::ComboBox::from_id_salt(name)
        .selected_text(current)
        .width(w)
        .truncate()
        .show_ui(ui, |ui| {
            for (o, l) in options {
                if ui.selectable_label(o == v, l).clicked() {
                    *v = o.clone();
                }
            }
        })
        .response;
    named(&r, egui::WidgetType::ComboBox, name);
}

fn labels<T: Copy>(all: &[T], label: fn(T) -> &'static str) -> Vec<(T, String)> {
    all.iter().map(|v| (*v, label(*v).to_string())).collect()
}

/// A path picker: `none` for the empty string, then `paths`.
fn path_combo(ui: &mut egui::Ui, name: &str, v: &mut String, paths: &[String], none: &str) {
    let mut options = vec![(String::new(), none.to_string())];
    options.extend(paths.iter().map(|p| (p.clone(), p.clone())));
    if !v.is_empty() && !paths.contains(v) {
        // A path the project lacks: shown, so it can be replaced.
        options.push((v.clone(), format!("{v} (missing)")));
    }
    combo(ui, name, v, &options);
}

/// An optional color: unset (the theme's) or a color.
fn color(ui: &mut egui::Ui, name: &str, v: &mut Option<[u8; 4]>, default: trino_core::Color) {
    let mut on = v.is_some();
    check(ui, name, &mut on, "");
    if on != v.is_some() {
        *v = on.then_some([default.r, default.g, default.b, default.a]);
    }
    match v {
        Some(rgba) => {
            theme::color_rgba(ui, rgba);
        }
        None => {
            ui.label(RichText::new("theme").size(11.5).color(c::TEXT_FAINT));
        }
    }
}

fn size_row(ui: &mut egui::Ui, name: &str, v: &mut Size) {
    let mut kind = match v {
        Size::Fit => 0,
        Size::Px(_) => 1,
        Size::Percent(_) => 2,
        Size::Fill => 3,
    };
    let options: Vec<(u8, String)> = ["Fit", "Px", "%", "Fill"]
        .iter()
        .enumerate()
        .map(|(i, l)| (i as u8, l.to_string()))
        .collect();
    theme::fixed_width(ui, 64.0, |ui| combo(ui, name, &mut kind, &options));
    let value = match *v {
        Size::Px(n) | Size::Percent(n) => n,
        _ => 0,
    };
    *v = match kind {
        0 => Size::Fit,
        1 => Size::Px(if matches!(v, Size::Px(_)) { value } else { 32 }),
        2 => Size::Percent(if matches!(v, Size::Percent(_)) {
            value
        } else {
            50
        }),
        _ => Size::Fill,
    };
    match v {
        Size::Px(n) => num(ui, &format!("{name} px"), n, 0..=4096),
        Size::Percent(n) => num(ui, &format!("{name} percent"), n, 0..=100),
        _ => {}
    }
}

fn kind_options() -> Vec<(&'static str, String)> {
    Kind::all()
        .iter()
        .map(|k| (k.name(), format!("{}  {}", kind_glyph(k), k.name())))
        .collect()
}

fn platform_label(p: Platform) -> &'static str {
    match p {
        Platform::Pc => "PC",
        Platform::N64 => "N64",
        Platform::N3ds => "3DS",
        Platform::Web => "Web",
    }
}

fn node_sections(
    ui: &mut egui::Ui,
    n: &mut NodeDef,
    choices: &Choices,
    translated: Option<(String, Option<String>)>,
) {
    let theme = trino_core::ui::Theme::DEFAULT;
    let title = n.label();
    theme::component(
        ui,
        "ui-node",
        kind_glyph(&n.kind),
        c::ACCENT_TEXT,
        &title,
        |ui| {
            let mut kind = n.kind.name();
            theme::prop(ui, "kind", |ui| {
                combo(ui, "Node kind", &mut kind, &kind_options())
            });
            if kind != n.kind.name()
                && let Some(k) = Kind::all().into_iter().find(|k| k.name() == kind)
            {
                n.kind = k;
            }
            theme::prop(ui, "id", |ui| {
                text(ui, "Node id", &mut n.id, "focus, nav, events")
            });
            kind_settings(ui, &mut n.kind, choices);
            theme::prop(ui, "color", |ui| {
                let default = match n.kind {
                    Kind::Text => theme.text,
                    Kind::Button => theme.button,
                    _ => theme.panel,
                };
                color(ui, "Node color", &mut n.color, default)
            });
        },
    );
    if n.kind.has_text() {
        theme::component(ui, "ui-node-text", icon::TEXT_T, c::TEXT, "Text", |ui| {
            theme::prop(ui, "text", |ui| {
                text(
                    ui,
                    "Node text",
                    &mut n.text,
                    "default language ({} = value)",
                )
            });
            theme::prop(ui, "key", |ui| {
                text(ui, "Node key", &mut n.key, "string table key")
            });
            if let Some((lang, t)) = translated {
                let (line, color) = match t {
                    Some(t) => (format!("{lang}: {t}"), c::TEXT_WEAK),
                    None => (
                        format!("{lang}: no entry (the default text shows)"),
                        c::WARN,
                    ),
                };
                ui.add(egui::Label::new(RichText::new(line).size(11.5).color(color)).truncate());
            }
            theme::prop(ui, "font", |ui| {
                path_combo(ui, "Node font", &mut n.font, &choices.fonts, "theme's")
            });
            theme::prop(ui, "scale", |ui| {
                num(ui, "Node text scale", &mut n.text_scale, 0..=8)
            });
            theme::prop(ui, "align", |ui| {
                combo(
                    ui,
                    "Node align",
                    &mut n.align,
                    &labels(TextAlign::ALL, TextAlign::label),
                )
            });
            theme::prop(ui, "valign", |ui| {
                combo(
                    ui,
                    "Node valign",
                    &mut n.valign,
                    &labels(VAlign::ALL, VAlign::label),
                )
            });
            theme::prop(ui, "overflow", |ui| {
                combo(
                    ui,
                    "Node overflow",
                    &mut n.overflow,
                    &labels(Overflow::ALL, Overflow::label),
                )
            });
        });
    }
    theme::component(
        ui,
        "ui-node-layout",
        icon::BOUNDING_BOX,
        c::TEXT,
        "Layout",
        |ui| {
            theme::prop(ui, "screen", |ui| {
                combo(
                    ui,
                    "Node screen",
                    &mut n.screen,
                    &labels(Side::ALL, Side::label),
                )
            });
            theme::prop(ui, "anchor", |ui| {
                combo(
                    ui,
                    "Node anchor",
                    &mut n.anchor,
                    &labels(Anchor::ALL, Anchor::label),
                )
            });
            theme::prop(ui, "offset", |ui| {
                num(ui, "Node offset x", &mut n.offset.0, -4096..=4096);
                num(ui, "Node offset y", &mut n.offset.1, -4096..=4096);
            });
            theme::prop(ui, "width", |ui| size_row(ui, "Node width", &mut n.size.0));
            theme::prop(ui, "height", |ui| {
                size_row(ui, "Node height", &mut n.size.1)
            });
            theme::prop(ui, "min", |ui| {
                num(ui, "Node min width", &mut n.min.0, 0..=4096);
                num(ui, "Node min height", &mut n.min.1, 0..=4096);
            });
            theme::prop(ui, "max", |ui| {
                num(ui, "Node max width", &mut n.max.0, 0..=4096);
                num(ui, "Node max height", &mut n.max.1, 0..=4096);
            });
            theme::prop(ui, "padding", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, side) in ["left", "top", "right", "bottom"].iter().enumerate() {
                        num(
                            ui,
                            &format!("Node padding {side}"),
                            &mut n.padding[i],
                            0..=255,
                        );
                    }
                });
            });
            theme::prop(ui, "bleed", |ui| {
                check(ui, "Node bleed", &mut n.bleed, "past the safe area")
            });
            theme::prop(ui, "on", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for p in Platform::ALL {
                        let mut on = n.visible_on.contains(p);
                        let name = format!("Node visible on {}", platform_label(*p));
                        check(ui, &name, &mut on, platform_label(*p));
                        if on != n.visible_on.contains(p) {
                            if on {
                                n.visible_on.push(*p);
                                n.visible_on
                                    .sort_by_key(|q| Platform::ALL.iter().position(|x| x == q));
                            } else {
                                n.visible_on.retain(|q| q != p);
                            }
                        }
                    }
                });
            });
            if n.visible_on.is_empty() {
                ui.label(
                    RichText::new("Shown on every target")
                        .size(11.0)
                        .color(c::TEXT_FAINT),
                );
            }
        },
    );
    let focusable = n.kind.is_focusable();
    theme::component(
        ui,
        "ui-node-behavior",
        icon::CURSOR_CLICK,
        c::TEXT,
        "Behavior",
        |ui| {
            theme::prop(ui, "bind", |ui| {
                text(ui, "Node bind", &mut n.bind, "value key")
            });
            if focusable {
                theme::prop(ui, "on_click", |ui| {
                    text(ui, "Node on_click", &mut n.on_click, "action (default: id)")
                });
            }
            theme::prop(ui, "visible_if", |ui| {
                text(ui, "Node visible_if", &mut n.visible_if, "bool value")
            });
            if focusable {
                theme::prop(ui, "enabled_if", |ui| {
                    text(ui, "Node enabled_if", &mut n.enabled_if, "bool value")
                });
                for (dir, v) in [
                    ("up", &mut n.nav.up),
                    ("down", &mut n.nav.down),
                    ("left", &mut n.nav.left),
                    ("right", &mut n.nav.right),
                ] {
                    theme::prop(ui, &format!("nav {dir}"), |ui| {
                        path_combo(
                            ui,
                            &format!("Node nav {dir}"),
                            v,
                            &choices.controls,
                            "by geometry",
                        )
                    });
                }
            }
        },
    );
}

fn kind_settings(ui: &mut egui::Ui, kind: &mut Kind, choices: &Choices) {
    let dirs = labels(Dir::ALL, Dir::label);
    match kind {
        Kind::Stack {
            dir,
            gap,
            align,
            justify,
            wrap,
        } => {
            theme::prop(ui, "dir", |ui| combo(ui, "Stack dir", dir, &dirs));
            theme::prop(ui, "gap", |ui| num(ui, "Stack gap", gap, -512..=512));
            theme::prop(ui, "align", |ui| {
                combo(ui, "Stack align", align, &labels(Align::ALL, Align::label))
            });
            theme::prop(ui, "justify", |ui| {
                combo(
                    ui,
                    "Stack justify",
                    justify,
                    &labels(Justify::ALL, Justify::label),
                )
            });
            theme::prop(ui, "wrap", |ui| {
                check(ui, "Stack wrap", wrap, "next line when full")
            });
        }
        Kind::Grid { cols, gap } => {
            theme::prop(ui, "cols", |ui| num(ui, "Grid cols", cols, 1..=32));
            theme::prop(ui, "gap", |ui| {
                num(ui, "Grid gap x", &mut gap.0, -512..=512);
                num(ui, "Grid gap y", &mut gap.1, -512..=512);
            });
        }
        Kind::Scroll { dir, gap } => {
            theme::prop(ui, "dir", |ui| combo(ui, "Scroll dir", dir, &dirs));
            theme::prop(ui, "gap", |ui| num(ui, "Scroll gap", gap, -512..=512));
        }
        Kind::List { dir, gap, max } => {
            theme::prop(ui, "dir", |ui| combo(ui, "List dir", dir, &dirs));
            theme::prop(ui, "gap", |ui| num(ui, "List gap", gap, -512..=512));
            theme::prop(ui, "max", |ui| num(ui, "List max", max, 1..=256));
        }
        Kind::Image {
            sprite,
            nine,
            fit,
            src,
        } => {
            theme::prop(ui, "sprite", |ui| {
                path_combo(ui, "Image sprite", sprite, &choices.sprites, "—")
            });
            theme::prop(ui, "fit", |ui| {
                combo(
                    ui,
                    "Image fit",
                    fit,
                    &labels(ImageFit::ALL, ImageFit::label),
                )
            });
            theme::prop(ui, "nine", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, side) in ["left", "top", "right", "bottom"].iter().enumerate() {
                        num(ui, &format!("Image nine {side}"), &mut nine[i], 0..=255);
                    }
                });
            });
            theme::prop(ui, "src", |ui| {
                let mut on = src.is_some();
                check(ui, "Image src set", &mut on, "");
                if on != src.is_some() {
                    *src = on.then_some((16, 16));
                }
                match src {
                    Some((w, h)) => {
                        num(ui, "Image src width", w, 1..=4096);
                        num(ui, "Image src height", h, 1..=4096);
                    }
                    None => {
                        ui.label(
                            RichText::new("from the image")
                                .size(11.5)
                                .color(c::TEXT_FAINT),
                        );
                    }
                }
            });
        }
        Kind::Rect {
            border,
            border_color,
        } => {
            theme::prop(ui, "border", |ui| num(ui, "Rect border", border, 0..=32));
            theme::prop(ui, "border color", |ui| {
                color(
                    ui,
                    "Rect border color",
                    border_color,
                    trino_core::ui::Theme::DEFAULT.accent,
                )
            });
        }
        Kind::Slider { min, max, step } => {
            theme::prop(ui, "min", |ui| {
                num(ui, "Slider min", min, -100_000..=100_000)
            });
            theme::prop(ui, "max", |ui| {
                num(ui, "Slider max", max, -100_000..=100_000)
            });
            theme::prop(ui, "step", |ui| num(ui, "Slider step", step, 1..=10_000));
        }
        Kind::Model3d {
            model,
            yaw,
            pitch,
            spin,
            extent,
            lift,
        } => {
            theme::prop(ui, "model", |ui| {
                path_combo(ui, "Model3d model", model, &choices.models, "—")
            });
            theme::prop(ui, "yaw", |ui| num(ui, "Model3d yaw", yaw, -360..=360));
            theme::prop(ui, "pitch", |ui| num(ui, "Model3d pitch", pitch, -90..=90));
            theme::prop(ui, "spin", |ui| num(ui, "Model3d spin", spin, -720..=720));
            theme::prop(ui, "extent", |ui| {
                num(ui, "Model3d extent", extent, 1..=10_000)
            });
            theme::prop(ui, "lift", |ui| {
                num(ui, "Model3d lift", lift, -10_000..=10_000)
            });
        }
        Kind::Overlay | Kind::Text | Kind::Button | Kind::Toggle => {}
    }
}

fn screen_sections(ui: &mut egui::Ui, d: &mut ScreenDef, choices: &Choices) {
    theme::component(
        ui,
        "ui-screen",
        icon::LAYOUT,
        c::ACCENT_TEXT,
        "Screen",
        |ui| {
            theme::prop(ui, "name", |ui| {
                text(ui, "Screen name", &mut d.name, "the file's stem")
            });
            theme::prop(ui, "reference", |ui| {
                num(ui, "Screen reference width", &mut d.reference.0, 16..=4096);
                num(ui, "Screen reference height", &mut d.reference.1, 16..=4096);
            });
            theme::prop(ui, "scaling", |ui| {
                combo(
                    ui,
                    "Screen scaling",
                    &mut d.scaling,
                    &labels(Scaling::ALL, Scaling::label),
                )
            });
            theme::prop(ui, "wrap", |ui| {
                check(ui, "Screen wrap", &mut d.wrap, "focus wraps around")
            });
            theme::prop(ui, "focus", |ui| {
                path_combo(
                    ui,
                    "Screen focus",
                    &mut d.focus,
                    &choices.controls,
                    "first control",
                )
            });
            theme::prop(ui, "on_back", |ui| {
                text(
                    ui,
                    "Screen on_back",
                    &mut d.on_back,
                    "action B / Escape sends",
                )
            });
        },
    );
    let t = trino_core::ui::Theme::DEFAULT;
    theme::component_open(
        ui,
        "ui-theme",
        icon::PALETTE,
        c::TEXT,
        "Theme",
        false,
        |ui| {
            let th = &mut d.theme;
            theme::prop(ui, "font", |ui| {
                path_combo(
                    ui,
                    "Theme font",
                    &mut th.font,
                    &choices.fonts,
                    "fonts/trino5x7",
                )
            });
            theme::prop(ui, "scale", |ui| {
                num(ui, "Theme text scale", &mut th.text_scale, 0..=8)
            });
            for (label, v, default) in [
                ("text", &mut th.text, t.text),
                ("dim", &mut th.dim, t.dim),
                ("panel", &mut th.panel, t.panel),
                ("button", &mut th.button, t.button),
                ("button text", &mut th.button_text, t.button_text),
                ("focus", &mut th.focus, t.focus),
                ("focus text", &mut th.focus_text, t.focus_text),
                ("accent", &mut th.accent, t.accent),
                ("pressed", &mut th.pressed, t.pressed),
            ] {
                theme::prop(ui, label, |ui| {
                    color(ui, &format!("Theme {label} color"), v, default)
                });
            }
        },
    );
    if !d.preview.is_empty() {
        theme::component_open(
            ui,
            "ui-preview-values",
            icon::SLIDERS_HORIZONTAL,
            c::TEXT,
            "Preview values",
            false,
            |ui| {
                for (key, v) in d.preview.iter_mut() {
                    let name = format!("Preview {key}");
                    theme::prop(ui, key, |ui| match v {
                        Sample::Int(i) => num(ui, &name, i, -1_000_000..=1_000_000),
                        Sample::Bool(b) => check(ui, &name, b, ""),
                        Sample::Text(t) => text(ui, &name, t, ""),
                        Sample::Items(items) => {
                            ui.label(
                                RichText::new(format!("{} items", items.len()))
                                    .color(c::TEXT_FAINT),
                            );
                        }
                    });
                }
            },
        );
    }
}
