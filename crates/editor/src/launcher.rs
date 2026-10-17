//! **Project launcher**: what the editor shows when it starts without a
//! project — recent projects, open an existing one, or create a new one
//! from a template (blank, or a copy of an engine demo).
//!
//! Creating runs the same scaffolder as `cargo xtask new --standalone`
//! (`trino_scaffold`), so both make identical projects. Templates come from
//! an engine checkout: `$TRINO_ENGINE`, else the checkout this editor was
//! built from.
//!
//! The editor resolves everything relative to the working directory, so
//! opening a project changes into it (`main.rs`) before the editor starts.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use trino_scaffold::{EngineSource, Names, Template};

use crate::recent::{self, RecentProjects};
use crate::theme::{self, c, icon};

/// The column's widest size; wider windows center it.
const MAX_WIDTH: f32 = 1040.0;
/// From this column width on, recent projects sit beside open / create.
const TWO_COLUMNS: f32 = 860.0;

pub struct Launcher {
    pub recent: RecentProjects,
    /// Where `recent` is saved (`None`: no config dir, nothing persists).
    pub recent_file: Option<PathBuf>,
    /// The engine checkout templates render from.
    pub engine: Option<PathBuf>,
    /// "Open existing": the folder typed in.
    pub open_path: String,
    pub open_error: Option<String>,
    /// "Create new project".
    pub name: String,
    pub folder: String,
    pub template: Template,
    pub create_error: Option<String>,
    /// Seconds since the epoch, for the "last opened" ages.
    pub now: u64,
}

/// The engine checkout to scaffold from: `$TRINO_ENGINE`, else the one this
/// binary was built in (if it is still there).
pub fn engine_checkout() -> Option<PathBuf> {
    let built = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::var_os("TRINO_ENGINE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .chain([built])
        .filter_map(|p| trino_scaffold::absolute(&p).ok())
        .find(|p| trino_scaffold::is_engine_checkout(p))
}

/// `dir` as a project the editor can open (absolute): a folder with a
/// `trino.toml` (a game project) or an `assets/manifest.toml` (the engine's
/// own content).
pub fn check_project(dir: &Path) -> Result<PathBuf, String> {
    if dir.as_os_str().is_empty() {
        return Err("type the folder of a project".into());
    }
    let abs = trino_scaffold::absolute(dir)?;
    if !abs.exists() {
        return Err(format!("{} does not exist", abs.display()));
    }
    if !abs.is_dir() {
        return Err(format!("{} is not a folder", abs.display()));
    }
    if !abs.join("trino.toml").is_file() && !abs.join("assets/manifest.toml").is_file() {
        return Err(format!(
            "{} is not a Trino project: it has no trino.toml (nor assets/manifest.toml)",
            abs.display()
        ));
    }
    Ok(abs)
}

impl Launcher {
    pub fn new(recent_file: Option<PathBuf>, engine: Option<PathBuf>) -> Self {
        let recent = recent_file
            .as_deref()
            .map(RecentProjects::load)
            .unwrap_or_default();
        // New projects go next to the engine checkout by default, not
        // inside it.
        let folder = engine
            .as_ref()
            .and_then(|e| e.parent().map(Path::to_path_buf))
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Launcher {
            recent,
            recent_file,
            engine,
            open_path: String::new(),
            open_error: None,
            name: "my-game".into(),
            folder,
            template: Template::Blank,
            create_error: None,
            now: recent::now(),
        }
    }

    /// The launcher with the default recent file and engine checkout.
    pub fn from_env() -> Self {
        Self::new(recent::default_file(), engine_checkout())
    }

    /// Where "Create" puts the project: `<folder>/<name>`.
    pub fn destination(&self) -> PathBuf {
        Path::new(self.folder.trim()).join(self.name.trim())
    }

    /// Scaffold the project the form describes; its folder on success.
    pub fn create(&self) -> Result<PathBuf, String> {
        let engine = self
            .engine
            .as_ref()
            .ok_or("project templates need a Trino engine checkout: set TRINO_ENGINE to one")?;
        let names = Names::new(self.name.trim())?;
        if self.folder.trim().is_empty() {
            return Err("choose the folder the project goes in".into());
        }
        let dest = trino_scaffold::absolute(&self.destination())?;
        trino_scaffold::scaffold(
            engine,
            &dest,
            &names,
            &EngineSource::Path(engine.clone()),
            self.template,
        )?;
        Ok(dest)
    }

    /// Forget a recent project (the folder itself is untouched).
    fn forget(&mut self, path: &Path) {
        self.recent.remove(path);
        if let Some(file) = &self.recent_file
            && let Err(e) = self.recent.save(file)
        {
            self.open_error = Some(e);
        }
    }

    /// Draw the launcher; the project to open, when one is picked.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        let mut open = None;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(c::BG0)
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let width = ui.available_width().min(MAX_WIDTH);
                        let pad = ((ui.available_width() - width) / 2.0).max(0.0);
                        ui.horizontal_top(|ui| {
                            ui.add_space(pad);
                            ui.vertical(|ui| {
                                ui.set_width(width);
                                open = self.body(ui, width);
                            });
                        });
                    });
            });
        open
    }

    fn body(&mut self, ui: &mut egui::Ui, width: f32) -> Option<PathBuf> {
        let mut open = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::CUBE).size(26.0).color(c::ACCENT));
            ui.label(
                RichText::new("Trino")
                    .size(26.0)
                    .family(theme::bold())
                    .color(c::TEXT),
            );
        });
        ui.label(
            RichText::new("Open a project, or start a new one for PC, N64 and 3DS.")
                .color(c::TEXT_WEAK),
        );
        ui.add_space(14.0);
        if width >= TWO_COLUMNS {
            ui.columns(2, |cols| {
                if let Some(p) = self.recent_card(&mut cols[0]) {
                    open = Some(p);
                }
                if let Some(p) = self.open_card(&mut cols[1]) {
                    open = Some(p);
                }
                cols[1].add_space(10.0);
                if let Some(p) = self.create_card(&mut cols[1]) {
                    open = Some(p);
                }
            });
        } else {
            for card in [Self::recent_card, Self::open_card, Self::create_card] {
                if let Some(p) = card(self, ui) {
                    open = Some(p);
                }
                ui.add_space(10.0);
            }
        }
        open
    }

    fn recent_card(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        let mut open = None;
        let mut forget = None;
        crate::app::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_title(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Recent projects");
            ui.add_space(4.0);
            if self.recent.entries.is_empty() {
                ui.label(
                    RichText::new("No recent projects yet: open or create one.")
                        .color(c::TEXT_FAINT),
                );
            }
            for (i, entry) in self.recent.entries.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let path = entry.path.display().to_string();
                let name = entry
                    .path
                    .file_name()
                    .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                let exists = entry.path.is_dir();
                ui.horizontal(|ui| {
                    // Room for the two buttons on the right.
                    let text_w = (ui.available_width() - 96.0).max(40.0);
                    ui.vertical(|ui| {
                        ui.set_width(text_w);
                        ui.add(
                            egui::Label::new(
                                RichText::new(&name).family(theme::bold()).color(c::TEXT),
                            )
                            .truncate(),
                        );
                        let chars = (text_w / 6.6) as usize;
                        ui.add(
                            egui::Label::new(
                                RichText::new(crate::app::elide_start(&path, chars))
                                    .size(11.5)
                                    .color(c::TEXT_WEAK),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&path);
                        let when = if exists {
                            format!("opened {}", recent::age(entry.opened, self.now))
                        } else {
                            "folder not found".into()
                        };
                        ui.label(RichText::new(when).size(11.0).color(if exists {
                            c::TEXT_FAINT
                        } else {
                            c::WARN
                        }));
                    });
                    let label = format!("Open {name}");
                    let b = ui
                        .add_enabled(exists, egui::Button::new("Open"))
                        .on_hover_text(&path);
                    b.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, exists, &label)
                    });
                    if b.clicked() {
                        open = Some(entry.path.clone());
                    }
                    if theme::named_icon_button(ui, icon::X, &format!("Forget {name}")).clicked() {
                        forget = Some(entry.path.clone());
                    }
                });
            }
        });
        if let Some(p) = forget {
            self.forget(&p);
        }
        open
    }

    fn open_card(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        let mut open = None;
        crate::app::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_title(ui, icon::FOLDER_OPEN, "Open existing");
            ui.add_space(4.0);
            ui.label(
                RichText::new("A project folder: it holds a trino.toml.")
                    .size(12.0)
                    .color(c::TEXT_WEAK),
            );
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.open_path)
                        .hint_text("/path/to/my-game")
                        .desired_width((ui.available_width() - 70.0).max(60.0)),
                );
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Open").clicked() || enter {
                    match check_project(Path::new(self.open_path.trim())) {
                        Ok(p) => open = Some(p),
                        Err(e) => self.open_error = Some(e),
                    }
                }
            });
            if let Some(e) = &self.open_error {
                ui.add(egui::Label::new(RichText::new(e).size(12.0).color(c::ERROR)).wrap());
            }
            if let Some(engine) = self.engine.clone() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{}  Engine demo scenes", icon::GAME_CONTROLLER))
                        .on_hover_text(engine.display().to_string())
                        .clicked()
                    {
                        open = Some(engine.clone());
                    }
                    ui.add(
                        egui::Label::new(
                            RichText::new("the engine's own scenes")
                                .size(11.5)
                                .color(c::TEXT_FAINT),
                        )
                        .truncate(),
                    );
                });
            }
        });
        open
    }

    fn create_card(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        let mut open = None;
        crate::app::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_title(ui, icon::FILE_PLUS, "Create new project");
            ui.add_space(4.0);
            theme::prop(ui, "Name", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .hint_text("my-game")
                        .desired_width(ui.available_width()),
                )
            });
            theme::prop(ui, "Folder", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.folder)
                        .hint_text("/path/to/games")
                        .desired_width(ui.available_width()),
                )
            });
            ui.add_space(4.0);
            ui.label(RichText::new("Template").size(12.0).color(c::TEXT_WEAK));
            for t in Template::ALL {
                let title = match t {
                    Template::Blank => "Blank".to_string(),
                    Template::Demo(trino_scaffold::Demo::Football) => {
                        "Football (Solid Eleven)".to_string()
                    }
                    Template::Demo(d) => format!("Based on the {} demo", d.key()),
                };
                ui.radio_value(&mut self.template, t, title);
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    ui.add(
                        egui::Label::new(RichText::new(t.about()).size(11.5).color(c::TEXT_FAINT))
                            .truncate(),
                    );
                });
            }
            ui.add_space(6.0);
            let dest = self.destination().display().to_string();
            let chars = (ui.available_width() / 6.6) as usize;
            ui.add(
                egui::Label::new(
                    RichText::new(crate::app::elide_start(&format!("Creates {dest}"), chars))
                        .size(11.5)
                        .color(c::TEXT_WEAK),
                )
                .truncate(),
            );
            ui.add_space(4.0);
            let ready = self.engine.is_some();
            let create = ui
                .add_enabled_ui(ready, |ui| theme::primary_button(ui, "Create project"))
                .inner;
            if create.clicked() {
                match self.create() {
                    Ok(p) => {
                        self.create_error = None;
                        open = Some(p);
                    }
                    Err(e) => self.create_error = Some(e),
                }
            }
            if !ready {
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "No engine checkout found: set TRINO_ENGINE to create projects.",
                        )
                        .size(12.0)
                        .color(c::WARN),
                    )
                    .wrap(),
                );
            }
            if let Some(e) = &self.create_error {
                ui.add(egui::Label::new(RichText::new(e).size(12.0).color(c::ERROR)).wrap());
            }
        });
        open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_need_a_trino_toml_or_an_asset_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(check_project(Path::new("")).is_err());
        let e = check_project(&dir.join("missing")).unwrap_err();
        assert!(e.contains("does not exist"), "{e}");
        std::fs::write(dir.join("file"), "").unwrap();
        assert!(
            check_project(&dir.join("file"))
                .unwrap_err()
                .contains("not a folder")
        );
        let e = check_project(dir).unwrap_err();
        assert!(e.contains("no trino.toml"), "{e}");
        std::fs::write(dir.join("trino.toml"), "").unwrap();
        assert_eq!(
            check_project(dir).unwrap(),
            trino_scaffold::absolute(dir).unwrap()
        );
        // The engine checkout opens too (its own scenes).
        let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        check_project(&engine).unwrap();
    }

    #[test]
    fn create_validates_then_scaffolds_into_folder_slash_name() {
        let engine = engine_checkout().expect("the editor is built in a checkout");
        let base = engine.join("target/test-projects");
        std::fs::create_dir_all(&base).unwrap();
        let tmp = tempfile::tempdir_in(base).unwrap();
        let mut l = Launcher::new(None, Some(engine.clone()));
        l.folder = tmp.path().display().to_string();
        l.name = "Not Valid".into();
        assert!(l.create().unwrap_err().contains("not a valid crate name"));
        l.name = "my-game".into();
        l.template = Template::Demo(trino_scaffold::Demo::Showcase);
        let dest = l.create().unwrap();
        assert_eq!(dest, tmp.path().join("my-game"));
        check_project(&dest).unwrap();
        assert!(dest.join("game/src/showcase_gen.rs").is_file());
        assert!(l.create().unwrap_err().contains("not empty"));
        let none = Launcher::new(None, None);
        assert!(none.create().unwrap_err().contains("TRINO_ENGINE"));
    }
}
