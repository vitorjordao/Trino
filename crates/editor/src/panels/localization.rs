//! The Localization tab (issue #59): every key × every language of
//! `assets/i18n/`, edited in place and written back as the RON files
//! `cargo xtask i18n` reads; missing cells (amber: a fallback shows; red:
//! nothing does) and cells with an `i18n check` issue are marked; the
//! preview language the UI tab and Play use; per font, the glyphs each
//! language lacks and the N64 TMEM its atlas takes.

use eframe::egui::{self, Color32, RichText};
use trino_core::i18n::PluralForm;
use trino_scene::i18n::Severity;

use crate::app::App;
use crate::localization::{CSV_PATH, Cell, Row};
use crate::theme::{self, c, icon};

/// The narrowest a language column gets before the table pages sideways.
const MIN_COLUMN: f32 = 96.0;
/// Room kept for the vertical scroll bar.
const SCROLL_BAR: f32 = 14.0;

fn named(r: &egui::Response, typ: egui::WidgetType, name: &str) {
    r.widget_info(|| egui::WidgetInfo::labeled(typ, true, name));
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let root = app.project.root.clone();
    let generation = app.project.generation;
    let loc = &mut app.localization;
    let preview_before = loc.preview.clone();
    loc.refresh_issues();
    loc.poll_fonts(&root, generation);
    if loc.fonts_measuring() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    toolbar(app, ui);
    let loc = &mut app.localization;
    if let Some(e) = &loc.error {
        ui.label(RichText::new(format!("{} {e}", icon::X_CIRCLE)).color(c::ERROR));
    }
    ui.separator();
    if loc.langs.tables.is_empty() {
        theme::empty_state(
            ui,
            icon::TRANSLATE,
            "No string tables",
            "Add a language: it becomes assets/i18n/<tag>.ron",
        );
    } else {
        egui::ScrollArea::vertical()
            .id_salt("loc-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                issues(app, ui);
                fonts(app, ui);
                ui.add_space(4.0);
                table(app, ui);
            });
    }
    // The UI tab previews the same language.
    if app.localization.preview != preview_before {
        app.ui_screens.lang = app.localization.preview.clone();
    }
}

// ------------------------------------------------------------- toolbar --

fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    let root = app.project.root.clone();
    let mut csv_out: Option<String> = None;
    let mut csv_in = false;
    ui.horizontal_wrapped(|ui| {
        let loc = &mut app.localization;
        ui.label(RichText::new(icon::TRANSLATE).color(c::ACCENT_TEXT));
        let w = 132.0f32.min(ui.available_width() - 8.0);
        theme::fixed_width(ui, w, |ui| {
            let names = loc.columns();
            let r = egui::ComboBox::from_id_salt("loc-preview")
                .selected_text(loc.preview.as_deref().unwrap_or("Default text"))
                .width(w)
                .truncate()
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(loc.preview.is_none(), "Default text")
                        .clicked()
                    {
                        loc.preview = None;
                    }
                    for l in names {
                        let on = loc.preview.as_deref() == Some(l.as_str());
                        if ui.selectable_label(on, &l).clicked() {
                            loc.preview = Some(l);
                        }
                    }
                })
                .response
                .on_hover_text(
                    "The language the editor previews: the UI tab's texts and Play's \
                     TRINO_LANG",
                );
            named(&r, egui::WidgetType::ComboBox, "Preview language");
        });
        let w = 150.0f32.min(ui.available_width() - 8.0);
        theme::fixed_width(ui, w, |ui| {
            let r = theme::search_box(ui, &mut loc.filter, "Filter keys");
            named(&r, egui::WidgetType::TextEdit, "Filter keys");
        });
        if theme::named_icon_button(ui, icon::PLUS, "Add key").clicked() {
            loc.new_key = Some(String::new());
        }
        let r = ui.add_enabled_ui(loc.selected.is_some(), |ui| {
            theme::named_icon_button(ui, icon::TRASH, "Remove key")
        });
        if r.inner.clicked()
            && let Some(k) = loc.selected.clone()
        {
            loc.remove_key(&k);
        }
        if theme::named_icon_button(ui, icon::GLOBE, "Add language").clicked() {
            loc.new_lang = Some(String::new());
        }
        if theme::named_icon_button(ui, icon::MAGIC_WAND, "Make pseudo-locale").clicked()
            && let Err(e) = loc.make_pseudo()
        {
            loc.error = Some(e);
        }
        if theme::named_icon_button(ui, icon::DOWNLOAD_SIMPLE, "Export CSV").clicked() {
            csv_out = Some(loc.csv());
        }
        if theme::named_icon_button(ui, icon::UPLOAD_SIMPLE, "Import CSV").clicked() {
            csv_in = true;
        }
        let errors = loc.errors();
        let warnings = loc
            .issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
            .count();
        let (text, fg) = match (errors, warnings) {
            (0, 0) => (format!("{} clean", icon::CHECK_CIRCLE), c::OK),
            (0, w) => (format!("{} {w} warnings", icon::WARNING), c::WARN),
            (e, _) => (format!("{} {e} errors", icon::X_CIRCLE), c::ERROR),
        };
        theme::pill(ui, &text, fg, c::BG2);
    });
    let loc = &mut app.localization;
    if let Some(name) = &mut loc.new_key {
        let mut create = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 150.0).clamp(80.0, 240.0);
            let r = ui.add(
                egui::TextEdit::singleline(name)
                    .hint_text("menu.play")
                    .desired_width(w),
            );
            named(&r, egui::WidgetType::TextEdit, "New key name");
            create = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            create |= ui.button("Create key").clicked();
            cancel = ui.button("Cancel").clicked();
        });
        if create {
            let n = name.clone();
            match loc.add_key(&n) {
                Ok(()) => loc.new_key = None,
                Err(e) => loc.error = Some(e),
            }
        } else if cancel {
            loc.new_key = None;
        }
    }
    if let Some(tag) = &mut loc.new_lang {
        let mut create = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 170.0).clamp(80.0, 160.0);
            let r = ui.add(
                egui::TextEdit::singleline(tag)
                    .hint_text("pt-BR")
                    .desired_width(w),
            );
            named(&r, egui::WidgetType::TextEdit, "New language tag");
            create = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            create |= ui.button("Create language").clicked();
            cancel = ui.button("Cancel").clicked();
        });
        if create {
            let t = tag.clone();
            match loc.add_language(&t) {
                Ok(_) => {
                    loc.new_lang = None;
                    loc.error = None;
                }
                Err(e) => loc.error = Some(e),
            }
        } else if cancel {
            loc.new_lang = None;
        }
    }
    if let Some(text) = csv_out {
        let path = root.join(CSV_PATH);
        match app.write_echo(path.clone(), text) {
            Ok(()) => app.log.toast(
                crate::log::Level::Ok,
                format!("{} {} written", icon::FILE_CSV, CSV_PATH),
            ),
            Err(e) => app.log.toast(crate::log::Level::Error, e),
        }
    }
    if csv_in {
        let path = root.join(CSV_PATH);
        let r = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {CSV_PATH}: {e} (Export CSV first)"))
            .and_then(|t| app.localization.import_csv(&t));
        match r {
            Ok(m) => app
                .log
                .toast(crate::log::Level::Ok, format!("{} {m}", icon::FILE_CSV)),
            Err(e) => app.log.toast(crate::log::Level::Error, e),
        }
    }
}

// -------------------------------------------------------------- issues --

fn issues(app: &mut App, ui: &mut egui::Ui) {
    let loc = &mut app.localization;
    if loc.issues.is_empty() {
        return;
    }
    let n = loc.issues.len();
    let mut pick = None;
    egui::CollapsingHeader::new(format!("{}  Issues ({n})", icon::WARNING))
        .id_salt("loc-issues")
        .default_open(true)
        .show(ui, |ui| {
            const SHOWN: usize = 8;
            for i in loc.issues.iter().take(SHOWN) {
                let color = match i.severity {
                    Severity::Error => c::ERROR,
                    Severity::Warning => c::WARN,
                    Severity::Info => c::TEXT_WEAK,
                };
                let text = format!("[{}] {}: {}", i.lang, i.key, i.message);
                let r = ui.add(
                    egui::Button::new(RichText::new(&text).color(color).size(12.0))
                        .frame(false)
                        .truncate(),
                );
                if r.clicked() && !i.key.is_empty() {
                    pick = Some(i.key.clone());
                }
                r.on_hover_text(&text);
            }
            if n > SHOWN {
                ui.label(
                    RichText::new(format!("… {} more (cargo xtask i18n check)", n - SHOWN))
                        .color(c::TEXT_FAINT),
                );
            }
        });
    if let Some(k) = pick {
        if !k.to_lowercase().contains(&loc.filter.trim().to_lowercase()) {
            loc.filter.clear();
        }
        loc.selected = Some(k);
        loc.scroll_to_selected = true;
    }
}

// --------------------------------------------------------------- fonts --

fn fonts(app: &mut App, ui: &mut egui::Ui) {
    let loc = &app.localization;
    let title = if loc.fonts_measuring() {
        format!("{}  Fonts (measuring…)", icon::TEXT_AA)
    } else {
        format!("{}  Fonts", icon::TEXT_AA)
    };
    egui::CollapsingHeader::new(title)
        .id_salt("loc-fonts")
        .default_open(true)
        .show(ui, |ui| {
            if let Some(e) = &loc.fonts_error {
                ui.label(RichText::new(e).color(c::ERROR));
            }
            if loc.fonts.is_empty() && !loc.fonts_measuring() {
                ui.label(
                    RichText::new("No font draws localized text yet ([i18n] check_fonts)")
                        .color(c::TEXT_FAINT),
                );
            }
            for f in &loc.fonts {
                let full = f.covered == f.used;
                let (glyph, color) = if full {
                    (icon::CHECK_CIRCLE, c::OK)
                } else {
                    (icon::X_CIRCLE, c::ERROR)
                };
                let via = if f.draws_with == f.font {
                    String::new()
                } else {
                    format!(" as {}", f.draws_with)
                };
                let tmem = match f.n64_bytes {
                    Some(b) => format!(
                        " · N64 {:.1} of {} KB TMEM{}",
                        b as f64 / 1024.0,
                        f.tmem / 1024,
                        if b > f.tmem { " (over)" } else { "" }
                    ),
                    None => String::new(),
                };
                let text = format!(
                    "{glyph} {} {}{via}: {}/{} glyphs{tmem}",
                    f.lang, f.font, f.covered, f.used
                );
                let over = f.n64_bytes.is_some_and(|b| b > f.tmem);
                let color = if !full {
                    color
                } else if over {
                    c::WARN
                } else {
                    c::TEXT_WEAK
                };
                let mut tip = text.clone();
                if !full {
                    tip.push_str(&format!("\nlacks: {}", f.missing));
                }
                ui.add(egui::Label::new(RichText::new(&text).color(color).size(12.0)).truncate())
                    .on_hover_text(tip);
            }
        });
}

// --------------------------------------------------------------- table --

fn cell_fill(cell: &Cell, issue: bool) -> Color32 {
    match (cell, issue) {
        (_, true) => theme::mix(c::BG0, c::ERROR, 0.22),
        (Cell::Own, false) => c::BG0,
        (Cell::Fallback(_), false) => theme::mix(c::BG0, c::WARN, 0.16),
        (Cell::Missing, false) => theme::mix(c::BG0, c::ERROR, 0.28),
    }
}

fn table(app: &mut App, ui: &mut egui::Ui) {
    let loc = &mut app.localization;
    let columns = loc.columns();
    let rows = loc.rows();
    let gap = ui.spacing().item_spacing.x;
    let avail = (ui.available_width() - SCROLL_BAR).max(120.0);
    let key_w = (avail * 0.28).clamp(80.0, 180.0).floor();
    let fit = (((avail - key_w) / (MIN_COLUMN + gap)).floor() as usize).max(1);
    let shown = columns.len().min(fit);
    if loc.first_column + shown > columns.len() {
        loc.first_column = columns.len() - shown;
    }
    if shown < columns.len() {
        ui.horizontal(|ui| {
            let first = loc.first_column;
            if ui
                .add_enabled(first > 0, egui::Button::new(icon::CARET_LEFT))
                .on_hover_text("Previous languages")
                .clicked()
            {
                loc.first_column -= 1;
            }
            ui.label(
                RichText::new(format!(
                    "languages {}–{} of {}",
                    first + 1,
                    first + shown,
                    columns.len()
                ))
                .color(c::TEXT_WEAK),
            );
            if ui
                .add_enabled(
                    first + shown < columns.len(),
                    egui::Button::new(icon::CARET_RIGHT),
                )
                .on_hover_text("Next languages")
                .clicked()
            {
                loc.first_column += 1;
            }
        });
    }
    let langs: Vec<String> = columns[loc.first_column..loc.first_column + shown].to_vec();
    let col_w = ((avail - key_w) / shown as f32 - gap).floor().max(40.0);
    let default = loc.default_lang();
    egui::Grid::new("loc-table")
        .num_columns(1 + shown)
        .striped(true)
        .spacing([gap, 3.0])
        .show(ui, |ui| {
            ui.add_sized(
                [key_w, 20.0],
                egui::Label::new(RichText::new("Key").color(c::TEXT_WEAK)).truncate(),
            );
            for l in &langs {
                let name = loc.langs.tables.get(l).map_or("", |t| t.name.as_str());
                let mut head = l.clone();
                if Some(l) == default.as_ref() {
                    head.push_str(" (default)");
                }
                let tip = if name.is_empty() {
                    head.clone()
                } else {
                    format!("{head}: {name}")
                };
                ui.add_sized(
                    [col_w, 20.0],
                    egui::Label::new(RichText::new(head).family(theme::bold())).truncate(),
                )
                .on_hover_text(tip);
            }
            ui.end_row();
            let mut last_key = String::new();
            for row in &rows {
                let first = row.key != last_key;
                last_key = row.key.clone();
                let selected = loc.selected.as_deref() == Some(row.key.as_str());
                let label = match (row.form, first) {
                    (Some(f), true) => format!("{} · {}", row.key, f.key()),
                    (Some(f), false) => format!("    · {}", f.key()),
                    (None, _) => row.key.clone(),
                };
                let color = if selected { c::ACCENT_TEXT } else { c::TEXT };
                let r = ui.add_sized(
                    [key_w, 22.0],
                    egui::Button::new(RichText::new(&label).color(color).size(12.5))
                        .frame(selected)
                        .truncate(),
                );
                let r = r.on_hover_text(&row.key);
                if r.clicked() {
                    loc.selected = Some(row.key.clone());
                }
                if selected && first && loc.scroll_to_selected {
                    r.scroll_to_me(Some(egui::Align::Center));
                    loc.scroll_to_selected = false;
                }
                for l in &langs {
                    cell(loc, ui, l, row, col_w);
                }
                ui.end_row();
            }
        });
}

fn cell(
    loc: &mut crate::localization::Localization,
    ui: &mut egui::Ui,
    lang: &str,
    row: &Row,
    w: f32,
) {
    let state = loc.cell(lang, &row.key);
    let issues: Vec<String> = loc
        .cell_issues(lang, &row.key)
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.message.clone())
        .collect();
    let mut text = loc.text(lang, row);
    let hint = match &state {
        Cell::Own => String::new(),
        _ => loc.shown(lang, row).unwrap_or_default(),
    };
    let r = ui.add_sized(
        [w, 22.0],
        egui::TextEdit::singleline(&mut text)
            .hint_text(RichText::new(hint).color(c::TEXT_FAINT).italics())
            .background_color(cell_fill(&state, !issues.is_empty())),
    );
    let form = row
        .form
        .map_or(String::new(), |f: PluralForm| format!(" · {}", f.key()));
    named(
        &r,
        egui::WidgetType::TextEdit,
        &format!("{lang} {}{form}", row.key),
    );
    let mut tip = match &state {
        Cell::Own => String::new(),
        Cell::Fallback(from) => format!("missing: shows {from}'s text"),
        Cell::Missing => "missing, and no fallback has it: shows nothing".into(),
    };
    for i in &issues {
        if !tip.is_empty() {
            tip.push('\n');
        }
        tip.push_str(i);
    }
    let r = if tip.is_empty() {
        r
    } else {
        r.on_hover_text(tip)
    };
    if r.gained_focus() {
        loc.selected = Some(row.key.clone());
    }
    if r.changed() {
        loc.set_text(lang, row, &text);
    }
}
