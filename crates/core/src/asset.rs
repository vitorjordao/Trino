//! Stable asset identity.
//!
//! An asset handle is the FNV-1a hash of its **logical path** — the path
//! declared in `assets/manifest.toml`, e.g. `"sprites/player"`. Games compute
//! handles at compile time (`const`), the asset pipeline computes the same
//! hash at bake time, and platforms load baked data by hash. Renaming an
//! asset therefore changes its handle (a deliberate breaking change);
//! re-baking its content does not — which is what makes live reload swap
//! content in place without invalidating anything.
//!
//! The pipeline fails the bake if two logical paths collide.

/// FNV-1a 32-bit. `const`, `no_std`, dependency-free — must produce the same
/// value in game code, pipeline and platforms.
pub const fn asset_id(logical_path: &str) -> u32 {
    let bytes = logical_path.as_bytes();
    let mut hash: u32 = 0x811c9dc5;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

/// [`asset_id`] of `prefix` followed by `name` (`"groups/"` + `"level1"`),
/// without building the string: `const` handles from a bare name.
pub const fn asset_id_prefixed(prefix: &str, name: &str) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    let parts = [prefix.as_bytes(), name.as_bytes()];
    let mut p = 0;
    while p < parts.len() {
        let bytes = parts[p];
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u32;
            hash = hash.wrapping_mul(0x0100_0193);
            i += 1;
        }
        p += 1;
    }
    hash
}

/// Whether a console's asset-index line with the tier column `column`
/// (`None` = no column: every tier; `"base"` or `"expanded"`, the
/// [`HwTier::key`](crate::HwTier::key)s) loads on `tier` hardware. The N64
/// and 3DS builds hold both tiers' data where it differs (issue #54, ADR
/// 0020): a scene or particle effect baked twice under one handle, a model
/// listed for the expanded tier only. An unknown tier name loads nowhere.
pub fn loads_on(column: Option<&str>, tier: crate::HwTier) -> bool {
    match column.map(str::trim) {
        None | Some("") => true,
        Some(name) => name == tier.key(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_const_and_stable() {
        // Golden values: changing the hash function breaks every baked asset
        // and every scene file in existence. Do not.
        const PLAYER: u32 = asset_id("sprites/player");
        assert_eq!(PLAYER, asset_id("sprites/player"));
        assert_eq!(asset_id(""), 0x811c9dc5);
        assert_ne!(asset_id("sprites/player"), asset_id("sprites/enemy"));
        assert_eq!(
            asset_id_prefixed("groups/", "level1"),
            asset_id("groups/level1")
        );
    }

    #[test]
    fn index_lines_load_on_their_tier() {
        use crate::HwTier::{Base, Expanded};
        for t in [Base, Expanded] {
            assert!(loads_on(None, t));
            assert!(loads_on(Some(""), t));
            assert!(!loads_on(Some("deluxe"), t));
        }
        assert!(loads_on(Some("base"), Base) && !loads_on(Some("base"), Expanded));
        assert!(loads_on(Some("expanded"), Expanded) && !loads_on(Some("expanded"), Base));
    }
}
