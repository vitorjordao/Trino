//! The Net tab (issue #67): local players and network sessions —
//! `trino.toml`'s `[players]` and `[net]`, a simulated network, two local
//! instances of the game online (`cargo xtask net local`) and the lobby
//! (`cargo xtask net lobby`). Every action shows the command it runs.

use eframe::egui::{self, RichText};
use trino_net::NetMode;

use crate::app::App;
use crate::theme::{self, c, icon};

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let root = app.project.root.clone();
    let net = &mut app.net;
    // A finished instance pair stays readable; a lobby that died is gone.
    if let Some(l) = &mut net.lobby
        && !l.running()
    {
        net.lobby = None;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Rows of a label, a slider and its value fit a narrow pane.
            ui.spacing_mut().slider_width = (ui.available_width() - 230.0).clamp(60.0, 200.0);
            ui.label(
                RichText::new(format!("{}  Players and network play", icon::WIFI_HIGH))
                    .family(theme::bold())
                    .color(c::TEXT),
            );
            ui.label(
                RichText::new(
                    "Games read every player from Platform::players. Online, the whole \
                     session runs under rollback: remote players sit in their seats.",
                )
                .size(11.5)
                .color(c::TEXT_WEAK),
            );
            ui.add_space(6.0);

            crate::app::card().show(ui, |ui| {
                ui.label(RichText::new("Session").family(theme::bold()));
                row(
                    ui,
                    "Local players",
                    egui::Slider::new(&mut net.max_players, 1..=4),
                );
                ui.horizontal(|ui| {
                    ui.label("Mode");
                    ui.radio_value(&mut net.cfg.mode, NetMode::Rollback, "Rollback")
                        .on_hover_text("Predict late inputs, roll back on a miss (GGPO)");
                    ui.radio_value(&mut net.cfg.mode, NetMode::Delay, "Delay")
                        .on_hover_text("Lockstep: wait for every input (no snapshots)");
                });
                ui.add_enabled_ui(!net.mode_is_delay(), |ui| {
                    row(
                        ui,
                        "Rollback window",
                        egui::Slider::new(&mut net.cfg.max_rollback, 1..=16).suffix(" frames"),
                    );
                });
                row(
                    ui,
                    "Input delay",
                    egui::Slider::new(&mut net.cfg.input_delay, 0..=10).suffix(" frames"),
                );
                ui.horizontal(|ui| {
                    let save = ui.add_enabled(
                        net.has_toml,
                        egui::Button::new(format!("{}  Save to trino.toml", icon::FLOPPY_DISK)),
                    );
                    if save
                        .on_hover_text("cargo xtask net config set players.max=… net.mode=…")
                        .on_disabled_hover_text(
                            "This project has no trino.toml: the instances take these \
                             settings for the run (TRINO_NET_*)",
                        )
                        .clicked()
                    {
                        net.status = Some(match net.save(&root) {
                            Ok(()) => "saved [players] and [net] to trino.toml".into(),
                            Err(e) => e,
                        });
                    }
                    if ui
                        .button(format!("{}  Read again", icon::ARROW_CLOCKWISE))
                        .on_hover_text("Read trino.toml again (an edit on disk)")
                        .clicked()
                    {
                        net.reload(&root);
                    }
                });
            });
            ui.add_space(6.0);

            crate::app::card().show(ui, |ui| {
                ui.label(RichText::new("Simulated network (each way)").family(theme::bold()));
                row(
                    ui,
                    "Latency",
                    egui::Slider::new(&mut net.latency_ms, 0..=300).suffix(" ms"),
                );
                row(
                    ui,
                    "Jitter",
                    egui::Slider::new(&mut net.jitter_ms, 0..=50).suffix(" ms"),
                );
                row(
                    ui,
                    "Loss",
                    egui::Slider::new(&mut net.loss_percent, 0.0..=20.0).suffix(" %"),
                );
            });
            ui.add_space(6.0);

            crate::app::card().show(ui, |ui| {
                ui.label(RichText::new("Two local instances").family(theme::bold()));
                ui.horizontal(|ui| {
                    ui.label("Game");
                    ui.add(egui::TextEdit::singleline(&mut net.game).desired_width(120.0));
                    ui.checkbox(&mut net.scripted, "Scripted check")
                        .on_hover_text(
                            "Each instance plays its seat of the two-player recording and \
                             both print their hash on frame 760: they must match",
                        );
                });
                let running = net.local.as_mut().is_some_and(|j| j.running());
                ui.horizontal(|ui| {
                    if running {
                        if ui
                            .button(RichText::new(format!("{}  Stop", icon::STOP)).color(c::ERROR))
                            .clicked()
                            && let Some(j) = &mut net.local
                        {
                            j.stop();
                        }
                    } else if ui
                        .button(
                            RichText::new(format!("{}  Start host + guest", icon::PLAY))
                                .color(c::PLAY),
                        )
                        .clicked()
                    {
                        net.status = net.start_local(&root).err();
                    }
                    let lobby = net.lobby.is_some();
                    let label = if lobby { "Stop lobby" } else { "Start lobby" };
                    if ui.button(format!("{}  {label}", icon::BROADCAST)).clicked() {
                        if lobby {
                            net.lobby = None;
                        } else {
                            net.status = net.start_lobby().err();
                        }
                    }
                });
                let cmd = format!("cargo xtask {}", net.local_args(&root).join(" "));
                ui.add(
                    egui::Label::new(
                        RichText::new(cmd)
                            .monospace()
                            .size(11.0)
                            .color(c::TEXT_FAINT),
                    )
                    .truncate(),
                );
                if let Some(s) = &net.status {
                    ui.label(RichText::new(s).color(c::WARN));
                }
                if let Some(l) = &net.lobby {
                    for line in l.lines().iter().filter(|l| l.starts_with("[net]")).take(2) {
                        ui.label(
                            RichText::new(line)
                                .monospace()
                                .size(11.0)
                                .color(c::TEXT_WEAK),
                        );
                    }
                }
                let lines = net.net_lines();
                if lines.is_empty() && !running {
                    ui.label(
                        RichText::new(
                            "Not running. Start opens two windows of the game, a host and a guest.",
                        )
                        .size(11.5)
                        .color(c::TEXT_WEAK),
                    );
                }
                for line in lines.iter().rev().take(8).rev() {
                    let color = if line.contains("Desynced") || line.contains("differ") {
                        c::ERROR
                    } else if line.contains("TRINO_NET_HASH") || line.contains("same game") {
                        c::PLAY
                    } else {
                        c::TEXT_WEAK
                    };
                    ui.add(
                        egui::Label::new(RichText::new(line).monospace().size(11.0).color(color))
                            .truncate(),
                    );
                }
            });
        });
    if net.local.as_mut().is_some_and(|j| j.running()) || net.lobby.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// A labeled slider row; the slider is named "<label> value" for screen
/// readers and the UI tests.
fn row(ui: &mut egui::Ui, label: &str, slider: egui::Slider<'_>) {
    ui.horizontal(|ui| {
        ui.label(label);
        let r = ui.add(slider);
        crate::panels::audio::name_slider(&r, &format!("{label} value"));
    });
}
