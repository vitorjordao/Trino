//! Assets › Sprite › Texture formats: what each platform bakes a sprite or
//! font atlas to (issue #52). One column per platform: the manifest's
//! format (`auto`, a hardware format, or the default), what the bake uses
//! and why, its bytes, its PSNR against the master (colored against the
//! threshold) and its share of the platform's texture memory; the
//! texture's `quality` and `min_psnr`; and the master beside what the
//! platform samples, zoomed with nearest filtering over a checkerboard.
//! Edits write the manifest (the watcher rebakes); `cargo xtask texture
//! inspect` prints the same decisions.

use eframe::egui::{self, Color32, RichText, Vec2};
use trino_asset_pipeline::Platform;
use trino_asset_pipeline::texture::{self, Quality, TextureReport};

use crate::app::{App, Cmd};
use crate::textures::{Column, bytes};
use crate::theme::{self, c, icon};

/// Shown in a format combo for "no `formats` entry".
const DEFAULT: &str = "default";

fn label(p: Platform) -> &'static str {
    match p {
        Platform::Pc => "PC",
        Platform::N64 => "N64",
        Platform::N3ds => "3DS",
        Platform::Web => "Web",
    }
}

/// The section, under the sprite's thumbnail.
pub fn section(app: &mut App, ui: &mut egui::Ui, logical: &str) {
    let key = crate::textures::key(&app.project.manifest_path(), app.project.generation);
    let assets = app.project.assets_dir();
    let (insp, errors) = app.textures.inspect(ui.ctx(), &assets, logical, key);
    let ready = insp.is_some();
    // A bake error once in the Console (and a toast), always in the panel.
    for e in errors {
        app.log.toast(crate::log::Level::Error, e);
    }
    let measuring = app.textures.measuring();
    let crate::textures::Textures {
        current,
        preview: platform,
        zoom,
        min_psnr_edit,
        ..
    } = &mut app.textures;
    let Some((_, insp, shown)) = current.as_ref().filter(|_| ready) else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Measuring formats…").color(c::TEXT_WEAK));
        });
        return;
    };
    let mut cmds = Vec::new();
    // `TRINO_EDITOR_INSPECT=texture`: scroll the formats to the top.
    if app.inspect_focus.as_deref() == Some("texture") {
        ui.scroll_to_rect(
            egui::Rect::from_min_size(ui.cursor().min, Vec2::new(1.0, 1.0)),
            Some(egui::Align::TOP),
        );
    }
    theme::component(
        ui,
        "texture-formats",
        icon::STACK,
        c::SPRITE,
        "Texture formats",
        |ui| {
            if measuring {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Measuring…").size(11.5).color(c::TEXT_FAINT));
                });
            }
            settings(ui, logical, insp, min_psnr_edit, &mut cmds);
            ui.add_space(4.0);
            grid(ui, logical, insp, &mut cmds);
            ui.add_space(4.0);
            reasons(ui, insp);
        },
    );
    theme::component(
        ui,
        "texture-preview",
        icon::MAGNIFYING_GLASS,
        c::SPRITE,
        "Master vs platform",
        |ui| preview(ui, insp, shown, platform, zoom),
    );
    app.cmds.extend(cmds);
}

/// `quality` and `min_psnr`: the threshold `auto` keeps (and a declared
/// format is checked against with `min_psnr`).
fn settings(
    ui: &mut egui::Ui,
    logical: &str,
    insp: &crate::textures::Inspection,
    edit: &mut Option<f64>,
    cmds: &mut Vec<Cmd>,
) {
    theme::prop(ui, "Quality", |ui| {
        let shown = match insp.quality.as_deref() {
            Some(q) => q.to_string(),
            None => format!("{DEFAULT} (normal)"),
        };
        let mut pick: Option<Option<&str>> = None;
        let r = egui::ComboBox::from_id_salt("texture-quality")
            .selected_text(shown)
            .width(ui.available_width().min(150.0))
            .truncate()
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(insp.quality.is_none(), format!("{DEFAULT} (normal)"))
                    .clicked()
                {
                    pick = Some(None);
                }
                for q in Quality::ALL {
                    let text = format!("{} ({} dB)", q.name(), q.min_psnr());
                    if ui
                        .selectable_label(insp.quality.as_deref() == Some(q.name()), text)
                        .clicked()
                    {
                        pick = Some(Some(q.name()));
                    }
                }
            });
        r.response
            .on_hover_text("The PSNR `auto` keeps: low 30, normal 35, high 40 dB")
            .widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Texture quality")
            });
        if let Some(q) = pick
            && q != insp.quality.as_deref()
        {
            cmds.push(Cmd::SetTextureKey(
                logical.into(),
                "quality",
                q.map(String::from),
            ));
        }
    });
    theme::prop(ui, "Min PSNR", |ui| {
        let declared = insp.settings.min_psnr;
        let mut on = edit.or(declared).is_some();
        let r = ui.checkbox(&mut on, "").on_hover_text(
            "Override the quality's threshold; a declared format below it fails the bake",
        );
        if r.changed() {
            *edit = None;
            let v = on.then(|| format!("{}", insp.settings.quality.min_psnr()));
            cmds.push(Cmd::SetTextureKey(logical.into(), "min_psnr", v));
        }
        if let Some(mut v) = edit.or(declared) {
            let r = ui.add(
                egui::DragValue::new(&mut v)
                    .range(10.0..=60.0)
                    .speed(0.1)
                    .max_decimals(1)
                    .suffix(" dB"),
            );
            if r.changed() {
                *edit = Some(v);
            }
            // Written once the user settles (a drag must not rebake per frame).
            if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged()))
                && let Some(v) = *edit
                && Some(v) != declared
            {
                cmds.push(Cmd::SetTextureKey(
                    logical.into(),
                    "min_psnr",
                    Some(format!("{v:.1}")),
                ));
            }
        } else {
            ui.label(
                RichText::new(format!(
                    "from quality ({} dB)",
                    insp.settings.quality.min_psnr()
                ))
                .size(11.5)
                .color(c::TEXT_FAINT),
            );
        }
    });
}

/// One row: a label, then a cell per platform.
fn row(
    ui: &mut egui::Ui,
    title: &str,
    widths: (f32, f32),
    mut cell: impl FnMut(&mut egui::Ui, usize),
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.allocate_ui_with_layout(
            egui::vec2(widths.0, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_width(widths.0);
                ui.add(egui::Label::new(RichText::new(title).color(c::TEXT_WEAK)).truncate());
            },
        );
        for i in 0..3 {
            ui.allocate_ui_with_layout(
                egui::vec2(widths.1, 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_width(widths.1);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    cell(ui, i)
                },
            );
        }
    });
}

fn small(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(color)).truncate())
}

/// The PSNR's color: at or over the threshold green; under it red when a
/// `min_psnr` makes it a bake error, amber when it is only below the
/// quality preset.
fn psnr_color(r: &TextureReport, preset: f64) -> Color32 {
    match r.psnr {
        None => c::TEXT_FAINT,
        Some(p) if p >= r.threshold.unwrap_or(preset) => c::OK,
        Some(_) if r.threshold.is_some() && !r.auto => c::ERROR,
        Some(_) => c::WARN,
    }
}

fn grid(ui: &mut egui::Ui, logical: &str, insp: &crate::textures::Inspection, cmds: &mut Vec<Cmd>) {
    let label_w = 60.0;
    let col_w = ((ui.available_width() - label_w - 3.0 * 4.0) / 3.0).floor();
    let widths = (label_w, col_w);
    let cols = &insp.columns;
    let preset = insp.settings.quality.min_psnr();
    // A narrow panel can't show a format in a third of its width: one
    // full-width combo per platform above the grid instead.
    let narrow = col_w < 90.0;
    if narrow {
        for col in cols {
            theme::prop(ui, label(col.platform), |ui| {
                let w = ui.available_width().min(160.0);
                format_combo(ui, logical, col, w, cmds);
            });
        }
    }
    row(ui, "", widths, |ui, i| {
        ui.label(
            RichText::new(label(cols[i].platform))
                .size(12.5)
                .family(theme::bold())
                .color(c::TEXT),
        );
    });
    if !narrow {
        row(ui, "Format", widths, |ui, i| {
            format_combo(ui, logical, &cols[i], col_w, cmds);
        });
    }
    row(ui, "Uses", widths, |ui, i| match &cols[i].report {
        Ok(r) => {
            let text = if r.auto {
                format!("{} · auto", r.format)
            } else {
                r.format.clone()
            };
            small(ui, text, if r.auto { c::ACCENT_TEXT } else { c::TEXT })
                .on_hover_text(format!("{} — {}", r.content, r.reason));
        }
        Err(e) => {
            small(ui, format!("{} bake fails", icon::WARNING), c::ERROR).on_hover_text(e);
        }
    });
    row(ui, "Size", widths, |ui, i| {
        if let Ok(r) = &cols[i].report {
            small(ui, bytes(r.bytes), c::TEXT_WEAK)
                .on_hover_text(format!("{} bytes of texels (and palette)", r.bytes));
        }
    });
    row(ui, "PSNR", widths, |ui, i| {
        if let Ok(r) = &cols[i].report {
            let thr = r.threshold.unwrap_or(preset);
            small(ui, texture::db(r.psnr), psnr_color(r, preset)).on_hover_text(format!(
                "Against the master; {} {thr} dB{}",
                if r.threshold.is_some() {
                    "must keep"
                } else {
                    "the quality preset is"
                },
                r.note.as_ref().map_or(String::new(), |n| format!("\n{n}"))
            ));
        }
    });
    row(ui, "Memory", widths, |ui, i| {
        if let Ok(r) = &cols[i].report {
            let (text, color, hover) = memory(r);
            small(ui, text, color).on_hover_text(hover);
        }
    });
}

/// The memory cell: the share of TMEM or 3DS texture memory, or the PC's
/// saving against RGBA8.
fn memory(r: &TextureReport) -> (String, Color32, String) {
    match r.share_percent() {
        Some(p) => (
            format!(
                "{} {}",
                if p >= 10.0 {
                    format!("{p:.0}%")
                } else if p >= 1.0 {
                    format!("{p:.1}%")
                } else {
                    format!("{p:.2}%")
                },
                if r.memory == "TMEM" { "TMEM" } else { "mem" }
            ),
            if p > 100.0 {
                c::ERROR
            } else if p > 75.0 {
                c::WARN
            } else {
                c::TEXT_WEAK
            },
            format!(
                "{} of {} {}",
                bytes(r.memory_bytes),
                bytes(r.memory_budget.unwrap_or(0)),
                r.memory
            ),
        ),
        None => {
            let rgba8 = r.width as u64 * r.height as u64 * 4;
            let saved = 100.0 - r.memory_bytes as f64 * 100.0 / rgba8.max(1) as f64;
            (
                if saved > 0.5 {
                    format!("−{saved:.0}% VRAM")
                } else {
                    "VRAM".into()
                },
                if saved > 0.5 { c::OK } else { c::TEXT_WEAK },
                format!(
                    "{} of VRAM ({} as RGBA8)",
                    bytes(r.memory_bytes),
                    bytes(rgba8)
                ),
            )
        }
    }
}

/// A platform's `formats` entry: the default (no entry), `auto`, or one of
/// its hardware formats. Its accessible label is "<platform> format".
fn format_combo(ui: &mut egui::Ui, logical: &str, col: &Column, width: f32, cmds: &mut Vec<Cmd>) {
    let p = col.platform;
    let shown = col.declared.clone().unwrap_or_else(|| DEFAULT.into());
    let mut pick: Option<Option<String>> = None;
    let r = egui::ComboBox::from_id_salt(("texture-format", p.key()))
        .selected_text(shown)
        .width(width)
        .truncate()
        .height(300.0)
        .show_ui(ui, |ui| {
            let default = p.default_texture_format().unwrap_or("none: an error");
            if ui
                .selectable_label(col.declared.is_none(), format!("{DEFAULT} ({default})"))
                .clicked()
            {
                pick = Some(None);
            }
            for f in std::iter::once("auto").chain(texture::formats(p)) {
                if ui
                    .selectable_label(col.declared.as_deref() == Some(f), f)
                    .clicked()
                {
                    pick = Some(Some(f.to_string()));
                }
            }
        });
    let name = format!("{} format", label(p));
    r.response
        .on_hover_text("The manifest's `formats` entry (default removes it)")
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &name));
    if let Some(f) = pick
        && f != col.declared
    {
        cmds.push(Cmd::SetTextureFormat(logical.into(), p, f));
    }
}

/// Why each platform got its format (the full text the cells hover), and
/// each bake error in full.
fn reasons(ui: &mut egui::Ui, insp: &crate::textures::Inspection) {
    for col in &insp.columns {
        let (text, color) = match &col.report {
            Ok(r) => (
                format!(
                    "{}: {}{}",
                    label(col.platform),
                    r.reason,
                    r.note.as_ref().map_or(String::new(), |n| format!(" ({n})"))
                ),
                c::TEXT_FAINT,
            ),
            Err(e) => (format!("{}: {e}", label(col.platform)), c::ERROR),
        };
        ui.add(egui::Label::new(RichText::new(text).size(11.5).color(color)).wrap());
    }
}

/// The master beside what the chosen platform samples, at the same zoom.
fn preview(
    ui: &mut egui::Ui,
    insp: &crate::textures::Inspection,
    shown: &crate::textures::Shown,
    platform: &mut usize,
    zoom: &mut f32,
) {
    ui.horizontal(|ui| {
        for (i, col) in insp.columns.iter().enumerate() {
            if ui
                .selectable_label(*platform == i, label(col.platform))
                .clicked()
            {
                *platform = i;
            }
        }
    });
    theme::prop(ui, "Zoom", |ui| {
        ui.add(
            egui::Slider::new(zoom, 1.0..=8.0)
                .logarithmic(true)
                .max_decimals(1)
                .suffix("×"),
        )
        .on_hover_text("Zoom on the center (nearest filtering)");
    });
    let i = (*platform).min(insp.columns.len() - 1);
    let col = &insp.columns[i];
    if col.width == 0 {
        return;
    }
    let gap = 6.0;
    let side = ((ui.available_width() - gap) / 2.0).floor().max(40.0);
    // Fit the texture's aspect in side × 260.
    let aspect = col.height as f32 / col.width as f32;
    let size = if side * aspect > 260.0 {
        Vec2::new(260.0 / aspect, 260.0)
    } else {
        Vec2::new(side, (side * aspect).max(8.0))
    };
    let title = match &col.report {
        Ok(r) => format!("{} {}", label(col.platform), r.format),
        Err(_) => format!("{} (declared)", label(col.platform)),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.vertical(|ui| {
            small(ui, "Master", c::TEXT_WEAK);
            image(ui, Some(&shown.master[i]), size, *zoom, None);
        });
        ui.vertical(|ui| {
            small(ui, title, c::TEXT_WEAK);
            let err = col.decoded.as_ref().err().map(String::as_str);
            image(ui, shown.decoded[i].as_ref(), size, *zoom, err);
        });
    });
}

/// `tex` over a checkerboard in a box of `size`, its center zoomed, or
/// `note` when there is nothing to show.
fn image(
    ui: &mut egui::Ui,
    tex: Option<&egui::TextureHandle>,
    size: Vec2,
    zoom: f32,
    note: Option<&str>,
) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let cell = 8.0;
    painter.rect_filled(rect, 0.0, Color32::from_gray(58));
    let (nx, ny) = (
        (rect.width() / cell).ceil() as i32,
        (rect.height() / cell).ceil() as i32,
    );
    for y in 0..ny {
        for x in (y % 2..nx).step_by(2) {
            let min = rect.min + Vec2::new(x as f32 * cell, y as f32 * cell);
            painter.rect_filled(
                egui::Rect::from_min_size(min, Vec2::splat(cell)).intersect(rect),
                0.0,
                Color32::from_gray(92),
            );
        }
    }
    match (tex, note) {
        (Some(t), _) => {
            let half = 0.5 / zoom.max(1.0);
            let uv = egui::Rect::from_min_max(
                egui::pos2(0.5 - half, 0.5 - half),
                egui::pos2(0.5 + half, 0.5 + half),
            );
            painter.image(t.id(), rect, uv, Color32::WHITE);
        }
        (None, note) => {
            let galley = painter.layout(
                note.unwrap_or("not modeled").to_string(),
                egui::FontId::proportional(11.0),
                c::TEXT,
                rect.width() - 12.0,
            );
            let at = rect.center() - galley.size() / 2.0;
            painter.rect_filled(
                egui::Rect::from_min_size(at, galley.size()).expand(4.0),
                4.0,
                c::BG0,
            );
            painter.galley(at, galley, c::TEXT);
        }
    }
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, c::BORDER),
        egui::StrokeKind::Inside,
    );
}
