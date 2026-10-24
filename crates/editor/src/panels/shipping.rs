//! The Project tab (issue #61): **Achievements** — the table of
//! `assets/achievements.ron` (id, name, hidden, stat threshold, icon), the
//! selected row's fields, stats and presence keys, what `services check`
//! finds, a preview of the engine's list and popup on the N64 / 3DS / PC
//! looks with a test unlock, and the buttons that freeze the bit order and
//! export Steam's files; **Packaging** — `trino.toml`'s packaging keys and
//! the buttons that run `cargo xtask package pc` and `deck check`. Every
//! field writes the same text the CLI does.

use eframe::egui::{self, RichText, Vec2};
use trino_scene::ui::Level;

use crate::app::App;
use crate::shipping::Page;
use crate::theme::{self, c, icon};

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (page, name, glyph) in [
                    (Page::Achievements, "Achievements", icon::TROPHY),
                    (Page::Packaging, "Packaging", icon::PACKAGE),
                ] {
                    if ui
                        .selectable_label(app.shipping.page == page, format!("{glyph} {name}"))
                        .clicked()
                    {
                        app.shipping.page = page;
                    }
                }
            });
            ui.separator();
            match app.shipping.page {
                Page::Achievements => achievements(app, ui),
                Page::Packaging => packaging(app, ui),
            }
        });
}

fn faint(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(text.into()).size(11.5).color(c::TEXT_FAINT)).wrap());
}

fn achievements(app: &mut App, ui: &mut egui::Ui) {
    let root = app.project.root.clone();
    // Status line.
    {
        let s = &app.shipping;
        let file = trino_scene::services::FILE;
        let (text, color) = if s.dirty {
            ("unsaved".to_string(), c::WARN)
        } else if s.exists {
            (file.to_string(), c::TEXT_FAINT)
        } else {
            (format!("no {file} yet: an edit creates it"), c::TEXT_FAINT)
        };
        let save =
            trino_core::AchievementState::packed_len(s.def.achievements.len(), s.def.stats.len());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(text).size(11.5).color(color));
            ui.label(
                RichText::new(format!(
                    "{} achievement(s), {} stat(s): {save} B in a console save",
                    s.def.achievements.len(),
                    s.def.stats.len()
                ))
                .size(11.5)
                .color(c::TEXT_WEAK),
            )
            .on_hover_text(
                "What `AchievementState::pack_for` writes into a game's save: 1 bit per \
                 achievement and 4 bytes per stat (the N64's EEPROM is 512 B or 2 KB)",
            );
            let locked = s.locked_rows();
            ui.label(
                RichText::new(if locked > 0 {
                    format!("{} {locked} bit(s) locked", icon::LOCK_SIMPLE)
                } else {
                    "order not locked".into()
                })
                .size(11.5)
                .color(c::TEXT_WEAK),
            );
        });
        if let Some(e) = &s.error {
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

    // The table.
    let names: Vec<(String, String)> = {
        let langs = trino_asset_pipeline::ui::langs(&root).unwrap_or_default();
        let lang = langs.default_lang().map(str::to_string).unwrap_or_default();
        app.shipping
            .def
            .achievements
            .iter()
            .map(|a| {
                let t = |k: &str| {
                    langs
                        .lookup(&lang, k)
                        .map_or_else(|| k.to_string(), |e| e.text().to_string())
                };
                (t(&a.name), t(&a.description))
            })
            .collect()
    };
    let locked = app.shipping.locked_rows();
    egui::Grid::new("achievements")
        .striped(true)
        .num_columns(5)
        .min_col_width(24.0)
        .show(ui, |ui| {
            for h in ["bit", "id", "name", "hidden", "unlocks at"] {
                ui.label(RichText::new(h).size(11.0).color(c::TEXT_FAINT));
            }
            ui.end_row();
            for (i, a) in app.shipping.def.achievements.iter().enumerate() {
                let lock = if i < locked {
                    format!("{} {i}", icon::LOCK_SIMPLE)
                } else {
                    i.to_string()
                };
                ui.label(RichText::new(lock).size(11.5).color(c::TEXT_WEAK));
                let picked = app.shipping.selected == Some(i);
                if ui
                    .add(
                        egui::Button::new(RichText::new(&a.id).size(12.0))
                            .selected(picked)
                            .frame(false),
                    )
                    .on_hover_text(&names[i].1)
                    .clicked()
                {
                    app.shipping.selected = Some(i);
                }
                theme::fixed_width(ui, 180.0, |ui| {
                    ui.add(egui::Label::new(RichText::new(&names[i].0).size(12.0)).truncate());
                });
                ui.label(if a.hidden { icon::EYE_SLASH } else { "" });
                ui.label(match &a.stat {
                    Some(s) => format!("{s} >= {}", a.threshold),
                    None => String::new(),
                });
                ui.end_row();
            }
        });
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(format!("{} Add achievement", icon::PLUS))
            .clicked()
        {
            let i = app.shipping.add_achievement();
            app.shipping.selected = Some(i);
        }
        if ui.button(format!("{} Add stat", icon::PLUS)).clicked() {
            app.shipping.add_stat();
        }
        if ui
            .button(format!("{} Lock order", icon::LOCK_SIMPLE))
            .on_hover_text(
                "Write assets/achievements.lock (cargo xtask services lock): do it before the \
                 first release; later the list may only grow at the end",
            )
            .clicked()
        {
            match trino_scene::services::write_lock(&root, &app.shipping.def) {
                Ok(p) => {
                    app.shipping.reload_lock(&root);
                    app.log.info(format!("wrote {}", p.display()));
                }
                Err(e) => app.log.error(e),
            }
        }
        if ui
            .button(format!("{} Export Steam files", icon::STEAM_LOGO))
            .on_hover_text(
                "dist/steam-config: the achievements CSV and localization VDF, rich presence, \
                 the Steam Input action manifest and 64 px icons (cargo xtask services export steam)",
            )
            .clicked()
        {
            let ship = &app.shipping.ship;
            let out = root.join(trino_asset_pipeline::services::STEAM_CONFIG_DIR);
            match trino_asset_pipeline::services::export_steam(
                &root,
                ship.steam_app_id,
                &ship.title,
                &out,
            ) {
                Ok(files) => app
                    .log
                    .info(format!("wrote {} file(s) in {}", files.len(), out.display())),
                Err(e) => app.log.error(e),
            }
        }
    });

    // The selected row.
    if let Some(i) = app
        .shipping
        .selected
        .filter(|&i| i < app.shipping.def.achievements.len())
    {
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!("{} Achievement {i}", icon::MEDAL))
                .family(theme::bold())
                .color(c::TEXT),
        );
        let sprites: Vec<String> = app
            .project
            .sprites
            .keys()
            .filter(|k| k.starts_with("sprites/"))
            .cloned()
            .collect();
        let stats: Vec<String> = app
            .shipping
            .def
            .stats
            .iter()
            .map(|s| s.id.clone())
            .collect();
        let mut changed = false;
        let a = &mut app.shipping.def.achievements[i];
        egui::Grid::new("achievement")
            .num_columns(2)
            .show(ui, |ui| {
                for (label, field) in [
                    ("id", &mut a.id),
                    ("name key", &mut a.name),
                    ("description key", &mut a.description),
                ] {
                    ui.label(label);
                    changed |= ui
                        .add(egui::TextEdit::singleline(field).desired_width(220.0))
                        .on_hover_text(label)
                        .changed();
                    ui.end_row();
                }
                ui.label("icon");
                let current = a.icon.clone().unwrap_or_else(|| "(medal)".into());
                egui::ComboBox::from_id_salt("achievement icon")
                    .selected_text(current)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(a.icon.is_none(), "(medal)").clicked() {
                            a.icon = None;
                            changed = true;
                        }
                        for s in &sprites {
                            if ui
                                .selectable_label(a.icon.as_deref() == Some(s), s)
                                .clicked()
                            {
                                a.icon = Some(s.clone());
                                changed = true;
                            }
                        }
                    });
                ui.end_row();
                ui.label("hidden");
                changed |= ui
                    .checkbox(&mut a.hidden, "Hidden until unlocked")
                    .changed();
                ui.end_row();
                ui.label("stat");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("achievement stat")
                        .selected_text(a.stat.clone().unwrap_or_else(|| "(none)".into()))
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(a.stat.is_none(), "(none)").clicked() {
                                a.stat = None;
                                a.threshold = 0;
                                changed = true;
                            }
                            for s in &stats {
                                if ui
                                    .selectable_label(a.stat.as_deref() == Some(s), s)
                                    .clicked()
                                {
                                    a.stat = Some(s.clone());
                                    changed = true;
                                }
                            }
                        });
                    if a.stat.is_some() {
                        changed |= ui
                            .add(egui::DragValue::new(&mut a.threshold).prefix(">= "))
                            .changed();
                    }
                });
                ui.end_row();
            });
        if changed {
            app.shipping.changed();
        }
        ui.horizontal(|ui| {
            if ui.button(format!("{} Remove", icon::TRASH)).clicked()
                && let Err(e) = app.shipping.remove_achievement(i)
            {
                app.log.toast(crate::log::Level::Warn, e);
            }
            if ui
                .button(format!("{} Move down", icon::ARROW_DOWN))
                .clicked()
                && let Err(e) = app.shipping.move_down(i)
            {
                app.log.toast(crate::log::Level::Warn, e);
            }
            if ui
                .button(format!("{} Test unlock", icon::TROPHY))
                .on_hover_text("Show this achievement's popup in the preview below")
                .clicked()
            {
                if let Some(u) = app.shipping.preview_unlocked.get_mut(i) {
                    *u = true;
                }
                app.shipping.toast.push(i);
            }
        });
    }

    // Stats and presence keys.
    ui.add_space(6.0);
    ui.label(RichText::new("Stats").family(theme::bold()).color(c::TEXT));
    let mut changed = false;
    egui::Grid::new("stats").num_columns(4).show(ui, |ui| {
        for s in app.shipping.def.stats.iter_mut() {
            changed |= ui
                .add_sized(
                    Vec2::new(140.0, 20.0),
                    egui::TextEdit::singleline(&mut s.id),
                )
                .on_hover_text("stat id")
                .changed();
            changed |= ui
                .add(egui::DragValue::new(&mut s.min).prefix("min "))
                .changed();
            changed |= ui
                .add(egui::DragValue::new(&mut s.max).prefix("max "))
                .changed();
            changed |= ui
                .checkbox(&mut s.increment_only, "increment only")
                .changed();
            ui.end_row();
        }
    });
    ui.label(
        RichText::new("Presence keys")
            .family(theme::bold())
            .color(c::TEXT),
    );
    let mut remove = None;
    for (k, key) in app.shipping.def.presence.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= ui
                .add(egui::TextEdit::singleline(key).desired_width(200.0))
                .changed();
            if ui
                .small_button(icon::TRASH)
                .on_hover_text("Remove the key")
                .clicked()
            {
                remove = Some(k);
            }
        });
    }
    if let Some(k) = remove {
        app.shipping.def.presence.remove(k);
        changed = true;
    }
    if ui
        .button(format!("{} Add presence key", icon::PLUS))
        .clicked()
    {
        app.shipping.def.presence.push("presence.new".into());
        changed = true;
    }
    if changed {
        app.shipping.changed();
    }

    // What `services check` says.
    ui.add_space(6.0);
    let sprites: std::collections::BTreeSet<String> = app.project.sprites.keys().cloned().collect();
    app.shipping.check(&root, &|s| sprites.contains(s));
    let (errors, warnings) = app
        .shipping
        .findings
        .iter()
        .fold((0, 0), |(e, w), f| match f.level {
            Level::Error => (e + 1, w),
            Level::Warning => (e, w + 1),
        });
    ui.label(
        RichText::new(if errors + warnings == 0 {
            format!("{} services check: all good", icon::CHECK_CIRCLE)
        } else {
            format!("services check: {errors} error(s), {warnings} warning(s)")
        })
        .color(if errors > 0 { c::ERROR } else { c::TEXT_WEAK }),
    );
    for f in &app.shipping.findings {
        let color = match f.level {
            Level::Error => c::ERROR,
            Level::Warning => c::WARN,
        };
        ui.add(egui::Label::new(RichText::new(f.to_string()).size(11.5).color(color)).wrap());
    }

    // The overlay preview: the engine's list and popup.
    ui.add_space(6.0);
    preview(app, ui);
}

fn preview(app: &mut App, ui: &mut egui::Ui) {
    let w = ui.available_width().clamp(160.0, 480.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, w * 0.75), egui::Sense::hover());
    let blob = match app.shipping.def.to_tach() {
        Ok(b) => b,
        Err(_) => return,
    };
    let Ok(table) = trino_core::AchievementTable::parse(&blob) else {
        return;
    };
    let langs = trino_asset_pipeline::ui::langs(&app.project.root).unwrap_or_default();
    let lang = langs.default_lang().map(str::to_string).unwrap_or_default();
    let strings = langs.strings(&lang);
    let state = app.shipping.preview_state();
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    app.shipping.toast.advance(dt);
    if app.shipping.toast.current().is_some() {
        ui.ctx().request_repaint();
    }
    let rs = app.render_state.clone();
    let p = crate::preview::Preview::ensure(
        &mut app.shipping_preview,
        &rs,
        trino_platform_pc::SimProfile::N64,
    );
    let key = ("shipping".to_string(), app.project.generation);
    if p.uploaded.as_ref() != Some(&key) {
        app.project.upload_all(&mut p.renderer);
        p.uploaded = Some(key);
    }
    let toast = app.shipping.toast;
    let mut list = trino_core::AchievementList::new();
    list.focus = app.shipping.selected.unwrap_or(0);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    p.show(
        &mut child,
        rect,
        false,
        |r: &mut trino_platform_pc::PcRenderer, _| {
            let style = trino_core::OverlayStyle::DEFAULT;
            list.draw(r, &strings, &table, &state, &style);
            toast.draw(r, &strings, &table, &style);
        },
    );
    faint(
        ui,
        "The engine's list and popup as the N64 draws them (the game's strings, its \
         default language). Test unlock shows a popup here; a game run unlocks for real.",
    );
}

fn packaging(app: &mut App, ui: &mut egui::Ui) {
    let root = app.project.root.clone();
    if !trino_asset_pipeline::ship::path(&root).is_file() {
        faint(
            ui,
            "No trino.toml: packaging is for projects (cargo xtask new <name> --standalone). \
             The engine's own apps/pc is a development host.",
        );
    }
    if let Some(e) = &app.shipping.ship_error {
        ui.add(
            egui::Label::new(RichText::new(format!("{} {e}", icon::WARNING)).color(c::ERROR))
                .wrap(),
        );
    }
    let mut writes: Vec<(&'static str, String)> = Vec::new();
    {
        let s = &mut app.shipping.ship_edit;
        let features = s.features.join(", ");
        let mut features_edit = features.clone();
        let mut app_id = s.steam_app_id.map(|v| v.to_string()).unwrap_or_default();
        let app_id_was = app_id.clone();
        egui::Grid::new("packaging").num_columns(2).show(ui, |ui| {
            let mut field =
                |ui: &mut egui::Ui, label: &str, key: &'static str, v: &mut String, hint: &str| {
                    ui.label(label);
                    let r = ui.add(
                        egui::TextEdit::singleline(v)
                            .hint_text(hint)
                            .desired_width(280.0),
                    );
                    let r = r.on_hover_text(key);
                    if r.lost_focus() {
                        writes.push((key, v.clone()));
                    }
                    ui.end_row();
                };
            field(
                ui,
                "title",
                "project.title",
                &mut s.title,
                "the game's name",
            );
            field(
                ui,
                "icon",
                "pc.icon",
                &mut s.icon,
                "assets/platform/pc/icon.png",
            );
            field(
                ui,
                "identifier",
                "pc.identifier",
                &mut s.identifier,
                "com.example.game",
            );
            field(
                ui,
                "publisher",
                "pc.publisher",
                &mut s.publisher,
                "[project] author",
            );
            field(
                ui,
                "version",
                "pc.version",
                &mut s.version,
                "the pc crate's",
            );
            field(ui, "features", "pc.features", &mut features_edit, "gamepad");
            field(ui, "profile", "pc.profile", &mut s.profile, "native");
            field(
                ui,
                "crash upload",
                "pc.crash_upload",
                &mut s.crash_upload,
                "https://… (players opt in)",
            );
            field(
                ui,
                "Steam app id",
                "steam.app_id",
                &mut app_id,
                "480 = Spacewar",
            );
            field(
                ui,
                "itch target",
                "itch.target",
                &mut s.itch_target,
                "user/game",
            );
        });
        if features_edit != features {
            s.features = features_edit
                .split(',')
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty())
                .collect();
        }
        if app_id != app_id_was {
            s.steam_app_id = app_id.parse().ok();
        }
    }
    for (key, value) in writes {
        let current = {
            let s = &app.shipping.ship;
            match key {
                "project.title" => s.title.clone(),
                "pc.icon" => s.icon.clone(),
                "pc.identifier" => s.identifier.clone(),
                "pc.publisher" => s.publisher.clone(),
                "pc.version" => s.version.clone(),
                "pc.features" => s.features.join(", "),
                "pc.profile" => s.profile.clone(),
                "pc.crash_upload" => s.crash_upload.clone(),
                "steam.app_id" => s.steam_app_id.map(|v| v.to_string()).unwrap_or_default(),
                _ => s.itch_target.clone(),
            }
        };
        if current == value {
            continue;
        }
        match app.write_ship_key(key, &value) {
            Ok(()) => app.log.info(format!("trino.toml: {key} = {value}")),
            Err(e) => {
                app.log.toast(crate::log::Level::Error, e);
                app.shipping.reload_ship(&root);
            }
        }
    }
    ui.add_space(6.0);
    let running = app.shipping.job.is_some();
    ui.horizontal_wrapped(|ui| {
        let os = std::env::consts::OS;
        if ui
            .add_enabled(
                !running,
                egui::Button::new(format!("{} Package for {os}", icon::PACKAGE)),
            )
            .on_hover_text("cargo xtask package pc --project <this project> --smoke")
            .clicked()
        {
            app.run_xtask_job(&["package", "pc", "--smoke", "--itch"], "package");
        }
        if ui
            .add_enabled(
                !running,
                egui::Button::new(format!("{} Deck check", icon::GAME_CONTROLLER)),
            )
            .on_hover_text("cargo xtask deck check: the Steam Deck Verified checklist")
            .clicked()
        {
            app.run_xtask_job(&["deck", "check"], "deck check");
        }
        if running {
            ui.spinner();
        }
    });
    if let Some(l) = &app.shipping.last_job {
        faint(ui, l.clone());
    }
    faint(
        ui,
        "Packages go to dist/: an AppImage on Linux, a zip with the .exe's icon and version on \
         Windows, an .app (and a .dmg) on macOS, each run once outside the project to check \
         it starts. Signing happens when its secrets are set (docs/SHIPPING.md).",
    );
}
