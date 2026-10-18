//! Inspector › Platforms: an entity's variants for PC, N64, 3DS and the Web
//! side by side (`Entity::platforms` in the scene file). Each column but
//! the PC's picks a model and a material ("Same as PC" by default), a
//! scale, hides the entity or leaves it out, and shows what its model
//! costs. The Web column starts from the PC's variant (the Web follows the
//! PC until it has its own, `PlatformVariants::get`). The Game view draws
//! the variant of the platform it simulates.
//!
//! The consoles' hardware tiers (issue #54: `n64_expanded`, `n3ds_new`)
//! are collapsed under their console: "Tiers" shows a tier's column after
//! its console's, starting from the console's variant (a tier follows its
//! console until it has its own). The LOD and particle panels share the
//! switches ([`tier_toggles`], [`with_tiers`]).
//!
//! Plain data in, `bool` out: tested without a GPU in `widget_tests.rs`.

use eframe::egui::{self, RichText};
use trino_core::perf::N64_COST;
use trino_scene::{Entity, Target, Variant};

use crate::theme::{self, c, icon};

/// A model the pickers offer, with what one draw of it costs.
#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub logical: String,
    pub verts: usize,
    pub tris: usize,
}

/// What the columns can choose from.
pub struct Choices<'a> {
    pub models: &'a [ModelInfo],
    pub materials: &'a [String],
}

/// Below this width the four columns stack (the inspector can be narrow).
pub const COLUMNS_MIN_WIDTH: f32 = 330.0;

/// What each shown tier column adds to [`COLUMNS_MIN_WIDTH`].
pub const TIER_COLUMN_WIDTH: f32 = 70.0;

const SAME: &str = "Same as PC";

/// The section body. Returns true when a variant changed.
pub fn ui(ui: &mut egui::Ui, e: &mut Entity, choices: &Choices) -> bool {
    let base_model = e.model.as_ref().map(|m| m.path.clone());
    let base_material = e.model.as_ref().and_then(|m| m.material.clone());
    let mut changed = false;
    let own = |t| e.platforms.own(t).is_some();
    tier_toggles(ui, own);
    let targets = with_tiers(ui.ctx(), &Target::ALL, own);
    let tiers = (targets.len() - Target::ALL.len()) as f32;
    if ui.available_width() >= COLUMNS_MIN_WIDTH + tiers * TIER_COLUMN_WIDTH {
        changed = grid(
            ui,
            e,
            &targets,
            base_model.as_deref(),
            base_material.as_deref(),
            choices,
        );
    } else {
        for t in targets {
            changed |= target_column(
                ui,
                t,
                e,
                base_model.as_deref(),
                base_material.as_deref(),
                choices,
            );
            ui.separator();
        }
    }
    if changed {
        e.platforms.prune();
    }
    ui.label(
        RichText::new(
            "Variants are a look: colliders, tags and children stay as on PC. \
             The Game view shows the console it simulates.",
        )
        .size(11.0)
        .color(c::TEXT_FAINT),
    );
    changed
}

/// Is `tier`'s column (row, cap) shown? When the data has a value of its
/// own there (`own`), or the user showed it ([`tier_toggles`]; one switch
/// per tier for every panel).
pub fn tier_shown(ctx: &egui::Context, tier: Target, own: bool) -> bool {
    own || ctx.data(|d| d.get_temp(tier_id(tier)).unwrap_or(false))
}

fn tier_id(tier: Target) -> egui::Id {
    egui::Id::new(("show-hw-tier", tier.key()))
}

/// "Tiers ☐ N64+Pak ☐ New 3DS": shows or collapses each tier's column
/// (accessible as "Show N64 + Expansion Pak", "Show New 3DS"). A tier with
/// a value of its own stays shown.
pub fn tier_toggles(ui: &mut egui::Ui, own: impl Fn(Target) -> bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Tiers").size(11.5).color(c::TEXT_FAINT))
            .on_hover_text(
                "The consoles' expanded hardware: a tier takes its console's value                  until it has its own",
            );
        for t in Target::TIERS {
            let has = own(t);
            let mut on = tier_shown(ui.ctx(), t, has);
            let r = ui
                .add_enabled(
                    !has,
                    egui::Checkbox::new(&mut on, RichText::new(t.label()).size(11.5)),
                )
                .on_hover_text(format!(
                    "Show the {} column (as on {} until it has its own)",
                    t.long_label(),
                    t.base().label()
                ))
                .on_disabled_hover_text(format!(
                    "{} has a value of its own here: shown",
                    t.long_label()
                ));
            let name = format!("Show {}", t.long_label());
            r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, !has, on, &name));
            if r.changed() {
                ui.data_mut(|d| d.insert_temp(tier_id(t), on));
            }
        }
    });
}

/// `targets` with each console's tier right after it where it is shown
/// ([`tier_shown`]; `own` says where the data has a value of its own).
pub fn with_tiers(
    ctx: &egui::Context,
    targets: &[Target],
    own: impl Fn(Target) -> bool,
) -> Vec<Target> {
    let mut out = Vec::new();
    for &t in targets {
        out.push(t);
        if let Some(tier) = t
            .expanded()
            .filter(|x| *x != t && tier_shown(ctx, *x, own(*x)))
        {
            out.push(tier);
        }
    }
    out
}

/// A tier's name under its console, where columns are narrow: `+Pak`,
/// `New` (the target's label otherwise).
pub fn tier_short(t: Target) -> &'static str {
    match t {
        Target::N64Expanded => "+Pak",
        Target::N3dsNew => "New",
        t => t.label(),
    }
}

/// "as on N64": what a target with no value of its own takes (a tier its
/// console's, the Web the PC's); `None` for the others.
pub fn inherits(t: Target) -> Option<String> {
    t.fallback().map(|f| format!("as on {}", f.label()))
}

fn glyph(t: Target) -> &'static str {
    match t {
        Target::Pc => icon::DESKTOP,
        Target::N64 | Target::N64Expanded => icon::GAME_CONTROLLER,
        Target::N3ds | Target::N3dsNew => icon::DEVICE_MOBILE,
        Target::Web => icon::BROWSER,
    }
}

/// A check box without text whose accessible name is `label`.
fn check(ui: &mut egui::Ui, value: &mut bool, label: String, hint: &str) -> bool {
    let r = ui
        .add(egui::Checkbox::without_text(value))
        .on_hover_text(hint);
    let v = *value;
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, v, &label));
    r.changed()
}

/// One row: a fixed-width title, then one cell per platform.
fn row(
    ui: &mut egui::Ui,
    title: &str,
    targets: &[Target],
    widths: (f32, f32),
    mut cell: impl FnMut(&mut egui::Ui, usize, Target),
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.allocate_ui_with_layout(
            egui::vec2(widths.0, 22.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_width(widths.0);
                ui.add(egui::Label::new(RichText::new(title).color(c::TEXT_WEAK)).truncate());
            },
        );
        for (i, &t) in targets.iter().enumerate() {
            ui.allocate_ui_with_layout(
                egui::vec2(widths.1, 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_width(widths.1);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    cell(ui, i, t)
                },
            );
        }
    });
}

/// A column per target (PC, N64, 3DS, Web, and the tiers shown after
/// their console), one property per row. A column starts from what its
/// target draws (`PlatformVariants::get`: the Web the PC's variant, a tier
/// its console's) and writes the target's own variant.
fn grid(
    ui: &mut egui::Ui,
    e: &mut Entity,
    targets: &[Target],
    base_model: Option<&str>,
    base_material: Option<&str>,
    choices: &Choices,
) -> bool {
    let label_w = 52.0;
    let n = targets.len() as f32;
    let col_w = ((ui.available_width() - label_w - n * 4.0) / n).floor();
    let widths = (label_w, col_w);
    let mut vs: Vec<Variant> = targets
        .iter()
        .map(|t| e.platforms.get(*t).cloned().unwrap_or_default())
        .collect();
    let before = vs.clone();
    let owned: Vec<bool> = targets
        .iter()
        .map(|t| e.platforms.own(*t).is_some())
        .collect();
    let mut reset = vec![false; targets.len()];

    row(ui, "", targets, widths, |ui, i, t| {
        let name = if t.hw_tier().is_expanded() {
            tier_short(t)
        } else {
            t.label()
        };
        ui.label(
            RichText::new(format!("{} {name}", glyph(t)))
                .size(12.5)
                .family(theme::bold())
                .color(c::TEXT),
        )
        .on_hover_text(t.long_label());
        let back = t.fallback().unwrap_or(Target::Pc);
        if owned[i]
            && ui
                .add(
                    egui::Button::new(RichText::new(icon::ARROW_COUNTER_CLOCKWISE).size(12.0))
                        .frame_when_inactive(false)
                        .min_size(egui::vec2(16.0, 16.0)),
                )
                .on_hover_text(format!("Back to the {} look", back.label()))
                .clicked()
        {
            reset[i] = true;
        }
    });
    // With a tier shown: where each column's look comes from.
    if targets.iter().any(|t| t.hw_tier().is_expanded()) {
        row(ui, "From", targets, widths, |ui, i, t| {
            let Some(from) = inherits(t) else {
                return;
            };
            let (text, color) = if owned[i] {
                ("own".to_string(), c::TEXT_WEAK)
            } else {
                (from, c::TEXT_FAINT)
            };
            ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).truncate())
                .on_hover_text(format!(
                    "{}: a variant of its own, or its {}'s until it has one",
                    t.long_label(),
                    t.fallback().unwrap_or(Target::Pc).label()
                ));
        });
    }
    if let Some(base) = base_model {
        row(ui, "Model", targets, widths, |ui, i, t| {
            if t == Target::Pc {
                ui.add(egui::Label::new(RichText::new(short(base)).color(c::TEXT_WEAK)).truncate())
                    .on_hover_text(format!("{base}: the Model component's mesh"));
            } else {
                pick(
                    ui,
                    (t, "model"),
                    &format!("{} model", t.label()),
                    &mut vs[i].model,
                    choices.models.iter().map(|m| m.logical.as_str()),
                    col_w,
                );
            }
        });
        row(ui, "Material", targets, widths, |ui, i, t| {
            if t == Target::Pc {
                let m = base_material.unwrap_or("Default");
                ui.add(egui::Label::new(RichText::new(short(m)).color(c::TEXT_FAINT)).truncate())
                    .on_hover_text(m);
            } else {
                pick(
                    ui,
                    (t, "material"),
                    &format!("{} material", t.label()),
                    &mut vs[i].material,
                    choices.materials.iter().map(String::as_str),
                    col_w,
                );
            }
        });
    }
    let base_scale = e.transform.scale;
    row(ui, "Scale", targets, widths, |ui, i, t| {
        let mut own = vs[i].scale.is_some();
        if check(
            ui,
            &mut own,
            format!("Scale on {}", t.label()),
            "A scale of its own: this entity's model (or prefab) only",
        ) {
            vs[i].scale = own.then_some(base_scale);
        }
        if let Some(s) = &mut vs[i].scale {
            if s[0] == s[1] && s[1] == s[2] {
                let mut u = s[0];
                if ui
                    .add(egui::DragValue::new(&mut u).speed(0.01).max_decimals(2))
                    .on_hover_text("Uniform scale")
                    .changed()
                {
                    *s = [u; 3];
                }
            } else {
                ui.label(RichText::new(format!("{:.2},{:.2},{:.2}", s[0], s[1], s[2])).size(11.0))
                    .on_hover_text("Non-uniform: edit it in the RON view or the file");
            }
        }
    });
    row(ui, "Hidden", targets, widths, |ui, i, t| {
        let mut hidden = vs[i].visible == Some(false);
        if check(
            ui,
            &mut hidden,
            format!("Hidden on {}", t.label()),
            "Like the eye in the Hierarchy, for this platform only",
        ) {
            vs[i].visible = hidden.then_some(false);
        }
    });
    row(ui, "Left out", targets, widths, |ui, i, t| {
        check(
            ui,
            &mut vs[i].disabled,
            format!("Not on {}", t.label()),
            "The entity and its children don't exist on this platform",
        );
    });
    row(ui, "Cost", targets, widths, |ui, i, t| {
        let v = &vs[i];
        let model = v.model.as_deref().or(base_model);
        if !Variant::shows(Some(v), e.visible) {
            ui.label(
                RichText::new(format!("{} none", icon::EYE_SLASH))
                    .size(11.0)
                    .color(c::TEXT_FAINT),
            );
        } else if let Some(m) = model.and_then(|p| choices.models.iter().find(|m| m.logical == p)) {
            let mut line = format!("{} tris", m.tris);
            if t.base() == Target::N64 {
                let us = N64_COST.mesh_draw_us(m.verts as u32, m.tris as u32);
                line.push_str(&format!(" ≈{:.1}ms", us as f32 / 1000.0));
            }
            ui.add(
                egui::Label::new(RichText::new(line).size(11.0).color(c::TEXT_FAINT)).truncate(),
            )
            .on_hover_text(
                "One draw; the N64 time comes from the calibrated cost model \
                     (cargo xtask model inspect)",
            );
        }
    });

    let mut changed = false;
    for (i, &t) in targets.iter().enumerate() {
        let slot = e.platforms.get_mut(t);
        if reset[i] {
            changed |= slot.take().is_some();
        } else if vs[i] != before[i] {
            *slot = Some(vs[i].clone());
            changed = true;
        }
    }
    changed
}

/// One platform as a vertical block (narrow inspectors).
fn target_column(
    ui: &mut egui::Ui,
    t: Target,
    e: &mut Entity,
    base_model: Option<&str>,
    base_material: Option<&str>,
    choices: &Choices,
) -> bool {
    let mut changed = false;
    let width = ui.available_width();
    let has_variant = e.platforms.own(t).is_some();
    let back = t.fallback().unwrap_or(Target::Pc).label();
    ui.horizontal(|ui| {
        ui.add(
            egui::Label::new(
                RichText::new(format!("{} {}", glyph(t), t.long_label()))
                    .family(theme::bold())
                    .color(c::TEXT),
            )
            .truncate(),
        );
        if has_variant
            && theme::icon_button(
                ui,
                icon::ARROW_COUNTER_CLOCKWISE,
                &format!("Back to the {back} look"),
            )
            .clicked()
        {
            *e.platforms.get_mut(t) = None;
            changed = true;
        }
    });
    // A tier (and the Web) starts from what it draws: its console's (the
    // PC's) variant until it has its own.
    if let Some(from) = inherits(t).filter(|_| !has_variant && t.hw_tier().is_expanded()) {
        ui.label(RichText::new(from).size(11.0).color(c::TEXT_FAINT));
    }
    let mut v = e.platforms.get(t).cloned().unwrap_or_default();
    let before = v.clone();

    let combo_w = (width - 4.0).max(60.0);
    if let Some(base) = base_model {
        if t == Target::Pc {
            ui.add(egui::Label::new(RichText::new(base).color(c::TEXT_WEAK)).truncate())
                .on_hover_text("The PC draws the Model component's mesh");
        } else {
            pick(
                ui,
                (t, "model"),
                &format!("{} model", t.label()),
                &mut v.model,
                choices.models.iter().map(|m| m.logical.as_str()),
                combo_w,
            );
        }
        if t != Target::Pc {
            pick(
                ui,
                (t, "material"),
                &format!("{} material", t.label()),
                &mut v.material,
                choices.materials.iter().map(String::as_str),
                combo_w,
            );
        } else {
            let m = base_material.unwrap_or("Default material");
            ui.add(egui::Label::new(RichText::new(m).color(c::TEXT_FAINT)).truncate());
        }
    }

    let mut own_scale = v.scale.is_some();
    if ui
        .checkbox(&mut own_scale, format!("Scale on {}", t.label()))
        .on_hover_text("This entity's own model (or prefab) only; children keep theirs")
        .changed()
    {
        v.scale = own_scale.then_some(e.transform.scale);
    }
    if let Some(s) = &mut v.scale {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let w = ((ui.available_width() - 6.0) / 3.0).max(36.0);
            for x in s.iter_mut() {
                ui.add_sized(
                    [w, 20.0],
                    egui::DragValue::new(x).speed(0.01).max_decimals(3),
                );
            }
        });
    }
    let mut hidden = v.visible == Some(false);
    if ui
        .checkbox(&mut hidden, format!("Hidden on {}", t.label()))
        .on_hover_text("Like the eye in the Hierarchy, for this platform only")
        .changed()
    {
        v.visible = hidden.then_some(false);
    }
    ui.checkbox(&mut v.disabled, format!("Not on {}", t.label()))
        .on_hover_text("The entity and its children don't exist on this platform");

    // What this platform's model costs.
    let model = v.model.as_deref().or(base_model);
    if !Variant::shows(Some(&v), e.visible) {
        ui.label(
            RichText::new(format!("{} nothing drawn", icon::EYE_SLASH))
                .size(11.0)
                .color(c::TEXT_FAINT),
        );
    } else if let Some(m) = model.and_then(|p| choices.models.iter().find(|m| m.logical == p)) {
        let mut line = format!("{} {} tris", icon::TRIANGLE, m.tris);
        if t.base() == Target::N64 {
            let us = N64_COST.mesh_draw_us(m.verts as u32, m.tris as u32);
            line.push_str(&format!(" · ≈{:.1} ms", us as f32 / 1000.0));
        }
        ui.add(egui::Label::new(RichText::new(line).size(11.0).color(c::TEXT_FAINT)).truncate())
            .on_hover_text(
                "One draw, estimated with the calibrated N64 cost model (model inspect)",
            );
    }

    if v != before {
        *e.platforms.get_mut(t) = Some(v);
        changed = true;
    }
    changed
}

/// `models/tree` → `tree`: grid cells are narrow, the folder is implied.
fn short(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// "Same as PC" or one of `options`, as a combo whose accessible label is
/// `label` (the UI tests and screen readers find it by that name). Narrow
/// combos show short names ("= PC", `tree`); the list shows full paths.
fn pick<'a>(
    ui: &mut egui::Ui,
    id: (Target, &str),
    label: &str,
    value: &mut Option<String>,
    options: impl Iterator<Item = &'a str>,
    width: f32,
) {
    let narrow = width < 140.0;
    let shown = match (value.as_deref(), narrow) {
        (None, true) => "= PC".to_string(),
        (None, false) => SAME.to_string(),
        (Some(v), true) => short(v).to_string(),
        (Some(v), false) => v.to_string(),
    };
    let r = egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(width)
        .truncate()
        .height(300.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(value.is_none(), SAME).clicked() {
                *value = None;
            }
            for o in options {
                if ui
                    .selectable_label(value.as_deref() == Some(o), o)
                    .clicked()
                {
                    *value = Some(o.to_string());
                }
            }
        });
    let label = label.to_string();
    let hover = value.clone().unwrap_or_else(|| SAME.into());
    r.response
        .on_hover_text(hover)
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &label));
}
