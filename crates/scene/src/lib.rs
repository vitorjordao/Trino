//! # trino-scene
//!
//! The scene file format: **versioned RON**, defined before anything
//! depends on it (editor, runtime spawning, tests) so it never has to be
//! retrofitted. Text-based on purpose — scenes must diff and merge in PRs.
//!
//! ## Versioning contract
//!
//! - `version` is the first field of every scene file.
//! - Loaders accept the current version and migrate older ones in
//!   [`Scene::from_ron`] (none exist yet; the harness is in place).
//! - Removing/renaming a field or changing its meaning bumps the version
//!   and adds a migration. Adding an optional field with a default does not.
//!
//! Sprites are referenced by **logical asset path** (`sprites/player`), the
//! same identity the pipeline and `SpriteId::from_path` use.

use serde::{Deserialize, Serialize};
use trino_core::SpriteId;

pub const SCENE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Scene {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub entities: Vec<Entity>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entity {
    pub name: String,
    #[serde(default)]
    pub transform: Transform2D,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<SpriteComponent>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Transform2D {
    pub pos: [f32; 2],
    pub scale: [f32; 2],
    /// Radians.
    pub rotation: f32,
}

impl Default for Transform2D {
    fn default() -> Self {
        Transform2D {
            pos: [0.0, 0.0],
            scale: [1.0, 1.0],
            rotation: 0.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SpriteComponent {
    /// Logical asset path, e.g. `"sprites/player"`.
    pub path: String,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
    #[serde(default = "white")]
    pub tint: [u8; 4],
}

fn white() -> [u8; 4] {
    [255, 255, 255, 255]
}

impl SpriteComponent {
    pub fn new(path: impl Into<String>) -> Self {
        SpriteComponent {
            path: path.into(),
            flip_x: false,
            flip_y: false,
            tint: white(),
        }
    }

    pub fn sprite_id(&self) -> SpriteId {
        SpriteId(trino_core::asset_id(&self.path))
    }
}

impl Scene {
    pub fn new(name: impl Into<String>) -> Self {
        Scene {
            version: SCENE_VERSION,
            name: name.into(),
            entities: Vec::new(),
        }
    }

    /// Parse + migrate. The single entry point for reading scenes.
    pub fn from_ron(text: &str) -> Result<Scene, String> {
        // Peek the version before full parsing so future migrations can
        // route to old schemas.
        let versioned: VersionOnly =
            ron::from_str(text).map_err(|e| format!("scene parse error: {e}"))?;
        match versioned.version {
            SCENE_VERSION => ron::from_str(text).map_err(|e| format!("scene parse error: {e}")),
            older if older < SCENE_VERSION => Err(format!(
                "scene version {older} has no migration path (current {SCENE_VERSION}) — \
                 this is a bug: migrations must exist for every released version"
            )),
            newer => Err(format!(
                "scene version {newer} is newer than this engine understands \
                 ({SCENE_VERSION}) — update Trino"
            )),
        }
    }

    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .expect("scene serialization cannot fail")
    }

    pub fn load(path: &std::path::Path) -> Result<Scene, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_ron(&text)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(path, self.to_ron())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

#[derive(Deserialize)]
struct VersionOnly {
    version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Scene {
        let mut scene = Scene::new("level-1");
        scene.entities.push(Entity {
            name: "player".into(),
            transform: Transform2D {
                pos: [144.0, 104.0],
                scale: [1.0, 1.0],
                rotation: 0.0,
            },
            sprite: Some(SpriteComponent::new("sprites/player")),
        });
        scene.entities.push(Entity {
            name: "marker".into(),
            transform: Transform2D::default(),
            sprite: None,
        });
        scene
    }

    #[test]
    fn round_trip_is_lossless() {
        let scene = sample();
        let text = scene.to_ron();
        let back = Scene::from_ron(&text).unwrap();
        assert_eq!(scene, back);
        // And stable: serializing again yields identical text (diffable).
        assert_eq!(text, back.to_ron());
    }

    #[test]
    fn missing_optional_fields_get_defaults() {
        let text = r#"(
            version: 1,
            name: "min",
            entities: [(name: "e")],
        )"#;
        let scene = Scene::from_ron(text).unwrap();
        assert_eq!(scene.entities[0].transform, Transform2D::default());
        assert!(scene.entities[0].sprite.is_none());
    }

    #[test]
    fn newer_version_is_rejected_with_guidance() {
        let text = r#"(version: 99, name: "future", entities: [])"#;
        let err = Scene::from_ron(text).unwrap_err();
        assert!(err.contains("update Trino"), "{err}");
    }

    #[test]
    fn sprite_id_matches_core_hash() {
        let sprite = SpriteComponent::new("sprites/player");
        assert_eq!(sprite.sprite_id(), SpriteId::from_path("sprites/player"));
    }
}
