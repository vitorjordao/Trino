//! The hardware tier the boot detects (issue #54, ADR 0020): the shim's
//! packed probe → [`Probe`] → [`HwTier`] and [`Caps`] (pure, so host tests
//! check the mapping without a console).
//!
//! On a New 3DS / New 2DS XL the boot turns the speedup on
//! (`osSetSpeedupEnable`: 804 MHz and the 2 MB L2 cache) unless the project
//! opted out (`trino.toml` `[3ds] speedup = false`, a `no_speedup` file in
//! the RomFS). The tier is `Expanded` only with the speedup on: a game that
//! opted out runs at the old 3DS's clock and gets the old 3DS's table, so
//! its timing is the same on every model.

use trino_core::{Caps, HwTier};

/// The RomFS file whose presence turns the New 3DS speedup off.
pub const NO_SPEEDUP_FILE: &str = "/no_speedup";

/// What the boot found (`trino_hw_probe`'s word: bit 0 New 3DS, bit 1
/// speedup on, bit 2 no stereo screen (an old 2DS or a New 2DS XL, issue
/// #63), bits 8.. the application memory region in MiB).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Probe {
    pub new3ds: bool,
    pub speedup: bool,
    /// A 3D (stereoscopic) top screen: not on an old 2DS or a New 2DS XL.
    pub stereo: bool,
    /// The application memory region the app got, MiB.
    pub app_mib: u32,
}

impl Probe {
    pub fn decode(word: u32) -> Probe {
        Probe {
            new3ds: word & 1 != 0,
            speedup: word & 2 != 0,
            stereo: word & 4 == 0,
            app_mib: word >> 8,
        }
    }

    pub fn tier(self) -> HwTier {
        if self.new3ds && self.speedup {
            HwTier::Expanded
        } else {
            HwTier::Base
        }
    }

    pub fn caps(self) -> Caps {
        Caps {
            stereo: self.stereo,
            ..Caps::N3DS.with_tier(self.tier())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_3ds_with_the_speedup_is_the_expanded_tier() {
        let p = Probe::decode(1 | 2 | (64 << 8));
        assert_eq!(
            p,
            Probe {
                new3ds: true,
                speedup: true,
                stereo: true,
                app_mib: 64
            }
        );
        assert_eq!((p.tier(), p.caps()), (HwTier::Expanded, Caps::N3DS_NEW));
        // An old 3DS never gets the speedup; a New 3DS that opted out runs
        // at the old clock with the old table.
        for word in [64 << 8, 1 | (64 << 8)] {
            let p = Probe::decode(word);
            assert_eq!((p.tier(), p.caps()), (HwTier::Base, Caps::N3DS));
            assert!(!p.speedup);
        }
    }

    #[test]
    fn a_2ds_has_no_stereo_screen() {
        let p = Probe::decode(4 | (64 << 8));
        assert!(!p.stereo && !p.caps().stereo);
        assert_eq!(p.app_mib, 64);
        assert!(Probe::decode(64 << 8).caps().stereo);
    }
}
