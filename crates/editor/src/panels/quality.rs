//! The Quality tab (issue #55): what Low, Medium, High and Ultra mean for
//! this project on the PC and the Web — `assets/quality.ron`, the text
//! file `cargo xtask quality set` writes just as well (the bake turns it
//! into the `quality/presets` blob the hosts read). One column per preset,
//! one row per setting; a changed preset can go back to the engine's
//! values. Edits are written once the user settles, an edit on disk shows
//! here live, an invalid file is shown, not written. A column's eye
//! previews the preset in the Game view (its PC or Web profile).

use eframe::egui::{self, RichText};
use trino_core::{
    AoSetting, DynRes, FrameLimit, PostSetting, QualityPreset, QualitySettings, ShadowSetting,
    TexFilter,
};

use crate::app::App;
use crate::theme::{self, c, icon};

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            header(app, ui);
            ui.add_space(4.0);
            table(app, ui);
        });
}

fn header(app: &App, ui: &mut egui::Ui) {
    let q = &app.quality;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{}  Quality presets", icon::SPEEDOMETER))
                .family(theme::bold())
                .color(c::TEXT),
        );
        let file = q.project.presets.display().to_string();
        let (text, color) = if q.dirty {
            ("unsaved".to_string(), c::WARN)
        } else if q.exists {
            (file, c::TEXT_FAINT)
        } else {
            (format!("no {file} yet: an edit creates it"), c::TEXT_FAINT)
        };
        ui.add(egui::Label::new(RichText::new(text).size(11.5).color(color)).truncate())
            .on_hover_text(
                "What each preset means on the PC and the Web (the consoles' presets are \
                 fixed). cargo xtask quality show prints it too.",
            );
    });
    ui.add(
        egui::Label::new(
            RichText::new(format!(
                "First launch: {} (trino.toml [pc.quality] default)",
                q.project.default
            ))
            .size(11.5)
            .color(c::TEXT_WEAK),
        )
        .truncate(),
    )
    .on_hover_text(
        "auto runs the calibration bench on the player's first launch and keeps the \
         highest preset that holds the frame rate; a preset name skips it",
    );
    if let Some(e) = &q.error {
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

/// The settings a row edits.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    RenderScale,
    DynRes,
    DynMin,
    DynMax,
    Msaa,
    Ao,
    Shadows,
    Post,
    LodBias,
    Particles,
    Filter,
    Anisotropy,
    Sharpen,
    FrameLimit,
}

impl Row {
    const ALL: [Row; 14] = [
        Row::RenderScale,
        Row::DynRes,
        Row::DynMin,
        Row::DynMax,
        Row::Msaa,
        Row::Ao,
        Row::Shadows,
        Row::Post,
        Row::LodBias,
        Row::Particles,
        Row::Filter,
        Row::Anisotropy,
        Row::Sharpen,
        Row::FrameLimit,
    ];

    /// The label, and what the row's accessible names say.
    fn label(self) -> &'static str {
        match self {
            Row::RenderScale => "render scale",
            Row::DynRes => "dynamic resolution",
            Row::DynMin => "dynres min",
            Row::DynMax => "dynres max",
            Row::Msaa => "msaa",
            Row::Ao => "ao",
            Row::Shadows => "shadows",
            Row::Post => "post",
            Row::LodBias => "lod bias",
            Row::Particles => "particles",
            Row::Filter => "texture filter",
            Row::Anisotropy => "anisotropy",
            Row::Sharpen => "sharpen",
            Row::FrameLimit => "frame limit",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Row::RenderScale => "The 3D's resolution against the picture's (the HUD stays sharp)",
            Row::DynRes => "The render scale follows the GPU's frame time within its range",
            Row::DynMin | Row::DynMax => "Dynamic resolution's range",
            Row::Msaa => "Multisampled edges",
            Row::Ao => "Caps the game's ambient occlusion (High keeps it as authored)",
            Row::Shadows => "Shadow map size and filtering (Off: planar shadows only)",
            Row::Post => "Post-processing quality (read by the post stack of issue #64)",
            Row::LodBias => "Multiplies every LOD distance (above 1: detail further away)",
            Row::Particles => "Multiplies every particle effect's cap",
            Row::Filter => "The textures' filter where a game leaves the default",
            Row::Anisotropy => "Anisotropic filtering of 3D textures",
            Row::Sharpen => "Contrast-adaptive sharpening of an upscaled 3D",
            Row::FrameLimit => "VSync, a frame cap or none",
        }
    }
}

fn table(app: &mut App, ui: &mut egui::Ui) {
    let mut preview = None;
    let mut reset = None;
    egui::Grid::new("quality-table")
        .num_columns(1 + QualityPreset::NAMED.len())
        .spacing([10.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label("");
            for p in QualityPreset::NAMED {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let on = app.game_view.quality == p;
                    let r = theme::toggle_icon(
                        ui,
                        on,
                        icon::EYE,
                        "Preview this preset in the Game view (PC or Web profile)",
                    );
                    let name = format!("Preview {}", p.key());
                    r.widget_info(|| {
                        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, &name)
                    });
                    if r.clicked() {
                        preview = Some(p);
                    }
                    let color = if app.quality.changed(p) {
                        c::ACCENT
                    } else {
                        c::TEXT
                    };
                    ui.label(RichText::new(p.label()).family(theme::bold()).color(color));
                });
            }
            ui.end_row();
            for row in Row::ALL {
                ui.label(RichText::new(row.label()).color(c::TEXT_WEAK))
                    .on_hover_text(row.hint());
                for p in QualityPreset::NAMED {
                    let mut q = app.quality.get(p);
                    let name = format!("{} {}", p.key(), row.label());
                    if cell(ui, row, &mut q, &name) {
                        app.quality.set(p, q);
                    }
                }
                ui.end_row();
            }
            ui.label("");
            for p in QualityPreset::NAMED {
                let r = ui
                    .add_enabled(
                        app.quality.changed(p),
                        egui::Button::new(format!("{}  Reset", icon::ARROW_COUNTER_CLOCKWISE)),
                    )
                    .on_hover_text("Back to the engine's values");
                let name = format!("Reset {}", p.key());
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
                if r.clicked() {
                    reset = Some(p);
                }
            }
            ui.end_row();
        });
    if let Some(p) = reset {
        app.quality.reset(p);
    }
    if let Some(p) = preview {
        app.game_view.quality = p;
        app.game_view.dynres = app.quality.get(p).dynamic_resolution.is_some();
    }
}

/// One setting of one preset. True when it changed.
fn cell(ui: &mut egui::Ui, row: Row, q: &mut QualitySettings, name: &str) -> bool {
    let drag = |ui: &mut egui::Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>| {
        // Hundredths, as the file writes them: an unrounded value would
        // read as a change (and a write) every frame.
        let mut x = (*v * 100.0).round() / 100.0;
        let r = ui.add(
            egui::DragValue::new(&mut x)
                .range(range)
                .speed(0.01)
                .max_decimals(2)
                .min_decimals(2),
        );
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, name));
        if r.changed() && x != *v {
            *v = x;
            return true;
        }
        false
    };
    match row {
        Row::RenderScale => drag(ui, &mut q.render_scale, 0.5..=2.0),
        Row::DynRes => {
            let mut on = q.dynamic_resolution.is_some();
            let r = ui.checkbox(&mut on, "");
            r.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, name)
            });
            if r.changed() {
                q.dynamic_resolution = on.then_some(DynRes::DEFAULT);
                return true;
            }
            false
        }
        Row::DynMin | Row::DynMax => match &mut q.dynamic_resolution {
            None => {
                ui.label(RichText::new("—").color(c::TEXT_FAINT));
                false
            }
            Some(d) => {
                let (lo, hi) = (d.min_scale, d.max_scale);
                if row == Row::DynMin {
                    drag(ui, &mut d.min_scale, 0.25..=hi)
                } else {
                    drag(ui, &mut d.max_scale, lo..=2.0)
                }
            }
        },
        Row::Msaa => {
            let mut v = q.msaa;
            let changed = combo(ui, name, &mut v, &[(1, "off"), (4, "4×")]);
            q.msaa = v;
            changed
        }
        Row::Ao => {
            let opts = AoSetting::ALL.map(|l| (l, l.label()));
            combo(ui, name, &mut q.ao, &opts)
        }
        Row::Shadows => {
            let opts = ShadowSetting::ALL.map(|l| (l, l.label()));
            combo(ui, name, &mut q.shadows, &opts)
        }
        Row::Post => {
            let opts = PostSetting::ALL.map(|l| (l, l.label()));
            combo(ui, name, &mut q.post, &opts)
        }
        Row::LodBias => drag(ui, &mut q.lod_bias, 0.1..=4.0),
        Row::Particles => drag(ui, &mut q.particle_scale, 0.1..=4.0),
        Row::Filter => combo(
            ui,
            name,
            &mut q.texture_filter,
            &[
                (TexFilter::Default, "default"),
                (TexFilter::Point, "point"),
                (TexFilter::Bilinear, "bilinear"),
            ],
        ),
        Row::Anisotropy => combo(
            ui,
            name,
            &mut q.anisotropy,
            &[(1, "off"), (2, "2×"), (4, "4×"), (8, "8×"), (16, "16×")],
        ),
        Row::Sharpen => drag(ui, &mut q.sharpen, 0.0..=1.0),
        Row::FrameLimit => {
            let labels: Vec<(FrameLimit, String)> = FrameLimit::CHOICES
                .iter()
                .map(|l| (*l, frame_limit_label(*l)))
                .collect();
            let opts: Vec<(FrameLimit, &str)> =
                labels.iter().map(|(l, s)| (*l, s.as_str())).collect();
            combo(ui, name, &mut q.frame_limit, &opts)
        }
    }
}

pub fn frame_limit_label(l: FrameLimit) -> String {
    match l {
        FrameLimit::VSync => "vsync".into(),
        FrameLimit::Fps(n) => format!("{n} fps"),
        FrameLimit::Unlimited => "unlimited".into(),
    }
}

fn combo<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let mut changed = false;
    let shown = options
        .iter()
        .find(|(v, _)| v == value)
        .map_or("custom", |(_, s)| *s);
    let r = egui::ComboBox::from_id_salt(name)
        .selected_text(shown)
        .width(88.0)
        .truncate()
        .show_ui(ui, |ui| {
            for (v, label) in options {
                if ui.selectable_label(v == value, *label).clicked() && v != value {
                    *value = *v;
                    changed = true;
                }
            }
        });
    let name = name.to_string();
    r.response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &name));
    changed
}
