//! Level tab: paint the platformer's ASCII tilemap and keep it in two-way
//! sync with the file on disk.
//!
//! The file (`examples/platformer/src/level1.txt`) is the single source of
//! truth — deliberately plain text so AI agents (or any external tool) can
//! edit it directly:
//!
//! - painting in the editor **saves immediately** to the file;
//! - external writes (an agent editing the ASCII) are picked up by the file
//!   watcher and reload the grid live.
//!
//! An echo guard (`last_written`) keeps the editor's own saves from
//! bouncing back as "external" reloads.

use std::path::PathBuf;

use eframe::egui;

/// Paintable tile kinds, in palette order.
pub const PALETTE: [(u8, &str); 6] = [
    (b'#', "Ground"),
    (b'B', "Brick"),
    (b'C', "Coin"),
    (b'F', "Flag"),
    (b'P', "Spawn"),
    (b'.', "Erase"),
];

pub struct LevelEditor {
    pub path: PathBuf,
    grid: Vec<Vec<u8>>,
    pub brush: u8,
    /// Exactly what this editor last wrote (echo guard for the watcher).
    last_written: String,
    /// Load/save error surfaced in the panel.
    pub error: Option<String>,
}

impl LevelEditor {
    pub fn load(path: PathBuf) -> Self {
        let mut editor = LevelEditor {
            path,
            grid: Vec::new(),
            brush: b'#',
            last_written: String::new(),
            error: None,
        };
        editor.reload_from_disk();
        editor
    }

    fn parse(text: &str) -> Vec<Vec<u8>> {
        text.lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.bytes().collect())
            .collect()
    }

    fn serialize(&self) -> String {
        let mut out = String::new();
        for row in &self.grid {
            out.push_str(core::str::from_utf8(row).unwrap_or(""));
            out.push('\n');
        }
        out
    }

    pub fn reload_from_disk(&mut self) {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => {
                self.grid = Self::parse(&text);
                self.last_written = text;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{}: {e}", self.path.display())),
        }
    }

    /// Called on watcher events. Returns true when an external change was
    /// loaded (callers log it).
    pub fn maybe_external_reload(&mut self) -> bool {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return false;
        };
        if text == self.last_written {
            return false; // our own save echoing back
        }
        self.grid = Self::parse(&text);
        self.last_written = text;
        self.error = None;
        true
    }

    fn paint(&mut self, col: usize, row: usize) -> bool {
        let Some(cell) = self.grid.get_mut(row).and_then(|r| r.get_mut(col)) else {
            return false;
        };
        if *cell == self.brush {
            return false;
        }
        *cell = self.brush;
        // The file is the source of truth: every stroke lands on disk so
        // watchers (xtask watch, agents) see it instantly.
        let text = self.serialize();
        match std::fs::write(&self.path, &text) {
            Ok(()) => {
                self.last_written = text;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("save failed: {e}")),
        }
        true
    }

    #[cfg(test)]
    pub fn tile_at(&self, col: usize, row: usize) -> u8 {
        self.grid[row][col]
    }

    #[cfg(test)]
    pub fn paint_for_test(&mut self, col: usize, row: usize) -> bool {
        self.paint(col, row)
    }

    fn tile_color(tile: u8) -> egui::Color32 {
        match tile {
            b'#' => egui::Color32::from_rgb(143, 86, 59),
            b'B' => egui::Color32::from_rgb(172, 50, 50),
            b'C' => egui::Color32::from_rgb(252, 224, 40),
            b'F' => egui::Color32::from_rgb(60, 200, 80),
            b'P' => egui::Color32::from_rgb(80, 150, 255),
            _ => egui::Color32::from_rgb(28, 32, 48),
        }
    }

    /// The Level tab UI. Returns log lines to append to the console.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Vec<String> {
        let mut log = Vec::new();

        ui.horizontal(|ui| {
            ui.label("Brush:");
            for (tile, name) in PALETTE {
                let selected = self.brush == tile;
                let color = Self::tile_color(tile);
                let text = egui::RichText::new(format!("{} {name}", tile as char))
                    .monospace()
                    .color(if tile == b'.' {
                        egui::Color32::GRAY
                    } else {
                        color
                    });
                if ui.selectable_label(selected, text).clicked() {
                    self.brush = tile;
                }
            }
            ui.separator();
            ui.monospace(self.path.display().to_string());
            if ui.button("↻ Reload").clicked() {
                self.reload_from_disk();
                log.push("level reloaded from disk".into());
            }
        });
        if let Some(err) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        }
        ui.separator();

        let rows = self.grid.len();
        let cols = self.grid.first().map(|r| r.len()).unwrap_or(0);
        if rows == 0 || cols == 0 {
            ui.label("No level loaded.");
            return log;
        }

        const CELL: f32 = 16.0;
        egui::ScrollArea::both().show(ui, |ui| {
            let size = egui::vec2(cols as f32 * CELL, rows as f32 * CELL);
            let (response, painter) = ui.allocate_painter(size, egui::Sense::click_and_drag());
            let origin = response.rect.min;

            for (row, line) in self.grid.iter().enumerate() {
                for (col, &tile) in line.iter().enumerate() {
                    let min = origin + egui::vec2(col as f32 * CELL, row as f32 * CELL);
                    let rect = egui::Rect::from_min_size(min, egui::vec2(CELL, CELL));
                    painter.rect_filled(rect, 0.0, Self::tile_color(tile));
                    if tile == b'C' || tile == b'F' || tile == b'P' {
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            tile as char,
                            egui::FontId::monospace(10.0),
                            egui::Color32::BLACK,
                        );
                    }
                }
            }
            // Grid lines (light).
            let stroke = egui::Stroke::new(0.5, egui::Color32::from_black_alpha(90));
            for col in 0..=cols {
                let x = origin.x + col as f32 * CELL;
                painter.vline(x, origin.y..=origin.y + size.y, stroke);
            }
            for row in 0..=rows {
                let y = origin.y + row as f32 * CELL;
                painter.hline(origin.x..=origin.x + size.x, y, stroke);
            }

            // Paint with click or drag.
            if (response.clicked() || response.dragged())
                && let Some(pos) = response.interact_pointer_pos()
            {
                let local = pos - origin;
                let col = (local.x / CELL) as usize;
                let row = (local.y / CELL) as usize;
                if self.paint(col, row) {
                    log.push(format!(
                        "painted {} at ({col},{row}) -> saved",
                        self.brush as char
                    ));
                }
            }
        });
        log
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_level(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trino-level-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.txt"));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn paint_writes_the_file_immediately() {
        let path = temp_level("paint", "...\n###\n");
        let mut editor = LevelEditor::load(path.clone());
        editor.brush = b'C';
        assert!(editor.paint_for_test(1, 0));
        assert_eq!(editor.tile_at(1, 0), b'C');
        // The stroke is on disk already — the file is the source of truth.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ".C.\n###\n");
        // Same-brush repaint is a no-op (no redundant writes).
        assert!(!editor.paint_for_test(1, 0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn external_change_reloads_but_own_save_does_not() {
        let path = temp_level("external", "...\n###\n");
        let mut editor = LevelEditor::load(path.clone());

        // Our own save echoing back through the watcher: ignored.
        editor.brush = b'B';
        editor.paint_for_test(0, 0);
        assert!(!editor.maybe_external_reload(), "own write must not bounce");

        // A real external edit (an AI agent rewriting the ASCII): loaded.
        std::fs::write(&path, "FFF\n###\n").unwrap();
        assert!(editor.maybe_external_reload());
        assert_eq!(editor.tile_at(0, 0), b'F');
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn out_of_bounds_paint_is_ignored() {
        let path = temp_level("oob", "..\n##\n");
        let mut editor = LevelEditor::load(path.clone());
        assert!(!editor.paint_for_test(99, 0));
        assert!(!editor.paint_for_test(0, 99));
        let _ = std::fs::remove_file(&path);
    }
}
