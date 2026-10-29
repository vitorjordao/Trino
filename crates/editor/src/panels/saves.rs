//! The Saves tab (issue #68): the project's saves decoded read-only, as the
//! engine reads them — which slot is the newest, each slot's sequence
//! number and check, the version tag, the first bytes. The same report as
//! `cargo xtask save inspect <file> --json` (`trino_asset_pipeline::saves`).
//!
//! Three sources (issue #121): the PC's (`target/save/`, what Play and
//! `cargo xtask run pc` write), the N64 cartridge's EEPROM as ares keeps it
//! beside the ROM (`target/n64/<rom>.eeprom`), and the 3DS SD card of
//! Azahar (`sdmc:/3ds/trino/<game>/`, and the folder every game shared
//! before #119, which a game's empty folder reads once).

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use trino_asset_pipeline::saves::{self, Report};
use trino_core::SaveStatus;

use crate::app::App;
use crate::theme::{self, c, icon};

/// Where saves come from.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Source {
    /// `target/save/` (the PC's slot files).
    #[default]
    Pc,
    /// `target/n64/*.eeprom` (ares's cartridge EEPROM images).
    N64,
    /// Azahar's SD card (`sdmc/3ds/trino/…`).
    N3ds,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::Pc, Source::N64, Source::N3ds];

    pub fn label(self) -> &'static str {
        match self {
            Source::Pc => "PC",
            Source::N64 => "N64 (ares)",
            Source::N3ds => "3DS (Azahar)",
        }
    }

    /// Where this source looks for a project at `root` (`None`: Azahar's SD
    /// card isn't there).
    pub fn dir(self, root: &Path) -> Option<PathBuf> {
        match self {
            Source::Pc => Some(root.join("target/save")),
            Source::N64 => Some(root.join("target/n64")),
            Source::N3ds => saves::azahar_sd(),
        }
    }

    /// The saves this source holds under `dir`.
    pub fn scan(self, dir: &Path) -> Vec<PathBuf> {
        match self {
            Source::Pc => SavesPanel::scan(dir),
            Source::N64 => saves::ares_eeproms(dir),
            Source::N3ds => saves::n3ds_folders(dir),
        }
    }
}

/// The tab's state: the saves found and the one shown.
#[derive(Default)]
pub struct SavesPanel {
    pub source: Source,
    /// The folder [`source`](Self::source) scanned (`None`: not found).
    pub dir: Option<PathBuf>,
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

    /// Scan the current source for the project at `root`.
    pub fn refresh(&mut self, root: &Path) {
        self.dir = self.source.dir(root);
        self.files = self
            .dir
            .as_deref()
            .map_or_else(Vec::new, |d| self.source.scan(d));
        self.scanned = true;
        if self.selected.is_none_or(|i| i >= self.files.len()) {
            self.selected = (!self.files.is_empty()).then_some(0);
        }
        self.inspect();
    }

    pub fn inspect(&mut self) {
        let medium = self.source == Source::N64;
        self.report = self
            .selected
            .and_then(|i| self.files.get(i))
            .map(|p| saves::inspect(p, medium));
    }

    /// Show `source` (the tab's buttons; `TRINO_EDITOR_SAVES=pc|n64|3ds`).
    pub fn show(&mut self, source: Source, root: &Path) {
        if self.source != source {
            self.source = source;
            self.selected = None;
        }
        self.refresh(root);
    }

    /// A list row's name: the file, or for a 3DS folder its game (the
    /// shared folder from before #119 says so).
    fn row_name(&self, p: &Path) -> String {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.source == Source::N3ds && p.ends_with("3ds/trino") {
            "3ds/trino (shared, before #119)".into()
        } else {
            name
        }
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
    let root = app.project.root.clone();
    let panel = &mut app.saves;
    if !panel.scanned {
        if let Ok(s) = std::env::var("TRINO_EDITOR_SAVES") {
            panel.source = match s.as_str() {
                "n64" => Source::N64,
                "3ds" => Source::N3ds,
                _ => Source::Pc,
            };
        }
        panel.refresh(&root);
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
                    panel.refresh(&root);
                }
            });
            ui.horizontal(|ui| {
                let mut pick = None;
                for s in Source::ALL {
                    if ui.selectable_label(panel.source == s, s.label()).clicked() {
                        pick = Some(s);
                    }
                }
                if let Some(s) = pick {
                    panel.show(s, &root);
                }
            });
            let where_ = panel
                .dir
                .as_ref()
                .map_or("(not found)".to_string(), |d| d.display().to_string());
            let what = match panel.source {
                Source::Pc => "the PC saves",
                Source::N64 => "the N64 EEPROM images ares keeps beside the ROMs",
                Source::N3ds => "the 3DS save folders on Azahar's SD card",
            };
            ui.label(
                RichText::new(format!(
                    "Read-only: {what} in {where_} as the engine reads them (A/B slots, \
                     newest intact first). Damage one by hand with `cargo xtask save corrupt`."
                ))
                .size(11.5)
                .color(c::TEXT_WEAK),
            );
            ui.add_space(6.0);
            if panel.files.is_empty() {
                let hint = match panel.source {
                    Source::Pc => "No saves yet: Play the game and save once.",
                    Source::N64 => "No EEPROM yet: run the game in ares (cargo xtask run n64).",
                    Source::N3ds => {
                        "No save folders: run the game in Azahar (cargo xtask run 3ds), \
                         or set TRINO_AZAHAR_SDMC to its SD card."
                    }
                };
                ui.label(RichText::new(hint).color(c::TEXT_WEAK));
                return;
            }
            let mut pick = None;
            crate::app::card().show(ui, |ui| {
                for (i, f) in panel.files.iter().enumerate() {
                    let name = panel.row_name(f);
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
                    "valid" | "shared" => c::OK,
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
