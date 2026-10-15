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
//! Dev hooks: `TRINO_EDITOR_SMOKE_FRAMES=N` closes after N frames;
//! `TRINO_EDITOR_SCREENSHOT=out.png` (+ `TRINO_EDITOR_SCREENSHOT_FRAME=N`)
//! saves a screenshot of the whole window and exits; `TRINO_EDITOR_TAB=level`
//! opens with the Level tab focused.

mod app;
mod budget;
mod dialogs;
mod doc;
mod gizmo;
#[cfg(test)]
mod layout_audit;
mod level_editor;
mod log;
mod ops;
mod palette;
mod panels;
mod play;
mod project;
mod theme;
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

fn main() -> eframe::Result {
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
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
