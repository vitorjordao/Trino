//! The project's quality presets the Quality tab edits (issue #55):
//! `assets/quality.ron` (or what `trino.toml`'s `[pc.quality] presets`
//! names) over the engine's built-in Low…Ultra, and `[pc.quality] default`
//! (shown, edited in `trino.toml`). The same file `cargo xtask quality
//! show|set|reset` reads and writes; the Game view previews a preset of it.

use std::path::{Path, PathBuf};

use trino_asset_pipeline::quality::{ProjectQuality, QualityFile};
use trino_core::{PresetTable, QualityPreset, QualitySettings};

pub struct QualityState {
    /// What the tab shows (edits land here first).
    pub table: PresetTable,
    /// `trino.toml`'s `[pc.quality]` (its defaults without one).
    pub project: ProjectQuality,
    /// The presets file exists (else the first edit creates it).
    pub exists: bool,
    /// Why the file on disk (or `trino.toml`) isn't valid.
    pub error: Option<String>,
    /// Edited and not written yet (written once the user settles).
    pub dirty: bool,
    /// Bumped on every change, for the views that preview a preset.
    pub rev: u64,
}

impl QualityState {
    pub fn load(root: &Path) -> QualityState {
        let (project, error) = match ProjectQuality::load(root) {
            Ok(p) => (p, None),
            Err(e) => (ProjectQuality::default(), Some(e)),
        };
        let mut s = QualityState {
            table: PresetTable::BUILT_IN,
            project,
            exists: false,
            error,
            dirty: false,
            rev: 0,
        };
        let text = std::fs::read_to_string(s.path(root)).ok();
        let keep = s.error.clone();
        s.reload(text.as_deref());
        if s.error.is_none() {
            s.error = keep;
        }
        s
    }

    /// The presets file under `root`.
    pub fn path(&self, root: &Path) -> PathBuf {
        root.join(&self.project.presets)
    }

    /// The file's new content (`None`: deleted). A broken file keeps the
    /// last good table and says why.
    pub fn reload(&mut self, text: Option<&str>) {
        self.exists = text.is_some();
        self.dirty = false;
        self.rev += 1;
        match text.map(|t| QualityFile::parse(t).and_then(|f| f.table())) {
            None => {
                self.table = PresetTable::BUILT_IN;
                self.error = None;
            }
            Some(Ok(t)) => {
                self.table = t;
                self.error = None;
            }
            Some(Err(e)) => self.error = Some(e),
        }
    }

    pub fn get(&self, p: QualityPreset) -> QualitySettings {
        self.table.get(p)
    }

    /// A preset's new values (`Custom` is not one).
    pub fn set(&mut self, p: QualityPreset, q: QualitySettings) {
        let Some(i) = p.index() else {
            return;
        };
        let q = QualitySettings { preset: p, ..q };
        if self.table.presets[i] != q {
            self.table.presets[i] = q;
            self.dirty = true;
            self.rev += 1;
        }
    }

    /// Back to the engine's values.
    pub fn reset(&mut self, p: QualityPreset) {
        self.set(p, QualitySettings::preset(p));
    }

    /// The preset differs from the engine's.
    pub fn changed(&self, p: QualityPreset) -> bool {
        self.get(p) != QualitySettings::preset(p)
    }

    /// The file to write: each preset's changes, checked by reading it
    /// back (a value out of the file's ranges is refused).
    pub fn to_text(&self) -> Result<String, String> {
        let text = QualityFile::from_table(&self.table).to_ron();
        let back = QualityFile::parse(&text).and_then(|f| f.table())?;
        if back != self.table {
            return Err("a value is out of its range".into());
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_round_trip_and_bad_files_keep_the_last_good_table() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = QualityState::load(dir.path());
        assert!(!s.exists && s.error.is_none());
        assert_eq!(s.table, PresetTable::BUILT_IN);
        assert!(!s.changed(QualityPreset::Low));
        s.set(
            QualityPreset::Low,
            QualitySettings {
                render_scale: 0.6,
                ..s.get(QualityPreset::Low)
            },
        );
        assert!(s.dirty && s.changed(QualityPreset::Low));
        let text = s.to_text().unwrap();
        assert!(text.contains("render_scale: 0.6"), "{text}");
        s.reload(Some(&text));
        assert_eq!(s.get(QualityPreset::Low).render_scale, 0.6);
        assert!(s.exists && !s.dirty);
        // A broken file: the table stays, the error shows.
        s.reload(Some("(version: 1, low: (msaa: 3))"));
        assert!(s.error.as_deref().unwrap().contains("msaa 3"));
        assert_eq!(s.get(QualityPreset::Low).render_scale, 0.6);
        // Out of the file's ranges: refused.
        s.set(
            QualityPreset::High,
            QualitySettings {
                msaa: 2,
                ..s.get(QualityPreset::High)
            },
        );
        assert!(s.to_text().is_err());
        s.reset(QualityPreset::High);
        s.reset(QualityPreset::Low);
        assert!(!s.changed(QualityPreset::Low));
        // Deleted: the built-in values.
        s.reload(None);
        assert_eq!(s.table, PresetTable::BUILT_IN);
        // trino.toml's section moves the file and says the default.
        std::fs::write(
            dir.path().join("trino.toml"),
            "[pc.quality]\ndefault = \"medium\"\npresets = \"assets/q.ron\"\n",
        )
        .unwrap();
        let s = QualityState::load(dir.path());
        assert_eq!(s.project.default, "medium");
        assert_eq!(s.path(dir.path()), dir.path().join("assets/q.ron"));
    }
}
