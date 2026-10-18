//! Inspector › Model › LOD: the distance chain of a model — full mesh →
//! lighter meshes → impostor → nothing — with each level's start per
//! platform, what each level costs, and a preview slider that makes both
//! views draw the entity as if the camera were that far (the switch shows
//! live). Edits the `lod` list of the scene file (`trino_scene::LodDef`).
//! The consoles' tiers (`n64_expanded`, `n3ds_new`) get a start of their
//! own when "Tiers" shows them (the Platforms tab's switches), else they
//! start where their console does.
//!
//! Plain data in, `bool` out: tested without a GPU in `widget_tests.rs`.

use eframe::egui::{self, RichText};
use trino_core::perf::N64_COST;
use trino_scene::{LodAt, LodDef, Target};

use crate::panels::platforms::ModelInfo;
use crate::theme::{self, c, icon};

/// What the levels can draw.
pub struct Choices<'a> {
    pub models: &'a [ModelInfo],
    /// Baked impostors (`impostors/<name>`).
    pub impostors: &'a [String],
    /// The entity's own model (the level before the first one).
    pub base: Option<&'a ModelInfo>,
}

const CULL: &str = "Nothing (cull)";

/// What a level draws, as shown in its combo.
fn draws(l: &LodDef) -> String {
    match (&l.impostor, &l.path) {
        (Some(i), _) => format!("{} {i}", icon::IMAGE_SQUARE),
        (None, Some(p)) => format!("{} {p}", icon::CUBE),
        (None, None) => CULL.to_string(),
    }
}

/// One draw of a level, in words: triangles and the N64 estimate.
pub fn cost(l: &LodDef, models: &[ModelInfo]) -> String {
    match (&l.impostor, &l.path) {
        (Some(_), _) => format!(
            "impostor · 2 tris · N64 ≈{:.2} ms",
            N64_COST.billboards_us(1) as f32 / 1000.0
        ),
        (None, Some(p)) => match models.iter().find(|m| &m.logical == p) {
            Some(m) => format!(
                "{} tris · N64 ≈{:.2} ms",
                m.tris,
                N64_COST.mesh_draw_us(m.verts as u32, m.tris as u32) as f32 / 1000.0
            ),
            None => format!("{p} is not baked"),
        },
        (None, None) => "draws nothing".to_string(),
    }
}

/// Which level draws at `distance` on the PC (`None` = the base model),
/// by the same rule as `trino_scene::DrawItem::level_at`.
pub fn level_at(lod: &[LodDef], distance: f32) -> Option<usize> {
    let mut order: Vec<usize> = (0..lod.len()).collect();
    order.sort_by(|a, b| lod[*a].distance.total_cmp(&lod[*b].distance));
    order
        .into_iter()
        .take_while(|i| distance >= lod[*i].distance)
        .last()
}

/// The section body. `preview` is the preview distance while the preview
/// is on. Returns true when a level changed.
pub fn ui(
    ui: &mut egui::Ui,
    lod: &mut Vec<LodDef>,
    choices: &Choices,
    preview: &mut Option<f32>,
) -> bool {
    let mut changed = false;
    let mut remove = None;
    let own = |t| lod.iter().any(|l| l.platforms.own(t).is_some());
    if !lod.is_empty() {
        crate::panels::platforms::tier_toggles(ui, own);
    }
    let targets = crate::panels::platforms::with_tiers(ui.ctx(), &PLATFORMS, own);
    let tiers = (targets.len() - PLATFORMS.len()) as f32;
    for (i, level) in lod.iter_mut().enumerate() {
        let n = i + 1;
        ui.horizontal(|ui| {
            changed |= ui
                .add(
                    egui::DragValue::new(&mut level.distance)
                        .speed(0.25)
                        .range(0.0..=10_000.0)
                        .suffix(" m"),
                )
                .on_hover_text("Camera distance where this level starts")
                .changed();
            // Leave room for the remove button (26 px + its padding).
            let w = (ui.available_width() - 44.0).max(60.0);
            let r = theme::fixed_width(ui, w, |ui| {
                egui::ComboBox::from_id_salt(("lod-draws", i))
                    .selected_text(draws(level))
                    .width(w)
                    .truncate()
                    .height(300.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                level.path.is_none() && level.impostor.is_none(),
                                CULL,
                            )
                            .clicked()
                        {
                            level.path = None;
                            level.impostor = None;
                            changed = true;
                        }
                        for imp in choices.impostors {
                            let sel = level.impostor.as_deref() == Some(imp.as_str());
                            if ui
                                .selectable_label(sel, format!("{} {imp}", icon::IMAGE_SQUARE))
                                .clicked()
                            {
                                level.impostor = Some(imp.clone());
                                level.path = None;
                                changed = true;
                            }
                        }
                        for m in choices.models {
                            let sel = level.impostor.is_none()
                                && level.path.as_deref() == Some(m.logical.as_str());
                            if ui
                                .selectable_label(sel, format!("{} {}", icon::CUBE, m.logical))
                                .clicked()
                            {
                                level.path = Some(m.logical.clone());
                                level.impostor = None;
                                changed = true;
                            }
                        }
                    })
            });
            let label = format!("LOD {n} draws");
            r.response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &label)
            });
            if theme::icon_button(ui, icon::X, "Remove level").clicked() {
                remove = Some(i);
            }
        });
        // Where this level starts on each platform: one row when it fits,
        // a row per platform in a narrow inspector.
        if ui.available_width() >= 470.0 + tiers * 120.0 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for &t in &targets {
                    changed |= platform_at(ui, n, t, level);
                }
            });
        } else {
            for &t in &targets {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    changed |= platform_at(ui, n, t, level);
                });
            }
        }
        ui.label(
            RichText::new(cost(level, choices.models))
                .size(11.0)
                .color(c::TEXT_FAINT),
        );
        ui.add_space(2.0);
    }
    if let Some(i) = remove {
        lod.remove(i);
        changed = true;
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .small_button(format!("{} Add level", icon::PLUS))
            .on_hover_text(
                "A lighter mesh (a fake object), an impostor (Model asset › Impostor) \
                 or nothing — farther each time",
            )
            .clicked()
        {
            let next = lod.iter().map(|l| l.distance).fold(0.0f32, f32::max) + 15.0;
            lod.push(LodDef {
                distance: next,
                ..LodDef::default()
            });
            changed = true;
        }
    });

    // Live preview: draw this entity as if the camera were `d` away.
    if !lod.is_empty() {
        let max = lod.iter().map(|l| l.distance).fold(10.0f32, f32::max) * 1.3;
        let mut on = preview.is_some();
        let mut d = preview.unwrap_or(0.0).min(max);
        ui.checkbox(&mut on, "Preview LOD")
            .on_hover_text("Both views draw this entity at the distance on the slider");
        if on {
            ui.horizontal(|ui| {
                // The track plus its value box (~70 px with the suffix).
                ui.spacing_mut().slider_width = (ui.available_width() - 80.0).clamp(30.0, 220.0);
                ui.add(egui::Slider::new(&mut d, 0.0..=max).suffix(" m"))
                    .on_hover_text("Preview distance");
            });
        }
        *preview = on.then_some(d);
        if let Some(d) = *preview {
            let text = match level_at(lod, d) {
                None => match choices.base {
                    Some(b) => format!("at {d:.1} m: the full mesh ({} tris)", b.tris),
                    None => format!("at {d:.1} m: the full mesh"),
                },
                Some(i) => format!(
                    "at {d:.1} m: LOD {} — {}",
                    i + 1,
                    cost(&lod[i], choices.models)
                ),
            };
            ui.add(
                egui::Label::new(RichText::new(text).size(11.5).color(c::ACCENT_TEXT)).truncate(),
            );
        }
    }
    changed
}

/// One platform's start of level `n`: same as the level, its own
/// distance, or never there.
/// The platforms a level's start row shows, the consoles first.
const PLATFORMS: [Target; 4] = [Target::N64, Target::N3ds, Target::Pc, Target::Web];

fn platform_at(ui: &mut egui::Ui, n: usize, t: Target, level: &mut LodDef) -> bool {
    let mut changed = false;
    let base = level.distance;
    // The Web follows the PC, a tier its console, until it has its own.
    let from = t.fallback();
    let slot = level.platforms.get_mut(t);
    let shown = match (slot.as_ref(), from) {
        (None, Some(f)) => format!("={}", f.label()),
        (None, None) => "=".to_string(),
        (Some(LodAt::At(_)), _) => "at".to_string(),
        (Some(LodAt::Never), _) => "never".to_string(),
    };
    ui.add_sized(
        [26.0, 20.0],
        egui::Label::new(
            RichText::new(crate::panels::platforms::tier_short(t))
                .size(11.0)
                .color(c::TEXT_WEAK),
        ),
    )
    .on_hover_text(t.long_label());
    let label = format!("LOD {n} on {}", t.label());
    // 0 = same as the level, 1 = its own distance, 2 = never.
    let mode = match slot {
        None => 0,
        Some(LodAt::At(_)) => 1,
        Some(LodAt::Never) => 2,
    };
    let r = egui::ComboBox::from_id_salt(("lod-at", n, t.key()))
        .selected_text(shown)
        .width(54.0)
        .show_ui(ui, |ui| {
            let same = from.map_or("= level".to_string(), |f| format!("= the {}", f.label()));
            for (m, text) in [same.as_str(), "own distance", "never"]
                .into_iter()
                .enumerate()
            {
                if ui.selectable_label(mode == m, text).clicked() && mode != m {
                    *slot = match m {
                        0 => None,
                        1 => Some(LodAt::At(base)),
                        _ => Some(LodAt::Never),
                    };
                    changed = true;
                }
            }
        });
    let hint = match t {
        Target::N64 => "On the N64 an impostor is CPU-submitted (~44 µs): measure before \
                        switching to it early — or skip the level there"
            .to_string(),
        Target::Web => "Where this level starts in a browser: the PC's start unless the Web \
                        has its own"
            .to_string(),
        t if t.hw_tier().is_expanded() => format!(
            "Where this level starts on the {}: the {}'s start unless it has its own",
            t.long_label(),
            t.base().label()
        ),
        _ => "Where this level starts on this platform".to_string(),
    };
    r.response
        .on_hover_text(hint)
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &label));
    if let Some(LodAt::At(d)) = slot {
        changed |= ui
            .add(
                egui::DragValue::new(d)
                    .speed(0.25)
                    .range(0.0..=10_000.0)
                    .suffix(" m"),
            )
            .changed();
    }
    changed
}
