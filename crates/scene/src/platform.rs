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
//!
//! The consoles' bigger hardware tiers (issue #54, ADR 0020) are targets
//! too: [`Target::N64Expanded`] (an N64 with the Expansion Pak, key
//! `n64_expanded`) and [`Target::N3dsNew`] (a New 3DS, `n3ds_new`). Each
//! follows its console's variant when it has none of its own, so the
//! resolution order is `n64_expanded` → `n64` → the entity as written.
//! They are not build targets ([`Target::ALL`] has the four builds): one ROM
//! or app holds both tiers' resolved data, and the console picks at boot.
//!
//! ```ron
//! platforms: (
//!     n64: Some((model: Some("models/tree-low"))),
//!     // The Expansion Pak's memory affords the full tree.
//!     n64_expanded: Some(()),
//! )
//! ```

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
    /// An N64 with the Expansion Pak (issue #54); falls back to the N64's
    /// variant.
    N64Expanded,
    /// A New 3DS / New 2DS XL (issue #54); falls back to the 3DS's variant.
    N3dsNew,
}

impl Target {
    /// The four build targets (one bake, one binary each).
    pub const ALL: [Target; 4] = [Target::Pc, Target::N64, Target::N3ds, Target::Web];
    /// The consoles' expanded hardware tiers, resolved inside their
    /// console's build.
    pub const TIERS: [Target; 2] = [Target::N64Expanded, Target::N3dsNew];
    /// Every target a scene resolves for: the builds, then the tiers.
    pub const EVERY: [Target; 6] = [
        Target::Pc,
        Target::N64,
        Target::N3ds,
        Target::Web,
        Target::N64Expanded,
        Target::N3dsNew,
    ];

    /// The manifest key: `pc`, `n64`, `3ds`, `web`, `n64_expanded`,
    /// `n3ds_new`.
    pub fn key(self) -> &'static str {
        match self {
            Target::Pc => "pc",
            Target::N64 => "n64",
            Target::N3ds => "3ds",
            Target::Web => "web",
            Target::N64Expanded => "n64_expanded",
            Target::N3dsNew => "n3ds_new",
        }
    }

    /// Display name: `PC`, `N64`, `3DS`, `Web`, `N64+Pak`, `New 3DS`.
    pub fn label(self) -> &'static str {
        match self {
            Target::Pc => "PC",
            Target::N64 => "N64",
            Target::N3ds => "3DS",
            Target::Web => "Web",
            Target::N64Expanded => "N64+Pak",
            Target::N3dsNew => "New 3DS",
        }
    }

    /// The hardware's full name (`N64 + Expansion Pak`; the label
    /// elsewhere).
    pub fn long_label(self) -> &'static str {
        match self {
            Target::N64Expanded => "N64 + Expansion Pak",
            Target::N3dsNew => "New 3DS",
            t => t.label(),
        }
    }

    /// 0..6 in [`Target::EVERY`] order — for per-target arrays. Arrays of
    /// baked data (one per build) index by [`base`](Self::base) instead.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The build target this one runs in: a tier's console, else itself.
    pub fn base(self) -> Target {
        match self {
            Target::N64Expanded => Target::N64,
            Target::N3dsNew => Target::N3ds,
            t => t,
        }
    }

    /// The hardware tier this target is.
    pub fn hw_tier(self) -> trino_core::HwTier {
        match self {
            Target::N64Expanded | Target::N3dsNew => trino_core::HwTier::Expanded,
            _ => trino_core::HwTier::Base,
        }
    }

    /// The console's expanded tier (`n64` → `n64_expanded`, `3ds` →
    /// `n3ds_new`); `None` for targets with one tier.
    pub fn expanded(self) -> Option<Target> {
        match self.base() {
            Target::N64 => Some(Target::N64Expanded),
            Target::N3ds => Some(Target::N3dsNew),
            _ => None,
        }
    }

    /// The same console at `tier` (PC and Web have one tier).
    pub fn with_tier(self, tier: trino_core::HwTier) -> Target {
        match tier {
            trino_core::HwTier::Base => self.base(),
            trino_core::HwTier::Expanded => self.expanded().unwrap_or(self),
        }
    }

    /// The target whose variant this one takes when it has none of its
    /// own: the PC for the Web, the console for a tier.
    pub fn fallback(self) -> Option<Target> {
        match self {
            Target::Web => Some(Target::Pc),
            Target::N64Expanded | Target::N3dsNew => Some(self.base()),
            _ => None,
        }
    }

    /// Parses a manifest key (`n3ds` is accepted for `3ds`; the tiers also
    /// as `TRINO_SIM` spells them, `n64x` and `3dsnew`).
    pub fn parse(s: &str) -> Option<Target> {
        match s.to_ascii_lowercase().as_str() {
            "pc" | "native" => Some(Target::Pc),
            "n64" => Some(Target::N64),
            "3ds" | "n3ds" => Some(Target::N3ds),
            "web" | "browser" => Some(Target::Web),
            "n64_expanded" | "n64x" => Some(Target::N64Expanded),
            "n3ds_new" | "3ds_new" | "3dsnew" | "new3ds" => Some(Target::N3dsNew),
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
    /// The N64 with the Expansion Pak's own variant; without one it takes
    /// `n64`'s. An empty `n64_expanded: Some(())` beside an `n64` variant
    /// means "as written" on the expanded console.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n64_expanded: Option<Variant>,
    /// The New 3DS's own variant; without one it takes `n3ds`'s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n3ds_new: Option<Variant>,
}

impl PlatformVariants {
    pub fn is_empty(&self) -> bool {
        Target::EVERY.iter().all(|t| self.own(*t).is_none())
    }

    /// The variant `target` draws with: its own, else its
    /// [`fallback`](Target::fallback)'s (the PC's for the Web, the
    /// console's for a tier).
    pub fn get(&self, target: Target) -> Option<&Variant> {
        self.own(target)
            .or_else(|| target.fallback().and_then(|f| self.own(f)))
    }

    /// The variant written for `target` itself, without a fallback (what
    /// a file stores; the editor counts these).
    pub fn own(&self, target: Target) -> Option<&Variant> {
        match target {
            Target::Pc => self.pc.as_ref(),
            Target::N64 => self.n64.as_ref(),
            Target::N3ds => self.n3ds.as_ref(),
            Target::Web => self.web.as_ref(),
            Target::N64Expanded => self.n64_expanded.as_ref(),
            Target::N3dsNew => self.n3ds_new.as_ref(),
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
            Target::N64Expanded => &mut self.n64_expanded,
            Target::N3dsNew => &mut self.n3ds_new,
        }
    }

    /// The other models the variants name (logical paths), in target
    /// order: assets the scene needs on some platform.
    pub fn models(&self) -> impl Iterator<Item = &str> {
        Target::EVERY
            .into_iter()
            .filter_map(|t| self.own(t)?.model.as_deref())
    }

    /// Drops variants that change nothing (the editor's "same as PC"). An
    /// empty variant of a target with a fallback (the Web, a tier) stays
    /// while the fallback has one: it is what keeps the target from
    /// following it.
    pub fn prune(&mut self) {
        for t in Target::EVERY {
            let shadows = t
                .fallback()
                .and_then(|f| self.own(f))
                .is_some_and(|v| !v.is_noop());
            if shadows {
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
        for t in Target::EVERY {
            assert_eq!(Target::parse(t.key()), Some(t));
            assert_eq!(Target::EVERY[t.index()], t);
        }
        assert_eq!(Target::parse("n3ds"), Some(Target::N3ds));
        assert_eq!(Target::parse("n64x"), Some(Target::N64Expanded));
        assert_eq!(Target::parse("3dsnew"), Some(Target::N3dsNew));
        assert_eq!(Target::parse("wii"), None);
    }

    #[test]
    fn tiers_belong_to_their_console() {
        use trino_core::HwTier;
        assert_eq!(Target::N64Expanded.base(), Target::N64);
        assert_eq!(Target::N3dsNew.base(), Target::N3ds);
        assert_eq!(Target::N64.expanded(), Some(Target::N64Expanded));
        assert_eq!(Target::N3dsNew.expanded(), Some(Target::N3dsNew));
        assert_eq!(Target::Pc.expanded(), None);
        assert_eq!(Target::N64.with_tier(HwTier::Expanded), Target::N64Expanded);
        assert_eq!(Target::N3dsNew.with_tier(HwTier::Base), Target::N3ds);
        assert_eq!(Target::Web.with_tier(HwTier::Expanded), Target::Web);
        for t in Target::ALL {
            assert_eq!(t.hw_tier(), HwTier::Base);
            assert_eq!(t.base(), t);
        }
        for t in Target::TIERS {
            assert_eq!(t.hw_tier(), HwTier::Expanded);
            assert_eq!(t.fallback(), Some(t.base()));
        }
    }

    #[test]
    fn a_tier_follows_its_console_unless_it_has_its_own() {
        let low = Variant {
            model: Some("models/tree-low".into()),
            ..Variant::default()
        };
        let mut v = PlatformVariants {
            n64: Some(low.clone()),
            ..PlatformVariants::default()
        };
        // n64_expanded -> n64 -> as written.
        assert_eq!(v.get(Target::N64Expanded), Some(&low));
        assert_eq!(v.get(Target::N3dsNew), None);
        assert_eq!(v.own(Target::N64Expanded), None);
        // An empty tier variant is "as written" and survives pruning while
        // the console has a variant to shadow.
        v.n64_expanded = Some(Variant::default());
        v.prune();
        assert_eq!(v.get(Target::N64Expanded), Some(&Variant::default()));
        assert_eq!(v.get(Target::N64), Some(&low));
        v.n64 = None;
        v.prune();
        assert!(v.n64_expanded.is_none() && v.is_empty());
        // RON spells the keys `n64_expanded` and `n3ds_new`.
        v.n3ds_new = Some(Variant {
            disabled: true,
            ..Variant::default()
        });
        v.n64_expanded = Some(low.clone());
        let text = ron::to_string(&v).unwrap();
        assert!(
            text.contains("n64_expanded") && text.contains("n3ds_new"),
            "{text}"
        );
        assert_eq!(ron::from_str::<PlatformVariants>(&text).unwrap(), v);
    }

    #[test]
    fn prune_drops_noops_only() {
        let mut v = PlatformVariants {
            pc: Some(Variant::default()),
            n64: Some(Variant {
                disabled: true,
                ..Variant::default()
            }),
            ..PlatformVariants::default()
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
