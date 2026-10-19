//! Trino Editor: a Unity/Blender-style shell over the real engine backend.
//!
//! - **Scene view** renders through `PcRenderer` (render-to-texture on
//!   eframe's wgpu device) with an orbit camera, grid, picking and a
//!   move/rotate/scale gizmo; the **Game view** shows the scene camera at a
//!   console's resolution and look.
//! - Scenes, prefabs, materials and fake objects are **text files** that
//!   hot-reload both ways: edit them here, or in a text editor / AI agent,
//!   and the other side updates live.
//! - **Play** launches the game as a separate process: a crashing game never
//!   takes the editor down.
//!
//! Start: `trino-editor <project-dir>` opens that project (it becomes the
//! working directory). Without one the **project launcher** shows (recent
//! projects, open, create from a template) — unless a hook that acts on
//! an open scene is set (`TRINO_EDITOR_SCENE`, `_SELECT`, `_TAB`, ...: see
//! [`EDITOR_HOOKS`]), which opens the working directory as before.
//! `--launcher` or `TRINO_EDITOR_LAUNCHER=1` always shows the launcher.
//!
//! Dev hooks: `TRINO_EDITOR_SMOKE_FRAMES=N` closes after N frames;
//! `TRINO_EDITOR_SCREENSHOT=out.png` (+ `TRINO_EDITOR_SCREENSHOT_FRAME=N`)
//! saves a screenshot of the whole window and exits (the launcher too);
//! `TRINO_EDITOR_TAB=level` opens with the Level tab focused (`mixer`,
//! `quality`, `memory`, `ui`: the Mixer, the Quality, the Memory, the UI
//! tab); `TRINO_EDITOR_UI=<screen>[,<platform>[,<lang>[,<node id>]]]` opens
//! the UI tab on a screen (`pause,3ds,en-US,resume`);
//! `TRINO_EDITOR_QUALITY=low[,dynres]` previews a quality preset in the Game
//! view's PC and Web profiles; `TRINO_EDITOR_OPEN=audio:music/theme@n64` opens the
//! Audio inspector on a sound, its waveform over that platform's decode;
//! `TRINO_EDITOR_RECENT=<file>` keeps the recent projects there.

mod access;
mod anim_panel;
mod app;
mod audio;
mod budget;
mod dialogs;
mod doc;
mod gizmo;
mod groups;
mod launcher;
#[cfg(test)]
mod layout_audit;
mod level_editor;
mod localization;
mod log;
mod ops;
mod palette;
mod panels;
mod play;
mod preview;
mod project;
mod quality;
mod recent;
mod textures;
mod theme;
mod ui_screens;
#[cfg(test)]
mod ui_tests;
mod viewport;
#[cfg(test)]
mod widget_tests;

use eframe::egui;

/// Window size: `TRINO_EDITOR_SIZE=960x600` (screenshots of small
/// windows), else 1600x940. Never below the 960x600 minimum.
fn window_size() -> [f32; 2] {
    std::env::var("TRINO_EDITOR_SIZE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some([w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?])
        })
        .map_or([1600.0, 940.0], |[w, h]| [w.max(960.0), h.max(600.0)])
}

/// Hooks that act on an open scene: any of them set (and no project
/// argument) opens the working directory in the editor, as before the
/// launcher existed.
pub const EDITOR_HOOKS: &[&str] = &[
    "TRINO_EDITOR_SCENE",
    "TRINO_EDITOR_SELECT",
    "TRINO_EDITOR_TAB",
    "TRINO_EDITOR_OPEN",
    "TRINO_EDITOR_PLAY",
    "TRINO_EDITOR_INSPECT",
    "TRINO_EDITOR_ANIM",
    "TRINO_EDITOR_ANIM_VIEW",
    "TRINO_EDITOR_PALETTE",
    "TRINO_EDITOR_GAME",
    "TRINO_EDITOR_LOD_PREVIEW",
    "TRINO_EDITOR_AO",
    "TRINO_EDITOR_SHADOWS",
    "TRINO_EDITOR_QUALITY",
    "TRINO_EDITOR_ASSET_GROUP",
    "TRINO_EDITOR_UI",
    "TRINO_EDITOR_ACCESS",
    "TRINO_EDITOR_LANG",
];

/// What the editor opens on start.
#[derive(Debug, PartialEq, Eq)]
pub enum Start {
    /// The project launcher.
    Launcher,
    /// This project folder (the first argument).
    Project(std::path::PathBuf),
    /// The working directory (an editor hook is set).
    Cwd,
}

/// Decide from the command line (without the program name) and the
/// environment (`env(name)`, empty values count as unset).
pub fn start(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Start {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    if args.iter().any(|a| a == "--launcher") {
        return Start::Launcher;
    }
    if let Some(dir) = args.iter().find(|a| !a.starts_with("--")) {
        return Start::Project(dir.into());
    }
    if env("TRINO_EDITOR_LAUNCHER").as_deref() == Some("1") {
        return Start::Launcher;
    }
    if EDITOR_HOOKS.iter().any(|k| set(k)) {
        return Start::Cwd;
    }
    Start::Launcher
}

/// Make `dir` the working directory (the editor's asset root) and add it
/// to the recent projects.
pub fn enter_project(dir: &std::path::Path) -> Result<(), String> {
    std::env::set_current_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Err(e) = recent::record_open(dir) {
        eprintln!("trino-editor: recent projects: {e}");
    }
    Ok(())
}

enum Screen {
    Launcher(Box<launcher::Launcher>),
    Editor(Box<app::App>),
}

/// The window's content: the launcher until a project opens, then the
/// editor.
struct Shell {
    screen: Screen,
    render_state: eframe::egui_wgpu::RenderState,
    // Hooks while the launcher shows (the editor runs its own).
    frames: u64,
    smoke_frames: Option<u64>,
    screenshot: Option<(u64, std::path::PathBuf)>,
    screenshot_requested: bool,
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let launcher = match &mut self.screen {
            Screen::Editor(app) => return app.ui(ui, frame),
            Screen::Launcher(l) => l,
        };
        if let Some(dir) = launcher.ui(ui) {
            match launcher::check_project(&dir).and_then(|d| enter_project(&d).map(|()| d)) {
                Ok(_) => {
                    let app = app::App::open(&ctx, self.render_state.clone());
                    self.screen = Screen::Editor(Box::new(app));
                    ctx.request_repaint();
                    return;
                }
                Err(e) => launcher.open_error = Some(e),
            }
        }
        app::screenshot_hook(
            &ctx,
            self.frames,
            &mut self.screenshot,
            &mut self.screenshot_requested,
            self.smoke_frames.is_none(),
        );
        self.frames += 1;
        if let Some(max) = self.smoke_frames {
            ctx.request_repaint();
            if self.frames >= max {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn on_exit(&mut self) {
        if let Screen::Editor(app) = &mut self.screen {
            eframe::App::on_exit(app.as_mut());
        }
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let start = start(&args, &|k| std::env::var(k).ok());
    if let Start::Project(dir) = &start {
        let entered = launcher::check_project(dir).and_then(|d| enter_project(&d));
        if let Err(e) = entered {
            eprintln!("trino-editor: {e}");
            std::process::exit(2);
        }
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("Trino Editor")
            .with_app_id("trino-editor")
            .with_inner_size(window_size())
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Trino Editor",
        options,
        Box::new(move |cc| Ok(Box::new(Shell::new(cc, &start)))),
    )
}

impl Shell {
    /// The launcher, or the editor on the working directory (main has
    /// already changed into a `Start::Project`).
    fn new(cc: &eframe::CreationContext<'_>, start: &Start) -> Self {
        let render_state = cc
            .wgpu_render_state
            .clone()
            .expect("editor requires the wgpu backend");
        let screen = match start {
            Start::Launcher => {
                theme::install(&cc.egui_ctx);
                Screen::Launcher(Box::new(launcher::Launcher::from_env()))
            }
            Start::Project(_) | Start::Cwd => {
                Screen::Editor(Box::new(app::App::open(&cc.egui_ctx, render_state.clone())))
            }
        };
        Shell {
            screen,
            render_state,
            frames: 0,
            smoke_frames: std::env::var("TRINO_EDITOR_SMOKE_FRAMES")
                .ok()
                .and_then(|v| v.parse().ok()),
            screenshot: app::screenshot_from_env(),
            screenshot_requested: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start_with(args: &[&str], vars: &[(&str, &str)]) -> Start {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        start(&args, &|k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        })
    }

    /// No project: the launcher; a project argument opens it; the old
    /// scene hooks still open the working directory; `--launcher` and
    /// `TRINO_EDITOR_LAUNCHER=1` force the launcher.
    #[test]
    fn start_picks_the_launcher_unless_a_project_or_a_scene_hook_says_otherwise() {
        assert_eq!(start_with(&[], &[]), Start::Launcher);
        assert_eq!(
            start_with(&["/games/my-game"], &[]),
            Start::Project("/games/my-game".into())
        );
        for hook in EDITOR_HOOKS {
            assert_eq!(start_with(&[], &[(hook, "x")]), Start::Cwd, "{hook}");
        }
        assert_eq!(
            start_with(&[], &[("TRINO_EDITOR_SCENE", "")]),
            Start::Launcher
        );
        // Window and capture hooks alone do not pick a project.
        for hook in [
            "TRINO_EDITOR_SIZE",
            "TRINO_EDITOR_SCREENSHOT",
            "TRINO_EDITOR_SMOKE_FRAMES",
            "TRINO_EDITOR_ECHO_LOG",
        ] {
            assert_eq!(start_with(&[], &[(hook, "1")]), Start::Launcher, "{hook}");
        }
        let scene = [("TRINO_EDITOR_SCENE", "scenes/x.scene.ron")];
        assert_eq!(start_with(&["--launcher"], &scene), Start::Launcher);
        assert_eq!(
            start_with(&[], &[("TRINO_EDITOR_LAUNCHER", "1"), scene[0]]),
            Start::Launcher
        );
        assert_eq!(
            start_with(&["."], &[("TRINO_EDITOR_LAUNCHER", "1")]),
            Start::Project(".".into()),
            "an explicit project wins"
        );
    }
}
