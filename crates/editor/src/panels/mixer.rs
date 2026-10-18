//! The Mixer tab (issue #53): the project's mix — each bus's starting
//! volume and the duck rules — as `assets/audio.toml`, the text file an
//! agent writes just as well (the bake turns it into the `audio/mixer`
//! asset every backend applies at boot). Edits are written once the user
//! settles; an edit on disk shows here live; an invalid mix is shown, not
//! written. The mutes are the preview player's only (players mute in the
//! game's options, which persist apart and win over the mix).

use eframe::egui::{self, RichText, Vec2};
use trino_asset_pipeline::audio::DuckDecl;
use trino_core::{Bus, MAX_DUCK_RULES};

use crate::app::App;
use crate::theme::{self, c, icon};

/// Below this width the buses and the duck rules stack.
const TWO_COLUMNS: f32 = 640.0;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let mut changed = false;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            header(app, ui);
            ui.add_space(4.0);
            if ui.available_width() >= TWO_COLUMNS {
                ui.columns(2, |cols| {
                    changed |= buses(app, &mut cols[0]);
                    changed |= ducks(app, &mut cols[1]);
                });
            } else {
                changed |= buses(app, ui);
                ui.add_space(8.0);
                changed |= ducks(app, ui);
            }
        });
    // The preview player follows at once (the file once the user settles).
    if changed && let Some(p) = &mut app.audio.player {
        app.mixer.apply(&mut p.audio);
    }
}

fn header(app: &App, ui: &mut egui::Ui) {
    let m = &app.mixer;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{}  Project mix", icon::FADERS_HORIZONTAL))
                .family(theme::bold())
                .color(c::TEXT),
        );
        let (text, color) = if m.dirty {
            ("unsaved", c::WARN)
        } else if m.exists {
            ("assets/audio.toml", c::TEXT_FAINT)
        } else {
            (
                "no assets/audio.toml yet: an edit creates it",
                c::TEXT_FAINT,
            )
        };
        ui.add(egui::Label::new(RichText::new(text).size(11.5).color(color)).truncate())
            .on_hover_text(
                "Each bus's starting volume and the duck rules; the player's own volumes \
                 (the game's options) persist apart and win. cargo xtask audio report \
                 prints it too.",
            );
    });
    if let Some(e) = &m.error {
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

/// One row per bus: preview mute, name, volume. True when anything moved.
fn buses(app: &mut App, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    theme::section_title(ui, icon::SPEAKER_HIGH, "Buses");
    for bus in Bus::ALL {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let muted = app.mixer.muted[bus.index()];
            let glyph = if muted {
                icon::SPEAKER_SLASH
            } else {
                icon::SPEAKER_HIGH
            };
            let name = format!("Mute {}", bus.label());
            let r = theme::toggle_icon(ui, muted, glyph, "Mute in the editor's preview only");
            r.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, muted, &name)
            });
            if r.clicked() {
                app.mixer.muted[bus.index()] = !muted;
                changed = true;
            }
            ui.add_sized(
                [70.0, 22.0],
                egui::Label::new(RichText::new(bus.label()).color(if bus == Bus::Master {
                    c::TEXT
                } else {
                    c::TEXT_WEAK
                }))
                .truncate(),
            );
            // Whole percents, as the slider rounds them: an unrounded
            // value would read as a change (and a write) every frame.
            let mut pct = (app.mixer.volume(bus) * 100.0).round();
            ui.spacing_mut().slider_width = (ui.available_width() - 64.0).clamp(40.0, 260.0);
            let r = ui.add(
                egui::Slider::new(&mut pct, 0.0..=100.0)
                    .suffix("%")
                    .max_decimals(0),
            );
            crate::panels::audio::name_slider(&r, &format!("{} volume", bus.label()));
            if r.changed() {
                app.mixer.set_volume(bus, pct / 100.0);
                changed = true;
            }
        });
    }
    changed
}

/// The duck rules: trigger → target, level, attack, release; add, remove.
fn ducks(app: &mut App, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    theme::section_title(ui, icon::ARROW_RIGHT, "Duck rules");
    if app.mixer.file.duck.is_empty() {
        ui.label(
            RichText::new("None: no bus pulls another down")
                .size(11.5)
                .color(c::TEXT_FAINT),
        );
    }
    let mut remove = None;
    let targets: Vec<Bus> = Bus::ALL.into_iter().filter(|b| *b != Bus::Master).collect();
    for (i, d) in app.mixer.file.duck.iter_mut().enumerate() {
        let n = i + 1;
        crate::app::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                changed |= bus_combo(
                    ui,
                    ("duck-trigger", i),
                    &format!("Duck {n} trigger"),
                    &mut d.trigger,
                    &targets,
                );
                ui.label(RichText::new("ducks").size(11.5).color(c::TEXT_FAINT));
                changed |= bus_combo(
                    ui,
                    ("duck-target", i),
                    &format!("Duck {n} target"),
                    &mut d.target,
                    &targets,
                );
                if theme::named_icon_button(ui, icon::TRASH, &format!("Remove duck {n}")).clicked()
                {
                    remove = Some(i);
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let mut pct = (d.level * 100.0).round();
                let r = ui.add(
                    egui::DragValue::new(&mut pct)
                        .range(0.0..=99.0)
                        .speed(0.5)
                        .max_decimals(0)
                        .prefix("to ")
                        .suffix("%"),
                );
                let label = format!("Duck {n} level");
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, &label)
                });
                if r.changed() {
                    d.level = (pct / 100.0).clamp(0.0, 0.99);
                    changed = true;
                }
                for (what, v) in [("attack", &mut d.attack_ms), ("release", &mut d.release_ms)] {
                    let r = ui
                        .add(
                            egui::DragValue::new(v)
                                .range(0..=5000)
                                .speed(5.0)
                                .prefix(format!("{what} "))
                                .suffix(" ms"),
                        )
                        .on_hover_text(if what == "attack" {
                            "How long the target takes to fall"
                        } else {
                            "How long it takes to come back once the trigger is quiet"
                        });
                    let label = format!("Duck {n} {what}");
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, &label)
                    });
                    changed |= r.changed();
                }
            });
        });
        ui.add_space(4.0);
    }
    if let Some(i) = remove {
        app.mixer.file.duck.remove(i);
        changed = true;
    }
    let full = app.mixer.file.duck.len() >= MAX_DUCK_RULES;
    let r = ui
        .add_enabled(
            !full,
            egui::Button::new(format!("{}  Add duck rule", icon::PLUS))
                .min_size(Vec2::new(0.0, 24.0)),
        )
        .on_hover_text("While the trigger bus plays, the target falls to the level")
        .on_disabled_hover_text(format!("At most {MAX_DUCK_RULES} rules"));
    if r.clicked() {
        let (trigger, target) = free_pair(&app.mixer.file.duck);
        app.mixer.file.duck.push(DuckDecl {
            trigger: trigger.label().into(),
            target: target.label().into(),
            level: 0.4,
            attack_ms: 150,
            release_ms: 300,
            comment: String::new(),
        });
        changed = true;
    }
    if changed {
        app.mixer.dirty = true;
    }
    changed
}

/// The first (trigger, target) pair no rule has yet, the usual ones
/// first (voice over music, then sfx over music, ...): a new rule never
/// starts out as a duplicate the bake rejects.
fn free_pair(rules: &[DuckDecl]) -> (Bus, Bus) {
    let order = [Bus::Voice, Bus::Sfx, Bus::Ui, Bus::Ambience, Bus::Music];
    let taken = |t: Bus, g: Bus| {
        rules
            .iter()
            .any(|d| d.trigger == t.label() && d.target == g.label())
    };
    order
        .into_iter()
        .flat_map(|t| {
            [Bus::Music, Bus::Ambience, Bus::Sfx, Bus::Voice, Bus::Ui].map(move |g| (t, g))
        })
        .find(|&(t, g)| t != g && !taken(t, g))
        .unwrap_or((Bus::Voice, Bus::Music))
}

/// A bus label combo; its accessible name is `name`.
fn bus_combo(
    ui: &mut egui::Ui,
    id: (&str, usize),
    name: &str,
    value: &mut String,
    buses: &[Bus],
) -> bool {
    let mut changed = false;
    let r = egui::ComboBox::from_id_salt(id)
        .selected_text(value.as_str())
        .width(84.0)
        .truncate()
        .show_ui(ui, |ui| {
            for b in buses {
                if ui.selectable_label(value == b.label(), b.label()).clicked()
                    && value != b.label()
                {
                    *value = b.label().to_string();
                    changed = true;
                }
            }
        });
    let name = name.to_string();
    r.response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &name));
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_rule_takes_a_free_pair() {
        let rule = |t: Bus, g: Bus| DuckDecl {
            trigger: t.label().into(),
            target: g.label().into(),
            level: 0.5,
            attack_ms: 0,
            release_ms: 0,
            comment: String::new(),
        };
        assert_eq!(free_pair(&[]), (Bus::Voice, Bus::Music));
        let rules = [rule(Bus::Voice, Bus::Music)];
        assert_eq!(free_pair(&rules), (Bus::Voice, Bus::Ambience));
        let all: Vec<DuckDecl> = [Bus::Music, Bus::Ambience, Bus::Sfx, Bus::Ui]
            .map(|g| rule(Bus::Voice, g))
            .into();
        assert_eq!(free_pair(&all), (Bus::Sfx, Bus::Music));
    }
}
