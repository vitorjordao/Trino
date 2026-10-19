//! Prefabs: `prefabs/<name>.prefab.ron`, referenced as `prefabs/<name>`.
//!
//! The model is Blender's *linked collection instance*: a prefab is an
//! entity tree saved on its own; a scene entity with a [`PrefabInstance`]
//! draws that tree under its transform. Editing the prefab file updates every
//! instance (live, in the editor). Instances stay linked while carrying small
//! per-instance [`Override`]s keyed by child path; `Library::unpack` copies
//! the tree into the scene for one-off edits ("make instances real").
//! Prefabs may instance other prefabs; cycles are rejected at resolve time.

use serde::{Deserialize, Serialize};

use crate::Entity;

/// v5: an entity's `shadow` (issue #57, ADR 0024: what it casts, whether
/// it receives). v4: the consoles' hardware tiers' variants and LOD starts
/// (`n64_expanded`, `n3ds_new`, issue #54). v3: the Web's (`web`, issue
/// #47). v2: entities may carry per-platform variants (`platforms`, see
/// [`crate::platform`]). v1 to v4 files load unchanged and save as v5.
pub const PREFAB_VERSION: u32 = 5;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Prefab {
    pub version: u32,
    /// The prefab's root entity (its transform is the prefab's pivot
    /// offset; children are the parts).
    pub root: Entity,
}

impl Prefab {
    pub fn new(root: Entity) -> Self {
        Prefab {
            version: PREFAB_VERSION,
            root,
        }
    }

    pub fn from_ron(text: &str) -> Result<Prefab, String> {
        let mut p: Prefab = ron::from_str(text).map_err(|e| format!("prefab parse error: {e}"))?;
        if p.version > PREFAB_VERSION {
            return Err(format!(
                "prefab version {} is newer than this engine understands ({PREFAB_VERSION}) — \
                 update Trino",
                p.version
            ));
        }
        // v1 is v2 without variants, v2 is v3 without the Web, v3 is v4
        // without the tiers, v4 is v5 without shadows.
        p.version = PREFAB_VERSION;
        Ok(p)
    }

    pub fn to_ron(&self) -> String {
        crate::to_ron_pretty(self)
    }

    pub fn load(path: &std::path::Path) -> Result<Prefab, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        crate::write_text(path, &self.to_ron())
    }
}

/// A linked copy of a prefab.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PrefabInstance {
    /// Logical path, e.g. `"prefabs/tree"`.
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
}

impl PrefabInstance {
    pub fn new(path: impl Into<String>) -> Self {
        PrefabInstance {
            path: path.into(),
            overrides: Vec::new(),
        }
    }
}

/// Per-instance tweak of one entity inside the prefab, addressed by the
/// `/`-joined names below the prefab root (`""` = the root itself,
/// `"trunk/leaves"` = child `leaves` of child `trunk`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Override {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<[u8; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
}

impl Override {
    /// Apply this override to `entity` (already located by `target`).
    pub fn apply(&self, entity: &mut Entity) {
        if let Some(model) = entity.model.as_mut() {
            if let Some(m) = &self.material {
                model.material = Some(m.clone());
            }
            if let Some(t) = self.tint {
                model.tint = Some(t);
            }
        }
        if let Some(v) = self.visible {
            entity.visible = v;
        }
    }
}

/// Find the entity addressed by a `/`-joined name path below `root`.
pub fn find_by_name_path<'a>(root: &'a mut Entity, target: &str) -> Option<&'a mut Entity> {
    let mut e = root;
    for name in target.split('/').filter(|s| !s.is_empty()) {
        e = e.children.iter_mut().find(|c| c.name == name)?;
    }
    Some(e)
}

/// Apply all overrides of an instance to a copy of the prefab tree.
/// Returns the targets that matched nothing (stale overrides after a
/// rename in the prefab) so tools can warn.
pub fn apply_overrides(root: &mut Entity, overrides: &[Override]) -> Vec<String> {
    let mut stale = Vec::new();
    for o in overrides {
        match find_by_name_path(root, &o.target) {
            Some(e) => o.apply(e),
            None => stale.push(o.target.clone()),
        }
    }
    stale
}
