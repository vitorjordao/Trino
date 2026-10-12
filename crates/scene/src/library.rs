//! The project's reusable content, loaded from disk: materials and prefabs
//! by logical path. The editor keeps one `Library` and reloads entries when
//! their files change; tools and tests load one with [`Library::load`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::prefab::{Prefab, PrefabInstance, apply_overrides};
use crate::{Entity, MaterialDef, Scene};

pub const MATERIALS_DIR: &str = "assets/materials";
pub const PREFABS_DIR: &str = "prefabs";
pub const MATERIAL_EXT: &str = ".material.ron";
pub const PREFAB_EXT: &str = ".prefab.ron";

/// Local bounding box of a baked model (`min`, `max`), for `Auto`
/// colliders. Filled by tools that can bake models (xtask, editor);
/// [`Library::load`] leaves it empty.
pub type ModelBounds = ([f32; 3], [f32; 3]);

#[derive(Clone, Debug, Default)]
pub struct Library {
    pub materials: BTreeMap<String, MaterialDef>,
    pub prefabs: BTreeMap<String, Prefab>,
    /// `models/<name>` -> local AABB (see [`ModelBounds`]).
    pub model_bounds: BTreeMap<String, ModelBounds>,
    /// Files that failed to parse (path: error) — shown, never fatal.
    pub errors: Vec<String>,
}

/// `prefix/<relative path without ext>` for every `*ext` file under `dir`.
fn scan(dir: &Path, ext: &str, prefix: &str) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(rel) = path
                .strip_prefix(dir)
                .ok()
                .and_then(|r| r.to_str())
                .and_then(|r| r.strip_suffix(ext))
            {
                out.push((format!("{prefix}/{}", rel.replace('\\', "/")), path));
            }
        }
    }
    out.sort();
    out
}

impl Library {
    /// Load every material and prefab under the project root.
    pub fn load(root: &Path) -> Library {
        let mut lib = Library::default();
        for (logical, path) in scan(&root.join(MATERIALS_DIR), MATERIAL_EXT, "materials") {
            match MaterialDef::load(&path) {
                Ok(m) => {
                    lib.materials.insert(logical, m);
                }
                Err(e) => lib.errors.push(e),
            }
        }
        for (logical, path) in scan(&root.join(PREFABS_DIR), PREFAB_EXT, "prefabs") {
            match Prefab::load(&path) {
                Ok(p) => {
                    lib.prefabs.insert(logical, p);
                }
                Err(e) => lib.errors.push(e),
            }
        }
        lib
    }

    /// File backing a material logical path (`materials/wood`).
    pub fn material_file(root: &Path, logical: &str) -> PathBuf {
        let rel = logical.strip_prefix("materials/").unwrap_or(logical);
        root.join(MATERIALS_DIR)
            .join(format!("{rel}{MATERIAL_EXT}"))
    }

    /// File backing a prefab logical path (`prefabs/tree`).
    pub fn prefab_file(root: &Path, logical: &str) -> PathBuf {
        let rel = logical.strip_prefix("prefabs/").unwrap_or(logical);
        root.join(PREFABS_DIR).join(format!("{rel}{PREFAB_EXT}"))
    }

    /// Logical path for a file on disk, if it is a material or prefab.
    /// `./` components are ignored on both sides (`./prefabs/a` and
    /// `prefabs/a` are the same file to a watcher).
    pub fn logical_for_file(root: &Path, file: &Path) -> Option<String> {
        let file = clean(file);
        let rel = |dir: &str, ext: &str, prefix: &str| {
            file.strip_prefix(clean(&root.join(dir)))
                .ok()?
                .to_str()?
                .strip_suffix(ext)
                .map(|r| format!("{prefix}/{}", r.replace('\\', "/")))
        };
        rel(MATERIALS_DIR, MATERIAL_EXT, "materials")
            .or_else(|| rel(PREFABS_DIR, PREFAB_EXT, "prefabs"))
    }

    /// (Re)load one file into the library; Err keeps the previous entry.
    pub fn reload_file(&mut self, root: &Path, file: &Path) -> Result<Option<String>, String> {
        let Some(logical) = Self::logical_for_file(root, file) else {
            return Ok(None);
        };
        if !file.exists() {
            self.materials.remove(&logical);
            self.prefabs.remove(&logical);
            return Ok(Some(logical));
        }
        if logical.starts_with("materials/") {
            self.materials
                .insert(logical.clone(), MaterialDef::load(file)?);
        } else {
            self.prefabs.insert(logical.clone(), Prefab::load(file)?);
        }
        Ok(Some(logical))
    }

    /// The tree an instance expands to: the prefab root with the
    /// instance's overrides applied. Stale override targets are returned.
    pub fn expand(&self, instance: &PrefabInstance) -> Result<(Entity, Vec<String>), String> {
        let prefab = self
            .prefabs
            .get(&instance.path)
            .ok_or_else(|| format!("unknown prefab `{}`", instance.path))?;
        let mut root = prefab.root.clone();
        let stale = apply_overrides(&mut root, &instance.overrides);
        Ok((root, stale))
    }

    /// "Make instance real": replace the prefab link on the entity at
    /// `path` with a copy of the prefab tree (as children, keeping the
    /// entity's own transform and existing children).
    pub fn unpack(&self, scene: &mut Scene, path: &[usize]) -> Result<(), String> {
        let entity = scene.get_mut(path).ok_or("no entity at that path")?;
        let instance = entity
            .prefab
            .take()
            .ok_or("entity is not a prefab instance")?;
        let (root, _) = match self.expand(&instance) {
            Ok(r) => r,
            Err(e) => {
                entity.prefab = Some(instance);
                return Err(e);
            }
        };
        entity.children.insert(0, root);
        Ok(())
    }

    /// Push the per-instance overrides of the instance at `path` into its
    /// prefab (Unity's *Apply overrides*): the prefab changes for every
    /// instance, this instance's overrides are cleared. Returns the prefab's
    /// logical path and new content for the caller to save. Stale overrides
    /// (targets renamed away) are dropped and reported in the error-free
    /// path as part of the returned list.
    pub fn apply_overrides_to_prefab(
        &mut self,
        scene: &mut Scene,
        path: &[usize],
    ) -> Result<(String, Prefab, Vec<String>), String> {
        let entity = scene.get_mut(path).ok_or("no entity at that path")?;
        let instance = entity
            .prefab
            .as_mut()
            .ok_or("entity is not a prefab instance")?;
        if instance.overrides.is_empty() {
            return Err("this instance has no overrides to apply".into());
        }
        let prefab = self
            .prefabs
            .get_mut(&instance.path)
            .ok_or_else(|| format!("unknown prefab `{}`", instance.path))?;
        let stale = apply_overrides(&mut prefab.root, &instance.overrides);
        instance.overrides.clear();
        Ok((instance.path.clone(), prefab.clone(), stale))
    }

    /// Turn the entity at `path` into a new prefab `logical` (returned, for
    /// the caller to save), replacing it in the scene with an instance at
    /// the same place. The prefab root gets an identity transform; the
    /// instance keeps the entity's.
    pub fn make_prefab(
        &mut self,
        scene: &mut Scene,
        path: &[usize],
        logical: &str,
    ) -> Result<Prefab, String> {
        let entity = scene.get_mut(path).ok_or("no entity at that path")?;
        let mut root = entity.clone();
        root.transform = crate::Transform::default();
        let prefab = Prefab::new(root);
        let name = entity.name.clone();
        let transform = entity.transform;
        let mut instance = Entity::new(name);
        instance.transform = transform;
        instance.prefab = Some(PrefabInstance::new(logical));
        *entity = instance;
        self.prefabs.insert(logical.to_string(), prefab.clone());
        Ok(prefab)
    }
}

/// Drop `.` components (Path comparisons treat a leading `./` as real).
pub fn clean(p: &Path) -> PathBuf {
    p.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelComponent, Override};

    #[test]
    fn logical_paths_ignore_dot_prefixes() {
        for root in [Path::new(""), Path::new(".")] {
            for file in [
                "assets/materials/x.material.ron",
                "./assets/materials/x.material.ron",
            ] {
                assert_eq!(
                    Library::logical_for_file(root, Path::new(file)).as_deref(),
                    Some("materials/x"),
                    "{root:?} + {file}"
                );
            }
            assert_eq!(
                Library::logical_for_file(root, Path::new("prefabs/a/b.prefab.ron")).as_deref(),
                Some("prefabs/a/b")
            );
        }
        assert_eq!(
            Library::material_file(Path::new(""), "materials/x"),
            PathBuf::from("assets/materials/x.material.ron")
        );
    }

    fn tree() -> Entity {
        let mut root = Entity::new("tree");
        let mut trunk = Entity::new("trunk").with_model(ModelComponent::new("models/trunk"));
        trunk
            .children
            .push(Entity::new("leaves").with_model(ModelComponent::new("models/leaves")));
        root.children.push(trunk);
        root
    }

    #[test]
    fn apply_overrides_pushes_them_into_the_prefab() {
        let mut lib = Library::default();
        lib.prefabs
            .insert("prefabs/tree".into(), Prefab::new(tree()));
        let mut scene = Scene::new("s");
        let mut inst = Entity::new("tree-1");
        let mut pi = PrefabInstance::new("prefabs/tree");
        pi.overrides.push(crate::Override {
            target: "trunk/leaves".into(),
            tint: Some([10, 200, 10, 255]),
            ..Default::default()
        });
        pi.overrides.push(crate::Override {
            target: "gone".into(),
            visible: Some(false),
            ..Default::default()
        });
        inst.prefab = Some(pi);
        scene.entities.push(inst);
        let (logical, prefab, stale) = lib.apply_overrides_to_prefab(&mut scene, &[0]).unwrap();
        assert_eq!(logical, "prefabs/tree");
        assert_eq!(stale, vec!["gone".to_string()]);
        let leaves = &prefab.root.children[0].children[0];
        assert_eq!(
            leaves.model.as_ref().unwrap().tint,
            Some([10, 200, 10, 255])
        );
        assert_eq!(lib.prefabs["prefabs/tree"], prefab, "library updated");
        assert!(
            scene.entities[0]
                .prefab
                .as_ref()
                .unwrap()
                .overrides
                .is_empty()
        );
        assert!(
            lib.apply_overrides_to_prefab(&mut scene, &[0]).is_err(),
            "nothing left"
        );
    }

    #[test]
    fn files_round_trip_through_load() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Prefab::new(tree())
            .save(&Library::prefab_file(root, "prefabs/nature/tree"))
            .unwrap();
        MaterialDef::default()
            .save(&Library::material_file(root, "materials/wood"))
            .unwrap();
        std::fs::write(root.join("prefabs/broken.prefab.ron"), "(nope").unwrap();
        let lib = Library::load(root);
        assert!(lib.prefabs.contains_key("prefabs/nature/tree"));
        assert!(lib.materials.contains_key("materials/wood"));
        assert_eq!(lib.errors.len(), 1, "{:?}", lib.errors);
        let file = Library::prefab_file(root, "prefabs/nature/tree");
        assert_eq!(
            Library::logical_for_file(root, &file).as_deref(),
            Some("prefabs/nature/tree")
        );
    }

    #[test]
    fn overrides_apply_and_stale_ones_are_reported() {
        let mut lib = Library::default();
        lib.prefabs
            .insert("prefabs/tree".into(), Prefab::new(tree()));
        let mut inst = PrefabInstance::new("prefabs/tree");
        inst.overrides.push(Override {
            target: "trunk/leaves".into(),
            tint: Some([255, 0, 0, 255]),
            ..Override::default()
        });
        inst.overrides.push(Override {
            target: "trunk/gone".into(),
            visible: Some(false),
            ..Override::default()
        });
        let (root, stale) = lib.expand(&inst).unwrap();
        let leaves = &root.children[0].children[0];
        assert_eq!(leaves.model.as_ref().unwrap().tint, Some([255, 0, 0, 255]));
        assert_eq!(stale, vec!["trunk/gone".to_string()]);
    }

    #[test]
    fn make_prefab_then_unpack() {
        let mut lib = Library::default();
        let mut scene = Scene::new("s");
        let mut t = tree();
        t.transform.pos = [5.0, 0.0, 0.0];
        scene.entities.push(t);
        let prefab = lib.make_prefab(&mut scene, &[0], "prefabs/tree").unwrap();
        assert_eq!(prefab.root.transform, crate::Transform::default());
        let inst = &scene.entities[0];
        assert_eq!(inst.prefab.as_ref().unwrap().path, "prefabs/tree");
        assert_eq!(inst.transform.pos, [5.0, 0.0, 0.0]);
        assert!(inst.children.is_empty());

        lib.unpack(&mut scene, &[0]).unwrap();
        let e = &scene.entities[0];
        assert!(e.prefab.is_none());
        assert_eq!(e.children[0].children[0].name, "trunk");
    }
}
