//! The document being edited — a scene, or a prefab opened for editing
//! (Blender-style "edit the source, every instance follows") — with
//! snapshot undo/redo.
//!
//! Undo is diff-free: after each UI frame the app calls [`Doc::commit`];
//! if the scene differs from the last committed snapshot and no pointer
//! button / text field is still active, the old snapshot goes on the undo
//! stack. Drags and typing coalesce into one step automatically, and every
//! edit path (inspector, gizmo, hierarchy, file reload) is covered without
//! per-command bookkeeping. Dirty state compares against the saved snapshot,
//! so undoing back to it makes the document clean again.

use std::path::PathBuf;

use trino_scene::{Entity, Prefab, Scene};

const MAX_UNDO: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub enum DocKind {
    Scene,
    /// Editing `prefabs/<name>` (its root is `scene.entities[0]`).
    Prefab {
        logical: String,
    },
}

pub struct Doc {
    pub kind: DocKind,
    pub path: PathBuf,
    pub scene: Scene,
    /// The active (primary) selection: the inspector and the gizmo follow it.
    pub selected: Option<Vec<usize>>,
    /// Additional selected entities (Ctrl/Shift+click, box select). Group
    /// operations (delete, duplicate, align, move) act on all of them.
    pub extra: Vec<Vec<usize>>,
    committed: Scene,
    saved: Scene,
    /// File text we last wrote/read (echo guard for the file watcher).
    pub disk_text: String,
    undo: Vec<(Scene, Option<Vec<usize>>)>,
    redo: Vec<(Scene, Option<Vec<usize>>)>,
}

impl Doc {
    pub fn scene(path: PathBuf, scene: Scene, disk_text: String) -> Self {
        Doc {
            kind: DocKind::Scene,
            path,
            committed: scene.clone(),
            saved: scene.clone(),
            scene,
            selected: None,
            extra: Vec::new(),
            disk_text,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Open a prefab for editing as a one-root scene.
    pub fn prefab(logical: String, path: PathBuf, prefab: &Prefab, disk_text: String) -> Self {
        let mut scene = Scene::new(logical.clone());
        scene.entities.push(prefab.root.clone());
        let mut doc = Doc::scene(path, scene, disk_text);
        doc.kind = DocKind::Prefab { logical };
        doc.selected = Some(vec![0]);
        doc
    }

    /// Every selected path, primary first.
    pub fn selection(&self) -> Vec<Vec<usize>> {
        self.selected
            .iter()
            .chain(self.extra.iter())
            .cloned()
            .collect()
    }

    /// Plain click: select only `path` (or nothing).
    pub fn select_only(&mut self, path: Option<Vec<usize>>) {
        self.selected = path;
        self.extra.clear();
    }

    /// Ctrl/Shift+click: add `path` (becoming the active one) or remove it.
    pub fn toggle(&mut self, path: Vec<usize>) {
        if self.selected.as_ref() == Some(&path) {
            self.selected = if self.extra.is_empty() {
                None
            } else {
                Some(self.extra.remove(0))
            };
        } else if let Some(i) = self.extra.iter().position(|p| *p == path) {
            self.extra.remove(i);
        } else {
            if let Some(old) = self.selected.take() {
                self.extra.push(old);
            }
            self.selected = Some(path);
        }
    }

    /// Box select / select all: the first path becomes the active one.
    pub fn select_many(&mut self, mut paths: Vec<Vec<usize>>) {
        paths.dedup();
        self.selected = if paths.is_empty() {
            None
        } else {
            Some(paths.remove(0))
        };
        self.extra = paths;
    }

    /// Drop selections that no longer point at an entity.
    fn prune_selection(&mut self) {
        if let Some(sel) = &self.selected
            && self.scene.get(sel).is_none()
        {
            self.selected = None;
        }
        let scene = &self.scene;
        self.extra.retain(|p| scene.get(p).is_some());
    }

    pub fn is_prefab(&self) -> bool {
        matches!(self.kind, DocKind::Prefab { .. })
    }

    pub fn title(&self) -> String {
        match &self.kind {
            DocKind::Scene => self.scene.name.clone(),
            DocKind::Prefab { logical } => logical.clone(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.scene != self.saved
    }

    /// Serialized form for disk.
    pub fn to_text(&self) -> String {
        match &self.kind {
            DocKind::Scene => self.scene.to_ron(),
            DocKind::Prefab { .. } => {
                let root = self
                    .scene
                    .entities
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Entity::new("root"));
                Prefab::new(root).to_ron()
            }
        }
    }

    /// Write to disk; returns the written text.
    pub fn save(&mut self) -> Result<(), String> {
        let text = self.to_text();
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.path, &text)
            .map_err(|e| format!("cannot write {}: {e}", self.path.display()))?;
        self.disk_text = text;
        self.saved = self.scene.clone();
        Ok(())
    }

    /// Replace content from disk (external edit), as an undoable step.
    pub fn reload_from(&mut self, scene: Scene, text: String) {
        self.push_undo();
        self.scene = scene.clone();
        self.committed = scene.clone();
        self.saved = scene;
        self.disk_text = text;
        self.prune_selection();
    }

    fn push_undo(&mut self) {
        self.undo
            .push((self.committed.clone(), self.selected.clone()));
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Record an undo step if the scene changed and the user is done
    /// interacting (`settled`). Returns true when a step was recorded.
    pub fn commit(&mut self, settled: bool) -> bool {
        if !settled || self.scene == self.committed {
            return false;
        }
        self.push_undo();
        self.committed = self.scene.clone();
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.scene != self.committed
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        // Uncommitted in-flight change: commit it first so it can be redone.
        self.commit(true);
        let Some((scene, sel)) = self.undo.pop() else {
            return false;
        };
        self.redo.push((self.scene.clone(), self.selected.clone()));
        self.scene = scene.clone();
        self.committed = scene;
        self.selected = sel;
        self.extra.clear();
        self.prune_selection();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some((scene, sel)) = self.redo.pop() else {
            return false;
        };
        self.undo.push((self.scene.clone(), self.selected.clone()));
        self.scene = scene.clone();
        self.committed = scene;
        self.selected = sel;
        self.extra.clear();
        self.prune_selection();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Doc {
        let mut s = Scene::new("t");
        s.entities.push(Entity::new("a"));
        Doc::scene(PathBuf::from("x.scene.ron"), s, String::new())
    }

    #[test]
    fn edits_coalesce_until_settled_and_undo_restores() {
        let mut d = doc();
        d.scene.entities[0].transform.pos[0] = 1.0;
        assert!(!d.commit(false), "still dragging");
        d.scene.entities[0].transform.pos[0] = 2.0;
        assert!(d.commit(true));
        assert!(d.dirty());
        assert!(d.undo());
        assert_eq!(d.scene.entities[0].transform.pos[0], 0.0);
        assert!(!d.dirty(), "back at the saved state");
        assert!(d.redo());
        assert_eq!(d.scene.entities[0].transform.pos[0], 2.0);
    }

    #[test]
    fn undo_includes_an_uncommitted_change() {
        let mut d = doc();
        d.scene.entities.push(Entity::new("b"));
        assert!(d.undo());
        assert_eq!(d.scene.entities.len(), 1);
        assert!(d.redo());
        assert_eq!(d.scene.entities.len(), 2);
    }

    #[test]
    fn multi_selection_toggles_and_prunes() {
        let mut d = doc();
        d.scene.entities.push(Entity::new("b"));
        d.scene.entities.push(Entity::new("c"));
        d.commit(true);
        d.select_only(Some(vec![0]));
        d.toggle(vec![1]);
        assert_eq!(d.selected, Some(vec![1]), "last clicked is active");
        assert_eq!(d.selection(), vec![vec![1], vec![0]]);
        d.toggle(vec![1]);
        assert_eq!(d.selection(), vec![vec![0]]);
        d.select_many(vec![vec![2], vec![0]]);
        let sel = d.selection();
        assert!(sel.contains(&vec![0]) && sel.contains(&vec![2]) && !sel.contains(&vec![1]));
        d.scene.entities.truncate(1);
        d.commit(true);
        assert!(d.undo());
        assert!(d.extra.is_empty(), "undo resets to the primary selection");
    }

    #[test]
    fn save_reload_and_history_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = doc();
        d.path = dir.path().join("nested/x.scene.ron");
        assert_eq!(d.title(), "t");
        assert!(!d.can_undo() && !d.can_redo());
        assert!(!d.undo() && !d.redo(), "nothing to undo or redo yet");

        d.scene.entities[0].name = "renamed".into();
        assert!(d.can_undo(), "an uncommitted edit is undoable");
        d.commit(true);
        assert!(d.dirty());
        d.save().unwrap();
        assert!(!d.dirty(), "saving makes the document clean");
        let on_disk = std::fs::read_to_string(&d.path).unwrap();
        assert_eq!(on_disk, d.disk_text);
        assert_eq!(Scene::from_ron(&on_disk).unwrap(), d.scene);

        // An external edit replaces the scene, prunes a stale selection and
        // stays undoable.
        d.select_many(vec![vec![0], vec![5]]);
        let mut external = d.scene.clone();
        external.entities.clear();
        d.reload_from(external.clone(), external.to_ron());
        assert_eq!(d.selected, None);
        assert!(d.extra.is_empty());
        assert!(!d.dirty(), "the reloaded text is the saved state");
        assert!(d.undo());
        assert_eq!(d.scene.entities[0].name, "renamed");

        // The undo stack is capped.
        for i in 0..(MAX_UNDO + 20) {
            d.scene.entities[0].transform.pos[1] = i as f32 + 1.0;
            d.commit(true);
        }
        let mut steps = 0;
        while d.undo() {
            steps += 1;
        }
        assert_eq!(steps, MAX_UNDO);

        // Saving into a path that cannot be a directory fails cleanly.
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        d.path = blocker.join("x.scene.ron");
        assert!(d.save().is_err());
    }

    #[test]
    fn prefab_docs_serialize_as_prefabs() {
        let p = Prefab::new(Entity::new("lamp"));
        let d = Doc::prefab(
            "prefabs/lamp".into(),
            PathBuf::from("p.prefab.ron"),
            &p,
            String::new(),
        );
        assert!(d.is_prefab());
        assert_eq!(Prefab::from_ron(&d.to_text()).unwrap(), p);
    }
}
