//! The hardware tier the boot detects (issue #54, ADR 0020): RDRAM size →
//! [`HwTier`] and its [`Caps`] (pure, so host tests check the rule without
//! a console).
//!
//! An Expansion Pak doubles RDRAM to 8 MiB. libdragon's
//! `is_memory_expanded()` calls anything from 0x7C0000 bytes up expanded
//! (the IPL3 may keep a few KB at the top), and so does Trino. The heap
//! grows into the extra memory by itself (libdragon's `sbrk` ends at
//! `get_memory_size()`).

use trino_core::{Caps, HwTier};

/// libdragon's `is_memory_expanded` threshold.
pub const EXPANDED_FROM: u32 = 0x7C_0000;

/// The tier of an N64 with `ram` bytes of RDRAM.
pub fn tier_of_memory(ram: u32) -> HwTier {
    if ram >= EXPANDED_FROM {
        HwTier::Expanded
    } else {
        HwTier::Base
    }
}

/// The renderer's table for that N64.
pub fn caps_of_memory(ram: u32) -> Caps {
    Caps::N64.with_tier(tier_of_memory(ram))
}

/// RDRAM in whole MiB, rounded to the nearest (`ram=8M` in the boot log).
pub fn megabytes(ram: u32) -> u32 {
    (ram + (1 << 19)) >> 20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_mib_is_base_and_an_expansion_pak_is_expanded() {
        assert_eq!(tier_of_memory(4 << 20), HwTier::Base);
        assert_eq!(tier_of_memory(8 << 20), HwTier::Expanded);
        // The IPL3 can keep the top few KB: still an Expansion Pak.
        assert_eq!(tier_of_memory(0x7C_0000), HwTier::Expanded);
        assert_eq!(tier_of_memory(0x7B_FFFF), HwTier::Base);
        assert_eq!(caps_of_memory(4 << 20), Caps::N64);
        assert_eq!(caps_of_memory(8 << 20), Caps::N64_EXPANDED);
        assert_eq!((megabytes(4 << 20), megabytes(0x7C_0000)), (4, 8));
    }
}
