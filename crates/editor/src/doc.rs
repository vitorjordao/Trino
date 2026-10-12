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
    pub selected: Option<Vec<usize>>,
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
        if let Some(sel) = &self.selected
            && self.scene.get(sel).is_none()
        {
            self.selected = None;
        }
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
        self.selected = sel.filter(|s| self.scene.get(s).is_some());
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some((scene, sel)) = self.redo.pop() else {
            return false;
        };
        self.undo.push((self.scene.clone(), self.selected.clone()));
        self.scene = scene.clone();
        self.committed = scene;
        self.selected = sel.filter(|s| self.scene.get(s).is_some());
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
