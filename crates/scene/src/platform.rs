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
//! A platform without a variant draws the entity as written ("same as PC"),
//! except the Web (issue #47), which follows the PC's variant when it has
//! none of its own: a browser is a PC-class target, so the PC's choices
//! (a heavier model, a per-pixel material) are its default too.

use serde::{Deserialize, Serialize};

/// The build target a scene is resolved for. Keys match the asset
/// manifest's platform names (`pc`, `n64`, `3ds`, `web`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
pub enum Target {
    #[default]
    Pc,
    N64,
    N3ds,
    /// A web browser (issue #47); falls back to the PC's variant.
    Web,
}

impl Target {
    pub const ALL: [Target; 4] = [Target::Pc, Target::N64, Target::N3ds, Target::Web];

    /// The manifest key: `pc`, `n64`, `3ds`, `web`.
    pub fn key(self) -> &'static str {
        match self {
            Target::Pc => "pc",
            Target::N64 => "n64",
            Target::N3ds => "3ds",
            Target::Web => "web",
        }
    }

    /// Display name: `PC`, `N64`, `3DS`, `Web`.
    pub fn label(self) -> &'static str {
        match self {
            Target::Pc => "PC",
            Target::N64 => "N64",
            Target::N3ds => "3DS",
            Target::Web => "Web",
        }
    }

    /// 0, 1, 2, 3 — for per-platform arrays.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Parses a manifest key (`n3ds` is accepted for `3ds`).
    pub fn parse(s: &str) -> Option<Target> {
        match s.to_ascii_lowercase().as_str() {
            "pc" | "native" => Some(Target::Pc),
            "n64" => Some(Target::N64),
            "3ds" | "n3ds" => Some(Target::N3ds),
            "web" | "browser" => Some(Target::Web),
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
    /// The Web's own variant; without one the Web takes `pc`'s (see the
    /// module docs). An empty `web: Some(())` beside a `pc` variant means
    /// "as written" on the Web.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<Variant>,
}

impl PlatformVariants {
    pub fn is_empty(&self) -> bool {
        self.pc.is_none() && self.n64.is_none() && self.n3ds.is_none() && self.web.is_none()
    }

    /// The variant `target` draws with: its own, and for the Web the PC's
    /// when it has none.
    pub fn get(&self, target: Target) -> Option<&Variant> {
        match target {
            Target::Pc => self.pc.as_ref(),
            Target::N64 => self.n64.as_ref(),
            Target::N3ds => self.n3ds.as_ref(),
            Target::Web => self.web.as_ref().or(self.pc.as_ref()),
        }
    }

    /// The variant written for `target` itself, without the Web's fallback
    /// to the PC (what a file stores; the editor counts these).
    pub fn own(&self, target: Target) -> Option<&Variant> {
        match target {
            Target::Pc => self.pc.as_ref(),
            Target::N64 => self.n64.as_ref(),
            Target::N3ds => self.n3ds.as_ref(),
            Target::Web => self.web.as_ref(),
        }
    }

    /// The variant written for `target` itself (no fallback): what the
    /// editor's column edits.
    pub fn get_mut(&mut self, target: Target) -> &mut Option<Variant> {
        match target {
            Target::Pc => &mut self.pc,
            Target::N64 => &mut self.n64,
            Target::N3ds => &mut self.n3ds,
            Target::Web => &mut self.web,
        }
    }

    /// Drops variants that change nothing (the editor's "same as PC"). An
    /// empty Web variant stays while the PC has one: it is what keeps the
    /// Web from following it.
    pub fn prune(&mut self) {
        for t in Target::ALL {
            let pc_changes = self.pc.as_ref().is_some_and(|v| !v.is_noop());
            if t == Target::Web && pc_changes {
                continue;
            }
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
            web: None,
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

    #[test]
    fn the_web_follows_the_pc_unless_it_has_its_own() {
        let low = Variant {
            model: Some("models/tree-low".into()),
            ..Variant::default()
        };
        let mut v = PlatformVariants {
            pc: Some(low.clone()),
            ..PlatformVariants::default()
        };
        assert_eq!(v.get(Target::Web), Some(&low));
        assert_eq!(v.get(Target::N64), None);
        // An empty Web variant means "as written" and survives pruning.
        v.web = Some(Variant::default());
        v.prune();
        assert_eq!(v.get(Target::Web), Some(&Variant::default()));
        // Without a PC variant to shadow, it is a no-op and goes.
        v.pc = None;
        v.prune();
        assert!(v.web.is_none() && v.is_empty());
        // RON spells it `web`.
        v.web = Some(low.clone());
        let text = ron::to_string(&v).unwrap();
        assert!(text.contains("web"), "{text}");
        assert_eq!(ron::from_str::<PlatformVariants>(&text).unwrap(), v);
    }
}
