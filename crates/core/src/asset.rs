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
    }
}
