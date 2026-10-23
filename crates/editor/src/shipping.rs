//! The Project tab's state (issue #61): its Achievements page edits
//! `assets/achievements.ron` (the file `cargo xtask services` reads and
//! the bake turns into `services/achievements`), checks it live
//! (`trino_scene::services::check`), freezes the bit order
//! (`assets/achievements.lock`) and exports Steam's files; its Packaging
//! page edits `trino.toml`'s packaging keys in place
//! (`trino_asset_pipeline::ship`, what `cargo xtask package config set`
//! writes) and runs `cargo xtask package pc` / `deck check`. An edit on
//! disk shows here live; an invalid file is shown, not written.

use std::path::{Path, PathBuf};

use trino_scene::services::{self as svc, AchievementDef, AchievementsDef, Finding, StatDef};

/// Which page the tab shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Achievements,
    Packaging,
}

pub struct ShippingState {
    pub page: Page,
    /// The achievements as the page shows them (edits land here first).
    pub def: AchievementsDef,
    /// `assets/achievements.ron` exists (else the first edit creates it).
    pub exists: bool,
    /// Why the file on disk isn't valid (the page keeps the last good one).
    pub error: Option<String>,
    /// Edited and not written yet.
    pub dirty: bool,
    /// What `services check` says about [`def`](Self::def).
    pub findings: Vec<Finding>,
    /// The shipped order (`assets/achievements.lock`).
    pub lock: Option<Vec<String>>,
    /// The selected row.
    pub selected: Option<usize>,
    /// Unlocked in the overlay preview (by row).
    pub preview_unlocked: Vec<bool>,
    /// The preview's popup.
    pub toast: trino_core::AchievementToast,
    /// `trino.toml`'s packaging keys.
    pub ship: trino_asset_pipeline::ship::Ship,
    pub ship_error: Option<String>,
    /// What the Packaging page's fields hold (written on Enter / focus out).
    pub ship_edit: trino_asset_pipeline::ship::Ship,
    /// A `cargo xtask package|deck` run and what it is.
    pub job: Option<(std::process::Child, &'static str)>,
    /// The last run's result line.
    pub last_job: Option<String>,
    /// Bumped on every change (the preview re-renders).
    pub rev: u64,
}

impl ShippingState {
    pub fn load(root: &Path) -> ShippingState {
        let mut s = ShippingState {
            page: match std::env::var("TRINO_EDITOR_PROJECT_PAGE").as_deref() {
                Ok("packaging") => Page::Packaging,
                _ => Page::Achievements,
            },
            def: AchievementsDef::default(),
            exists: false,
            error: None,
            dirty: false,
            findings: Vec::new(),
            lock: svc::load_lock(root),
            selected: None,
            preview_unlocked: Vec::new(),
            toast: trino_core::AchievementToast::new(),
            ship: Default::default(),
            ship_error: None,
            ship_edit: Default::default(),
            job: None,
            last_job: None,
            rev: 0,
        };
        let text = std::fs::read_to_string(svc::path(root)).ok();
        s.reload(text.as_deref());
        s.reload_ship(root);
        s
    }

    pub fn path(root: &Path) -> PathBuf {
        svc::path(root)
    }

    /// The file's new content (`None`: deleted).
    pub fn reload(&mut self, text: Option<&str>) {
        self.exists = text.is_some();
        self.dirty = false;
        self.rev += 1;
        match text.map(AchievementsDef::from_ron) {
            None => {
                self.def = AchievementsDef::default();
                self.error = None;
            }
            Some(Ok(d)) => {
                self.def = d;
                self.error = None;
            }
            Some(Err(e)) => self.error = Some(e),
        }
        self.preview_unlocked
            .resize(self.def.achievements.len(), false);
    }

    pub fn reload_ship(&mut self, root: &Path) {
        match trino_asset_pipeline::ship::Ship::load(root) {
            Ok(s) => {
                self.ship_edit = s.clone();
                self.ship = s;
                self.ship_error = None;
            }
            Err(e) => self.ship_error = Some(e),
        }
    }

    pub fn reload_lock(&mut self, root: &Path) {
        self.lock = svc::load_lock(root);
    }

    /// Run `services check` on the page's achievements.
    pub fn check(&mut self, root: &Path, sprites: &dyn Fn(&str) -> bool) {
        let langs = trino_asset_pipeline::ui::langs(root).unwrap_or_default();
        self.findings = svc::check(&self.def, &langs, self.lock.as_deref(), sprites);
    }

    pub fn changed(&mut self) {
        self.dirty = true;
        self.rev += 1;
        self.preview_unlocked
            .resize(self.def.achievements.len(), false);
    }

    /// Bits the lock freezes (rows that can't move or go).
    pub fn locked_rows(&self) -> usize {
        self.lock.as_ref().map_or(0, |l| l.len())
    }

    /// A new achievement at the end, with a fresh id and its keys.
    pub fn add_achievement(&mut self) -> usize {
        let mut n = self.def.achievements.len() + 1;
        let id = loop {
            let id = format!("ACHIEVEMENT_{n}");
            if !self.def.achievements.iter().any(|a| a.id == id)
                && !self.def.stats.iter().any(|s| s.id == id)
            {
                break id;
            }
            n += 1;
        };
        let key = id.to_ascii_lowercase();
        self.def.achievements.push(AchievementDef {
            name: format!("ach.{key}.name"),
            description: format!("ach.{key}.desc"),
            id,
            ..AchievementDef::default()
        });
        self.changed();
        self.def.achievements.len() - 1
    }

    /// Remove row `i` (refused for a locked row: saves hold its bit).
    pub fn remove_achievement(&mut self, i: usize) -> Result<(), String> {
        if i < self.locked_rows() {
            return Err(format!(
                "{} is in achievements.lock: a shipped save holds its bit",
                self.def.achievements[i].id
            ));
        }
        self.def.achievements.remove(i);
        self.selected = None;
        self.changed();
        Ok(())
    }

    /// Swap rows `i` and `i + 1` (both past the lock).
    pub fn move_down(&mut self, i: usize) -> Result<(), String> {
        if i < self.locked_rows() || i + 1 >= self.def.achievements.len() {
            return Err("locked rows keep their order".into());
        }
        self.def.achievements.swap(i, i + 1);
        self.changed();
        Ok(())
    }

    pub fn add_stat(&mut self) {
        let mut n = self.def.stats.len() + 1;
        while self.def.stats.iter().any(|s| s.id == format!("STAT_{n}")) {
            n += 1;
        }
        self.def.stats.push(StatDef {
            id: format!("STAT_{n}"),
            ..StatDef::default()
        });
        self.changed();
    }

    /// The text to write.
    pub fn to_text(&self) -> String {
        self.def.to_ron()
    }

    /// The preview's state: the rows ticked "unlocked".
    pub fn preview_state(&self) -> trino_core::AchievementState {
        let mut s = trino_core::AchievementState::EMPTY;
        for (i, on) in self.preview_unlocked.iter().enumerate() {
            s.set_unlocked(i, *on);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_respect_the_lock_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ShippingState::load(dir.path());
        assert!(!s.exists && s.def.achievements.is_empty());
        let a = s.add_achievement();
        let b = s.add_achievement();
        assert_eq!((a, b), (0, 1));
        assert_eq!(s.def.achievements[0].id, "ACHIEVEMENT_1");
        assert_eq!(s.def.achievements[1].name, "ach.achievement_2.name");
        s.add_stat();
        assert!(s.dirty);
        let text = s.to_text();
        s.reload(Some(&text));
        assert_eq!(s.def.achievements.len(), 2);
        assert_eq!(s.def.stats[0].id, "STAT_1");
        s.lock = Some(vec!["ACHIEVEMENT_1".into()]);
        assert!(s.remove_achievement(0).is_err());
        assert!(s.move_down(0).is_err());
        s.add_achievement();
        assert!(s.move_down(1).is_ok());
        assert_eq!(s.def.achievements[2].id, "ACHIEVEMENT_2");
        assert!(s.remove_achievement(2).is_ok());
        // A broken file keeps the last good list.
        s.reload(Some("(version: 1, achievments: [])"));
        assert!(s.error.is_some());
        assert_eq!(s.def.achievements.len(), 2);
        s.preview_unlocked[1] = true;
        assert!(s.preview_state().is_unlocked(1));
    }
}
