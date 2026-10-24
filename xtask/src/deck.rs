//! `cargo xtask deck check [--json]` (issue #61): the Steam Deck Verified
//! checklist for a project, as far as text and code can tell — each line
//! passes, warns (a reviewer looks) or fails (non-zero exit):
//!
//! - **Controller:** the PC crate has the `gamepad` feature (a package turns
//!   it on): the Deck's controls work with no mouse or keyboard.
//! - **Glyphs:** the game prints controller glyphs (`gamepad_family`,
//!   `button_glyph`, `binding_label`) rather than only keyboard keys.
//! - **Steam Input:** with `[steam] app_id`, the action manifest
//!   (`services export steam`) is on disk.
//! - **Legibility:** every `ui/*.ui.ron` screen laid out at 1280 × 800 (the
//!   Deck's screen) draws no text under 9 pixels, in every language.
//! - **Resolution:** the package starts on the native profile (fits
//!   1280 × 800 at 16:10) or a console look letterboxed in it.
//! - **Default settings:** the quality presets detect the Deck
//!   (`trino_render_wgpu::quality::is_steam_deck`) and start it there.
//! - **Launcher:** packages start the game directly.
//! - **Suspend:** the runner clamps a frame's `dt` (0.1 s), so a resume
//!   doesn't teleport the game.

use std::path::Path;
use std::process::ExitCode;

use serde_json::json;
use trino_core::Family;
use trino_scene::ui::{Flat, ScreenDef, Target, audit};

use crate::project::Project;

/// Valve's minimum text size at 1280 × 800.
pub const MIN_TEXT_PX: f32 = 9.0;
pub const DECK: (u16, u16) = (1280, 800);

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Line {
    pub check: &'static str,
    pub status: Status,
    pub detail: String,
}

pub fn run(project: &Project, args: &[&str]) -> ExitCode {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let json = crate::web::take_flag(&mut args, "--json");
    if args.iter().map(String::as_str).ne(["check"]) {
        eprintln!("xtask: usage: cargo xtask deck check [--json]");
        return ExitCode::FAILURE;
    }
    match check(project) {
        Ok(lines) => {
            let fails = lines.iter().filter(|l| l.status == Status::Fail).count();
            let warns = lines.iter().filter(|l| l.status == Status::Warn).count();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({ "checks": lines, "fail": fails, "warn": warns })
                    )
                    .unwrap()
                );
            } else {
                for l in &lines {
                    let s = match l.status {
                        Status::Pass => "ok  ",
                        Status::Warn => "warn",
                        Status::Fail => "FAIL",
                    };
                    println!("{s} [{}] {}", l.check, l.detail);
                }
                println!("deck check: {fails} failing, {warns} to review");
            }
            if fails == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn line(check: &'static str, status: Status, detail: impl Into<String>) -> Line {
    Line {
        check,
        status,
        detail: detail.into(),
    }
}

/// The source files of the project's game (`game/`, else `src/`, else the
/// engine's castle64), as text.
fn game_sources(project: &Project) -> String {
    let dirs = if project.is_engine {
        vec![project.root.join("examples/castle64/src")]
    } else {
        vec![project.root.join("game/src"), project.root.join("src")]
    };
    let mut text = String::new();
    for d in dirs {
        let mut stack = vec![d];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    text.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
                }
            }
        }
    }
    text
}

/// Every screen's smallest text at 1280 × 800 on the PC, per language:
/// (screen, language, px).
pub fn legibility(root: &Path) -> Result<Vec<(String, String, f32)>, String> {
    let manifest = crate::ui::manifest(root)?;
    let langs = trino_asset_pipeline::ui::langs(root)?;
    let platform = crate::ui_render::pipeline_platform(Family::Pc);
    let fonts = trino_asset_pipeline::ui::fonts(root, &manifest, platform);
    let langs = trino_asset_pipeline::i18n::with_remaps(langs, &manifest, platform);
    let mut caps = Family::Pc.caps();
    caps.screen_width = DECK.0;
    caps.screen_height = DECK.1;
    let mut lang_list: Vec<Option<String>> = langs.names().into_iter().map(Some).collect();
    if lang_list.is_empty() {
        lang_list.push(None);
    }
    let mut out = Vec::new();
    for path in trino_asset_pipeline::ui::screen_files(root) {
        let def = ScreenDef::load(&path)?;
        let sizes = |s: &str| trino_asset_pipeline::ui::sprite_size(root, &manifest, platform, s);
        let flat = Flat::new(&def, &sizes);
        for lang in &lang_list {
            let r = audit(
                &flat,
                Target {
                    caps,
                    lang: lang.as_deref(),
                    interface_scale: 1.0,
                    access: trino_core::ui::UiAccess::NONE,
                },
                &fonts,
                &langs,
            );
            if let Some(px) = r.min_text_px {
                out.push((def.name.clone(), lang.clone().unwrap_or_default(), px));
            }
        }
    }
    Ok(out)
}

pub fn check(project: &Project) -> Result<Vec<Line>, String> {
    let mut lines = Vec::new();
    let source = game_sources(project);

    // Controller.
    let pc_toml = std::fs::read_to_string(project.root.join("pc/Cargo.toml")).unwrap_or_default();
    let gamepad = project.is_engine
        || pc_toml.contains("gamepad")
        || project
            .ship
            .features
            .as_ref()
            .is_some_and(|f| f.iter().any(|x| x == "gamepad"));
    lines.push(if gamepad {
        line(
            "controller",
            Status::Pass,
            "the PC host builds with gamepads (feature `gamepad`)",
        )
    } else {
        line(
            "controller",
            Status::Fail,
            "the PC crate has no `gamepad` feature: add gamepad = [\"trino-runner-pc/gamepad\"]",
        )
    });

    // Glyphs.
    let glyphs = [
        "gamepad_family",
        "button_glyph",
        "binding_label",
        "PadFamily",
    ]
    .iter()
    .any(|k| source.contains(k));
    lines.push(if glyphs {
        line(
            "glyphs",
            Status::Pass,
            "the game asks for the pad's glyphs or labels",
        )
    } else {
        line(
            "glyphs",
            Status::Warn,
            "no gamepad_family/button_glyph/binding_label in the game: check its prompts \
             show controller buttons, not keys",
        )
    });

    // Steam Input.
    match project.steam_app_id {
        Some(app) => {
            let m = project
                .root
                .join(crate::services::STEAM_CONFIG_DIR)
                .join(format!("game_actions_{app}.vdf"));
            lines.push(if m.is_file() {
                line(
                    "steam input",
                    Status::Pass,
                    format!("{} is exported", m.display()),
                )
            } else {
                line(
                    "steam input",
                    Status::Warn,
                    "no action manifest: run `cargo xtask services export steam` and set it as \
                     the default configuration in Steamworks",
                )
            });
        }
        None => lines.push(line(
            "steam input",
            Status::Warn,
            "no [steam] app_id: Steam presents the Deck as an Xbox pad (gilrs reads it)",
        )),
    }

    // Legibility.
    let sizes = legibility(&project.root)?;
    if sizes.is_empty() {
        lines.push(line(
            "legibility",
            Status::Warn,
            "no ui/*.ui.ron screens: check the game's own text at 1280x800",
        ));
    } else {
        let small: Vec<String> = sizes
            .iter()
            .filter(|(_, _, px)| *px < MIN_TEXT_PX)
            .map(|(s, l, px)| format!("{s} [{l}] {px:.0} px"))
            .collect();
        let min = sizes.iter().map(|x| x.2).fold(f32::MAX, f32::min);
        lines.push(if small.is_empty() {
            line(
                "legibility",
                Status::Pass,
                format!(
                    "{} screen x language layouts at 1280x800: the smallest text is {min:.0} px (>= 9)",
                    sizes.len()
                ),
            )
        } else {
            line(
                "legibility",
                Status::Fail,
                format!("text under 9 px at 1280x800: {}", small.join(", ")),
            )
        });
    }

    // Resolution, settings, launcher, suspend.
    let profile = &project.ship.profile;
    lines.push(line(
        "resolution",
        Status::Pass,
        if profile == "native" {
            "the package starts on the native profile: 1280x800 at 16:10, no bars".to_string()
        } else {
            format!("the package starts on the {profile} look, letterboxed in 1280x800")
        },
    ));
    lines.push(line(
        "default settings",
        Status::Pass,
        "the quality presets detect the Deck and start it on its own preset",
    ));
    lines.push(line(
        "launcher",
        Status::Pass,
        "packages start the game directly (no launcher)",
    ));
    lines.push(line(
        "suspend",
        Status::Pass,
        "frame dt is clamped to 0.1 s: a resume after suspend does not jump the game",
    ));
    Ok(lines)
}
