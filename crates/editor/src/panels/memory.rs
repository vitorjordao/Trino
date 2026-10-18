//! The Memory tab (issue #56, ADR 0022): what the project's assets take of
//! each console's RAM — the engine reserve and the game's state off the
//! top (`[memory]`), then boot and each group — and what the open scene
//! loads at once (its `requires` and its cells' groups) against what is
//! left: the numbers `cargo xtask assets report` prints and strict mode
//! enforces. Measured on a worker whenever the manifest or a bake moves.

use eframe::egui::{self, RichText};
use trino_asset_pipeline::memory::{self, MemoryPlan, show_bytes};

use crate::app::App;
use crate::theme::{self, c, icon};

/// Cards per row from this width on (one below it).
const TWO_COLUMNS: f32 = 620.0;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let scene = trino_asset_pipeline::groups::scene_groups(&app.doc.scene);
    let name = app.doc.title();
    let ctx = ui.ctx().clone();
    let assets = app.project.assets_dir();
    let generation = app.project.generation;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{}  Asset memory", icon::MEMORY))
                        .family(theme::bold())
                        .color(c::TEXT),
                );
                ui.label(
                    RichText::new(format!("{name} loads {}", scene.join(" + ")))
                        .size(11.5)
                        .color(c::TEXT_WEAK),
                );
                if app.groups.measuring() {
                    ui.add(egui::Spinner::new().size(12.0));
                }
            });
            ui.label(
                RichText::new(
                    "RAM per console tier: the engine and the game's state first, then \
                     each group's assets. Same numbers: cargo xtask assets report.",
                )
                .size(11.0)
                .color(c::TEXT_FAINT),
            );
            ui.add_space(4.0);
            match app.groups.plans(&ctx, &assets, generation) {
                None => {
                    ui.label(RichText::new("Measuring…").color(c::TEXT_WEAK));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(c::ERROR).size(12.0));
                }
                Some(Ok(plans)) => {
                    let refs: Vec<&str> = scene.iter().map(String::as_str).collect();
                    if ui.available_width() >= TWO_COLUMNS {
                        for pair in plans.chunks(2) {
                            ui.columns(pair.len().max(2), |cols| {
                                for (col, plan) in cols.iter_mut().zip(pair) {
                                    card(col, plan, &refs);
                                }
                            });
                            ui.add_space(6.0);
                        }
                    } else {
                        for plan in plans {
                            card(ui, plan, &refs);
                            ui.add_space(6.0);
                        }
                    }
                }
            }
        });
}

/// One console tier: the RAM split, the scene's bar, every group's bytes.
fn card(ui: &mut egui::Ui, plan: &MemoryPlan, scene: &[&str]) {
    let label = format!(
        "{} {}",
        memory::platform_label(plan.platform),
        plan.tier.key()
    );
    crate::app::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&label).family(theme::bold()).color(c::TEXT));
            ui.label(
                RichText::new(format!(
                    "{} RAM · {} engine · {} game · {} for assets",
                    show_bytes(plan.ram),
                    show_bytes(plan.reserve),
                    show_bytes(plan.game),
                    show_bytes(plan.budget())
                ))
                .size(11.0)
                .color(c::TEXT_WEAK),
            );
        });
        let used = plan.resident(scene);
        let budget = plan.budget().max(1);
        let frac = used as f32 / budget as f32;
        let fill = if frac > 1.0 {
            c::ERROR
        } else if frac > 0.85 {
            c::WARN
        } else {
            c::OK
        };
        theme::prop(ui, "Scene", |ui| {
            ui.add(
                egui::ProgressBar::new(frac.min(1.0))
                    .desired_width(ui.available_width() - 4.0)
                    // Dimmed: the bar's white text stays readable on it.
                    .fill(fill.gamma_multiply(0.55))
                    .text(format!(
                        "{} / {} ({:.0}%)",
                        show_bytes(used),
                        show_bytes(plan.budget()),
                        frac * 100.0
                    )),
            );
        });
        if let Err(e) = plan.check(scene) {
            ui.label(RichText::new(e).size(11.0).color(c::ERROR));
        }
        let [meshes, textures, audio, data] = plan.resident_by_category(scene);
        ui.label(
            RichText::new(format!(
                "meshes {} · textures {} · audio {} · data {}",
                show_bytes(meshes),
                show_bytes(textures),
                show_bytes(audio),
                show_bytes(data)
            ))
            .size(11.0)
            .color(c::TEXT_FAINT),
        );
        ui.add_space(2.0);
        egui::Grid::new(("memory-groups", &label))
            .num_columns(3)
            .spacing([12.0, 2.0])
            .striped(true)
            .show(ui, |ui| {
                for h in ["Group", "Assets", "In RAM"] {
                    ui.label(RichText::new(h).size(11.0).color(c::TEXT_FAINT));
                }
                ui.end_row();
                for g in &plan.groups {
                    let loaded = scene.contains(&g.as_str()) || g == "boot";
                    let text = if loaded {
                        format!("{} {g}", icon::CHECK)
                    } else {
                        g.clone()
                    };
                    ui.label(RichText::new(text).size(12.0).color(if loaded {
                        c::TEXT
                    } else {
                        c::TEXT_WEAK
                    }))
                    .on_hover_text(if loaded {
                        "the open scene loads it"
                    } else {
                        "not loaded by the open scene"
                    });
                    let n = plan
                        .assets
                        .iter()
                        .filter(|a| a.groups.iter().any(|x| x == g))
                        .count();
                    ui.label(RichText::new(n.to_string()).size(12.0).color(c::TEXT_WEAK));
                    ui.label(
                        RichText::new(show_bytes(plan.group_bytes(g)))
                            .size(12.0)
                            .color(c::TEXT_WEAK),
                    );
                    ui.end_row();
                }
            });
    });
}
