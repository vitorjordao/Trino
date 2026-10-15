//! Per-platform variants of an entity: one scene, three looks.
//!
//! A console often needs a cheaper model, a simpler material, a smaller
//! scale, or no object at all. Instead of `cfg` code in the game, the entity
//! carries a [`PlatformVariants`] block, and every consumer resolves it for
//! its [`Target`]: the editor's Game view (its simulated console), `cargo
//! xtask scene export --platform`, and the `TSCN` bake of each target.
//!
//! ```ron
//! (
//!     name: "tree",
//!     model: Some((path: "models/tree")),
//!     platforms: (
//!         n64: Some((model: Some("models/tree-low"), scale: Some((0.9, 0.9, 0.9)))),
//!         n3ds: Some((material: Some("materials/leaf-flat"))),
//!     ),
//! )
//! ```
//!
//! A platform without a variant draws the entity as written ("same as PC").

use serde::{Deserialize, Serialize};

/// The build target a scene is resolved for. Keys match the asset
/// manifest's platform names (`pc`, `n64`, `3ds`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
pub enum Target {
    #[default]
    Pc,
    N64,
    N3ds,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Pc, Target::N64, Target::N3ds];

    /// The manifest key: `pc`, `n64`, `3ds`.
    pub fn key(self) -> &'static str {
        match self {
            Target::Pc => "pc",
            Target::N64 => "n64",
            Target::N3ds => "3ds",
        }
    }

    /// Display name: `PC`, `N64`, `3DS`.
    pub fn label(self) -> &'static str {
        match self {
            Target::Pc => "PC",
            Target::N64 => "N64",
            Target::N3ds => "3DS",
        }
    }

    /// 0, 1, 2 — for per-platform arrays.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Parses a manifest key (`n3ds` is accepted for `3ds`).
    pub fn parse(s: &str) -> Option<Target> {
        match s.to_ascii_lowercase().as_str() {
            "pc" | "native" => Some(Target::Pc),
            "n64" => Some(Target::N64),
            "3ds" | "n3ds" => Some(Target::N3ds),
            _ => None,
        }
    }
}

/// An entity's variants, one per platform (`None` = same as PC). In RON
/// the 3DS field is `n3ds` (identifiers cannot start with a digit).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PlatformVariants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pc: Option<Variant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n64: Option<Variant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n3ds: Option<Variant>,
}

impl PlatformVariants {
    pub fn is_empty(&self) -> bool {
        self.pc.is_none() && self.n64.is_none() && self.n3ds.is_none()
    }

    pub fn get(&self, target: Target) -> Option<&Variant> {
        match target {
            Target::Pc => self.pc.as_ref(),
            Target::N64 => self.n64.as_ref(),
            Target::N3ds => self.n3ds.as_ref(),
        }
    }

    pub fn get_mut(&mut self, target: Target) -> &mut Option<Variant> {
        match target {
            Target::Pc => &mut self.pc,
            Target::N64 => &mut self.n64,
            Target::N3ds => &mut self.n3ds,
        }
    }

    /// Drops variants that change nothing (the editor's "same as PC").
    pub fn prune(&mut self) {
        for t in Target::ALL {
            let slot = self.get_mut(t);
            if slot.as_ref().is_some_and(Variant::is_noop) {
                *slot = None;
            }
        }
    }
}

/// What one platform changes. Every field is optional: unset fields keep
/// the entity's own value.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Variant {
    /// Another model (logical path) for the entity's `model` component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Another material for the entity's `model` component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    /// Replaces the transform's scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f32; 3]>,
    /// Overrides `visible` (false hides the entity and its subtree, like
    /// the hierarchy's eye).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// Not on this platform at all: the entity and everything under it are
    /// dropped (no draw, no collider, no node in the export).
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Variant {
    /// True when the variant changes nothing.
    pub fn is_noop(&self) -> bool {
        *self == Variant::default()
    }

    /// Whether the entity exists (and is visible) on this platform given its
    /// own `visible` flag.
    pub fn shows(variant: Option<&Variant>, visible: bool) -> bool {
        match variant {
            Some(v) if v.disabled => false,
            Some(v) => v.visible.unwrap_or(visible),
            None => visible,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        for t in Target::ALL {
            assert_eq!(Target::parse(t.key()), Some(t));
        }
        assert_eq!(Target::parse("n3ds"), Some(Target::N3ds));
        assert_eq!(Target::parse("wii"), None);
    }

    #[test]
    fn prune_drops_noops_only() {
        let mut v = PlatformVariants {
            pc: Some(Variant::default()),
            n64: Some(Variant {
                disabled: true,
                ..Variant::default()
            }),
            n3ds: None,
        };
        v.prune();
        assert!(v.pc.is_none());
        assert!(v.n64.as_ref().unwrap().disabled);
    }

    #[test]
    fn disabled_wins_over_visible() {
        let v = Variant {
            visible: Some(true),
            disabled: true,
            ..Variant::default()
        };
        assert!(!Variant::shows(Some(&v), true));
        assert!(Variant::shows(None, true));
        let hide = Variant {
            visible: Some(false),
            ..Variant::default()
        };
        assert!(!Variant::shows(Some(&hide), true));
    }
}
