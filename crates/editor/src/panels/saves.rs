//! The Saves tab (issue #68): the project's PC saves (`target/save/`, what
//! Play and `cargo xtask run pc` write) decoded read-only, as the engine
//! reads them — which slot is the newest, each slot's sequence number and
//! check, the version tag, the first bytes. The same report as `cargo xtask
//! save inspect <file> --json` (`trino_asset_pipeline::saves`).

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use trino_asset_pipeline::saves::{self, Report};
use trino_core::SaveStatus;

use crate::app::App;
use crate::theme::{self, c, icon};

/// The tab's state: the saves found and the one shown.
#[derive(Default)]
pub struct SavesPanel {
    pub files: Vec<PathBuf>,
    pub selected: Option<usize>,
    pub report: Option<Result<Report, String>>,
    scanned: bool,
}

impl SavesPanel {
    /// The saves under `dir`: a slot pair (`x.a`, `x.b`) and a lone save
    /// file each count once, by their base path.
    pub fn scan(dir: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|r| {
                r.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_file())
                    .filter_map(|p| {
                        let name = p.file_name()?.to_str()?.to_string();
                        if let Some(base) =
                            name.strip_suffix(".a").or_else(|| name.strip_suffix(".b"))
                        {
                            Some(p.with_file_name(base))
                        } else if name.ends_with(".sav")
                            || name.ends_with(".eeprom")
                            || name.ends_with(".eep")
                        {
                            Some(p)
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out.dedup();
        out
    }

    pub fn refresh(&mut self, dir: &Path) {
        self.files = Self::scan(dir);
        self.scanned = true;
        if self.selected.is_none_or(|i| i >= self.files.len()) {
            self.selected = (!self.files.is_empty()).then_some(0);
        }
        self.inspect();
    }

    pub fn inspect(&mut self) {
        self.report = self
            .selected
            .and_then(|i| self.files.get(i))
            .map(|p| saves::inspect(p, false));
    }
}

fn status_color(s: SaveStatus) -> egui::Color32 {
    match s {
        SaveStatus::Ok => c::OK,
        SaveStatus::RecoveredFromBackup => c::WARN,
        SaveStatus::Corrupted => c::ERROR,
        _ => c::TEXT_WEAK,
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let dir = app.project.root.join("target/save");
    let panel = &mut app.saves;
    if !panel.scanned {
        panel.refresh(&dir);
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}  Saves", icon::FLOPPY_DISK))
                        .family(theme::bold())
                        .color(c::TEXT),
                );
                if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Scan again").clicked() {
                    panel.refresh(&dir);
                }
            });
            ui.label(
                RichText::new(format!(
                    "Read-only: the PC saves in {} as the engine reads them (A/B slots, \
                     newest intact first). Damage one by hand with `cargo xtask save corrupt`.",
                    dir.display()
                ))
                .size(11.5)
                .color(c::TEXT_WEAK),
            );
            ui.add_space(6.0);
            if panel.files.is_empty() {
                ui.label(
                    RichText::new("No saves yet: Play the game and save once.").color(c::TEXT_WEAK),
                );
                return;
            }
            let mut pick = None;
            crate::app::card().show(ui, |ui| {
                for (i, f) in panel.files.iter().enumerate() {
                    let name = f
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let on = panel.selected == Some(i);
                    if ui
                        .add(
                            egui::Button::new(RichText::new(&name).color(if on {
                                c::ACCENT_TEXT
                            } else {
                                c::TEXT
                            }))
                            .frame(false),
                        )
                        .clicked()
                    {
                        pick = Some(i);
                    }
                }
            });
            if let Some(i) = pick {
                panel.selected = Some(i);
                panel.inspect();
            }
            ui.add_space(6.0);
            match &panel.report {
                Some(Ok(r)) => report(ui, r),
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(c::ERROR));
                }
                None => {}
            }
        });
}

fn report(ui: &mut egui::Ui, r: &Report) {
    crate::app::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Status").family(theme::bold()));
            ui.label(RichText::new(r.status.label()).color(status_color(r.status)));
            ui.label(RichText::new(format!("({})", r.kind)).color(c::TEXT_FAINT));
        });
        match &r.newest {
            Some(n) => ui.label(format!("Reads {} bytes from {n}", r.len)),
            None => ui.label(
                RichText::new("Reads nothing: the game starts from its defaults")
                    .color(c::TEXT_WEAK),
            ),
        };
        if let Some(v) = r.version {
            ui.label(format!("Version tag {v}"));
        }
        if !r.preview.is_empty() {
            ui.label(RichText::new(&r.preview).monospace().color(c::TEXT_WEAK));
        }
        ui.add_space(4.0);
        egui::Grid::new("save-slots").striped(true).show(ui, |ui| {
            for h in ["Slot", "State", "Bytes", "Seq"] {
                ui.label(RichText::new(h).color(c::TEXT_FAINT));
            }
            ui.end_row();
            for s in &r.slots {
                ui.label(&s.name);
                let col = match s.state {
                    "valid" => c::OK,
                    "damaged" => c::ERROR,
                    _ => c::TEXT_WEAK,
                };
                ui.label(RichText::new(s.state).color(col));
                ui.label(s.bytes.to_string());
                ui.label(s.seq.map_or("-".into(), |q| q.to_string()));
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("cargo xtask save inspect {} --json", r.file))
                .monospace()
                .size(11.0)
                .color(c::TEXT_FAINT),
        );
    });
}
