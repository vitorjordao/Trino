//! The Accessibility tab (issue #60, ADR 0026):
//!
//! - **Simulation:** the Scene, Game and UI views show the picture as a
//!   viewer with protanopia, deuteranopia, tritanopia or no color vision
//!   sees it (the renderer's simulation view), and the UI tab lays its
//!   screen out at the player's text size and in high contrast. Also in
//!   View › Accessibility simulation and `TRINO_EDITOR_ACCESS`.
//! - **Semantic colors:** `assets/palettes.ron` — each color, what each
//!   mode shows instead (click a swatch to change it, × to drop it) and,
//!   under it, how that mode's viewer sees it. Written on every edit, live
//!   from disk.
//! - **Checklist:** what `cargo xtask access check` reports, from the same
//!   code: groups apart, pairs readable, the theme, captions in every
//!   language, every screen at 200 % text.

use eframe::egui::{self, Color32, RichText};
use trino_core::{AccessSettings, Color, CvdMode};
use trino_scene::access::SemanticColor;
use trino_scene::ui::Level;

use crate::app::App;
use crate::theme::{self, c, icon};

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            header(app, ui);
            ui.add_space(4.0);
            simulation(ui, &mut app.access.preview);
            ui.add_space(6.0);
            colors(app, ui);
            ui.add_space(6.0);
            checklist(app, ui);
        });
}

fn header(app: &App, ui: &mut egui::Ui) {
    let a = &app.access;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{}  Accessibility", icon::EYE))
                .family(theme::bold())
                .color(c::TEXT),
        );
        let (text, color) = if let Some(e) = &a.error {
            (e.clone(), c::ERROR)
        } else if a.exists {
            ("assets/palettes.ron".to_string(), c::TEXT_FAINT)
        } else {
            (
                "no assets/palettes.ron yet: adding a color creates it".to_string(),
                c::TEXT_FAINT,
            )
        };
        ui.add(egui::Label::new(RichText::new(text).size(11.5).color(color)).truncate())
            .on_hover_text(
                "Semantic colors with an alternative per color-vision mode. Games swap them \
                 per draw (AccessData::color / palette). cargo xtask access check runs the \
                 same checks.",
            );
    });
}

/// The views' simulation: color vision, the UI's text size, high contrast.
pub fn simulation(ui: &mut egui::Ui, s: &mut AccessSettings) -> bool {
    let before = *s;
    theme::section_title(ui, icon::EYE, "Simulation (Scene, Game and UI views)");
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Color vision").size(12.0).color(c::TEXT_WEAK));
        for m in CvdMode::ALL {
            ui.selectable_value(&mut s.cvd, m, m.label());
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("UI text").size(12.0).color(c::TEXT_WEAK));
        ui.add(
            egui::Slider::new(&mut s.text_scale, 100..=200)
                .step_by(25.0)
                .suffix(" %"),
        );
        ui.checkbox(&mut s.high_contrast, "High contrast");
    });
    *s != before
}

/// View › Accessibility simulation: the same as the tab's first section.
pub fn simulation_menu(ui: &mut egui::Ui, s: &mut AccessSettings) -> bool {
    let before = *s;
    for m in CvdMode::ALL {
        let label = match m {
            CvdMode::Off => "Color vision: as drawn".to_string(),
            m => format!("Color vision: {}", m.label().to_lowercase()),
        };
        if ui.radio_value(&mut s.cvd, m, label).clicked() {
            ui.close();
        }
    }
    ui.separator();
    let mut big = s.text_scale >= 200;
    if ui.checkbox(&mut big, "UI text at 200 %").changed() {
        s.text_scale = if big { 200 } else { 100 };
    }
    ui.checkbox(&mut s.high_contrast, "UI high contrast");
    *s != before
}

fn color32(c: Color) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

fn swatch(ui: &mut egui::Ui, color: Color, size: egui::Vec2) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color32(color));
    ui.painter().rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, c::BORDER),
        egui::StrokeKind::Inside,
    );
    resp
}

fn colors(app: &mut App, ui: &mut egui::Ui) {
    theme::section_title(ui, icon::PALETTE, "Semantic colors");
    let mut changed = false;
    let mut remove = None;
    let names: Vec<String> = app.access.palettes.colors.keys().cloned().collect();
    if names.is_empty() {
        ui.label(
            RichText::new(
                "None yet. A game asks for a color by name and gets the player's mode's.",
            )
            .size(11.5)
            .color(c::TEXT_FAINT),
        );
    }
    egui::Grid::new("access-colors")
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            if !names.is_empty() {
                ui.label(RichText::new("color").size(11.0).color(c::TEXT_FAINT));
                for m in CvdMode::ALL {
                    ui.label(RichText::new(m.label()).size(11.0).color(c::TEXT_FAINT));
                }
                ui.end_row();
            }
            for name in &names {
                let Some(col) = app.access.palettes.colors.get_mut(name) else {
                    continue;
                };
                ui.horizontal(|ui| {
                    if ui
                        .small_button(icon::TRASH)
                        .on_hover_text("Remove this color")
                        .clicked()
                    {
                        remove = Some(name.clone());
                    }
                    ui.label(RichText::new(name).color(c::TEXT)).on_hover_text(
                        if col.note.is_empty() {
                            name.as_str()
                        } else {
                            col.note.as_str()
                        },
                    );
                });
                for m in CvdMode::ALL {
                    changed |= mode_cell(ui, col, m);
                }
                ui.end_row();
            }
        });
    if let Some(n) = remove {
        app.access.palettes.colors.remove(&n);
        for g in &mut app.access.palettes.distinct {
            g.retain(|x| x != &n);
        }
        changed = true;
    }
    ui.horizontal(|ui| {
        let id = ui.id().with("access-new-color");
        let mut name: String = ui.data_mut(|d| d.get_temp(id).unwrap_or_default());
        ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(150.0)
                .hint_text("team.home"),
        );
        let valid = !name.is_empty()
            && !name.contains(char::is_whitespace)
            && !app.access.palettes.colors.contains_key(&name);
        if ui
            .add_enabled(
                valid,
                egui::Button::new(format!("{}  Add color", icon::PLUS)),
            )
            .clicked()
        {
            app.access
                .palettes
                .colors
                .insert(name.clone(), SemanticColor::new([200, 200, 200]));
            name.clear();
            changed = true;
        }
        ui.data_mut(|d| d.insert_temp(id, name));
    });
    if changed {
        app.access.edited();
    }
}

/// One mode's cell: the color the game draws in that mode (an edit sets
/// the mode's alternative, `Off` the base), and how that viewer sees it.
fn mode_cell(ui: &mut egui::Ui, col: &mut SemanticColor, m: CvdMode) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let shown = col.get(m);
        let mut rgb = [shown.r, shown.g, shown.b];
        let resp = ui.color_edit_button_srgb(&mut rgb).on_hover_text(match m {
            CvdMode::Off => "The base color".to_string(),
            m if col.alternative(m).is_some() => format!("The {} alternative", m.key()),
            m => format!("No {} alternative: the base shows", m.key()),
        });
        if resp.changed() {
            col.set_alternative(m, Some(rgb));
            changed = true;
        }
        if m != CvdMode::Off {
            swatch(
                ui,
                trino_core::access::simulate(m, shown),
                egui::vec2(10.0, 18.0),
            )
            .on_hover_text(format!("As a viewer with {} sees it", m.key()));
            if col.alternative(m).is_some()
                && ui
                    .small_button("×")
                    .on_hover_text("Drop the alternative")
                    .clicked()
            {
                col.set_alternative(m, None);
                changed = true;
            }
        }
    });
    changed
}

fn checklist(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        theme::section_title(ui, icon::LIST_CHECKS, "Checklist");
        if ui
            .small_button(format!("{}  Run again", icon::ARROWS_CLOCKWISE))
            .on_hover_text("cargo xtask access check")
            .clicked()
        {
            app.access.stale();
        }
    });
    let root = app.project.root.clone();
    app.access.refresh_report(&root);
    match &app.access.report {
        None => {}
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(c::ERROR).size(12.0));
        }
        Some(Ok(r)) => {
            ui.label(
                RichText::new(format!(
                    "{} colors, {} captions ({} important), {} icons, {} screens at 200 % text: \
                     {} errors, {} warnings",
                    r.colors,
                    r.captions,
                    r.important,
                    r.icons,
                    r.screens,
                    r.errors(),
                    r.warnings()
                ))
                .size(11.5)
                .color(if r.findings.is_empty() {
                    c::OK
                } else {
                    c::WARN
                }),
            );
            for line in &r.checklist {
                let (glyph, color) = if line.passed {
                    (icon::CHECK_CIRCLE, c::OK)
                } else {
                    (icon::X_CIRCLE, c::ERROR)
                };
                ui.label(
                    RichText::new(format!("{glyph}  [{}] {}", line.area, line.check))
                        .size(12.0)
                        .color(color),
                );
            }
            for f in &r.findings {
                let color = match f.level {
                    Level::Error => c::ERROR,
                    Level::Warning => c::WARN,
                };
                ui.label(RichText::new(f.to_string()).size(11.5).color(color));
            }
        }
    }
}
