//! The project's collision layers (issue #66): `assets/physics.ron` names
//! up to 16 layers and says which pairs do **not** collide — the layer
//! matrix the editor's Project tab edits and `cargo xtask physics layers`
//! prints and sets. Scenes name a collider's layer (`layer: "pickup"`);
//! resolve turns the name into its bit, and the bake writes the matrix as
//! the `physics/layers` blob (`trino_core::physics::LayerMatrix`).
//!
//! ```ron
//! (
//!     version: 1,
//!     layers: ["world", "player", "enemy", "pickup"],
//!     ignore: [("pickup", "enemy")],
//! )
//! ```
//!
//! Layer 0 is the default: a collider without `layer` is on it. A project
//! without the file has one layer, `world`.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub const PHYSICS_FILE: &str = "assets/physics.ron";
pub const PHYSICS_VERSION: u32 = 1;
/// The blob the bake writes the matrix to.
pub const LAYERS_BLOB_PATH: &str = "physics/layers";
pub const MAX_LAYERS: usize = 16;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PhysicsSettings {
    pub version: u32,
    /// Layer names, bit 0 first.
    pub layers: Vec<String>,
    /// Pairs of layers that pass through each other.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<(String, String)>,
}

impl Default for PhysicsSettings {
    fn default() -> Self {
        PhysicsSettings {
            version: PHYSICS_VERSION,
            layers: vec!["world".into()],
            ignore: Vec::new(),
        }
    }
}

impl PhysicsSettings {
    /// The project's settings, the defaults without a file.
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join(PHYSICS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_ron(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(_) => Ok(Self::default()),
        }
    }

    pub fn from_ron(text: &str) -> Result<Self, String> {
        let s: PhysicsSettings = ron::from_str(text).map_err(|e| format!("parse error: {e}"))?;
        if s.version != PHYSICS_VERSION {
            return Err(format!(
                "physics settings version {} (this engine reads {PHYSICS_VERSION})",
                s.version
            ));
        }
        let problems = s.problems();
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        Ok(s)
    }

    pub fn to_ron(&self) -> String {
        let pretty = ron::ser::PrettyConfig::new().struct_names(false);
        let mut text = ron::ser::to_string_pretty(self, pretty).expect("plain data");
        text.push('\n');
        text
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        let path = root.join(PHYSICS_FILE);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, self.to_ron()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// What is wrong with it (empty when nothing).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.layers.is_empty() {
            out.push("no layers: name at least one (`world`)".into());
        }
        if self.layers.len() > MAX_LAYERS {
            out.push(format!(
                "{} layers: at most {MAX_LAYERS} (a layer is a bit of a u16 mask)",
                self.layers.len()
            ));
        }
        for (i, l) in self.layers.iter().enumerate() {
            if l.is_empty() || l.contains(char::is_whitespace) {
                out.push(format!("layer {i}: `{l}` needs a name without spaces"));
            }
            if self.layers[..i].contains(l) {
                out.push(format!("layer `{l}` is named twice"));
            }
        }
        for (a, b) in &self.ignore {
            for n in [a, b] {
                if !self.layers.contains(n) {
                    out.push(format!("ignore ({a}, {b}): no layer `{n}`"));
                }
            }
        }
        out
    }

    /// The bit of layer `name`.
    pub fn bit(&self, name: &str) -> Option<u16> {
        self.layers
            .iter()
            .position(|l| l == name)
            .filter(|&i| i < MAX_LAYERS)
            .map(|i| 1u16 << i)
    }

    /// Do layers `a` and `b` (names) collide?
    pub fn collide(&self, a: &str, b: &str) -> bool {
        !self
            .ignore
            .iter()
            .any(|(x, y)| (x == a && y == b) || (x == b && y == a))
    }

    /// Set whether `a` and `b` collide (the editor's checkbox).
    pub fn set_collide(&mut self, a: &str, b: &str, on: bool) {
        self.ignore
            .retain(|(x, y)| !((x == a && y == b) || (x == b && y == a)));
        if !on {
            let (x, y) = if a <= b { (a, b) } else { (b, a) };
            self.ignore.push((x.into(), y.into()));
            self.ignore.sort();
        }
    }

    /// The mask a body on each layer queries with, bit 0 first.
    pub fn masks(&self) -> Vec<u16> {
        let n = self.layers.len().min(MAX_LAYERS);
        (0..n)
            .map(|i| {
                (0..n)
                    .filter(|&j| self.collide(&self.layers[i], &self.layers[j]))
                    .fold(0u16, |m, j| m | (1 << j))
            })
            .collect()
    }

    /// The `physics/layers` blob (`TPHY`, read by
    /// `trino_core::physics::LayerMatrix`).
    pub fn to_blob(&self) -> Vec<u8> {
        let masks = self.masks();
        let mut b = Vec::with_capacity(8 + masks.len() * 2);
        b.extend_from_slice(trino_core::physics::LAYERS_MAGIC);
        b.extend_from_slice(&trino_core::physics::LAYERS_VERSION.to_le_bytes());
        b.extend_from_slice(&(masks.len() as u16).to_le_bytes());
        for m in masks {
            b.extend_from_slice(&m.to_le_bytes());
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_round_trip_and_make_masks() {
        let text = r#"(version: 1, layers: ["world", "player", "enemy", "pickup"],
            ignore: [("pickup", "enemy")])"#;
        let mut s = PhysicsSettings::from_ron(text).unwrap();
        assert_eq!(s.bit("pickup"), Some(8));
        assert_eq!(s.bit("nope"), None);
        assert!(!s.collide("enemy", "pickup"));
        assert_eq!(s.masks(), vec![0b1111, 0b1111, 0b0111, 0b1011]);
        s.set_collide("enemy", "pickup", true);
        assert!(s.ignore.is_empty());
        s.set_collide("player", "world", false);
        assert_eq!(s.ignore, vec![("player".into(), "world".into())]);
        let back = PhysicsSettings::from_ron(&s.to_ron()).unwrap();
        assert_eq!(back, s);
        let blob = s.to_blob();
        let m = trino_core::physics::LayerMatrix::parse(&blob).unwrap();
        // player × world now pass through each other.
        assert_eq!(m.mask(1), 0b1110);
        assert_eq!(m.mask(2), 0b1111);
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn bad_settings_say_why() {
        let e = PhysicsSettings::from_ron(r#"(version: 1, layers: ["a", "a"])"#).unwrap_err();
        assert!(e.contains("named twice"), "{e}");
        let e = PhysicsSettings::from_ron(r#"(version: 1, layers: ["a"], ignore: [("a", "b")])"#)
            .unwrap_err();
        assert!(e.contains("no layer `b`"), "{e}");
        let e = PhysicsSettings::from_ron(r#"(version: 2, layers: ["a"])"#).unwrap_err();
        assert!(e.contains("version 2"), "{e}");
        assert_eq!(PhysicsSettings::default().bit("world"), Some(1));
    }
}
