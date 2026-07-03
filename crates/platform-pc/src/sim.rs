//! Console-simulation profiles: which console the PC pretends to be.
//!
//! Fase 1 gives each profile its internal resolution and `Caps`. Fase 4/5
//! add the visual emulation on top (N64 3-point filtering, RGBA5551 dither,
//! VI stage) and strict-mode validation against `Caps`.

use trino_core::Caps;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SimProfile {
    /// No console simulation. Still renders through the offscreen target,
    /// at a fixed 640x480 for now (native-resolution mode comes with the
    /// editor viewport in Fase 3).
    #[default]
    Native,
    /// 320x240, N64 budgets.
    N64,
    /// 400x240 (3DS top screen), 3DS budgets.
    N3ds,
}

impl SimProfile {
    pub fn caps(self) -> Caps {
        match self {
            SimProfile::Native => Caps::PC,
            SimProfile::N64 => Caps::N64,
            SimProfile::N3ds => Caps::N3DS,
        }
    }

    /// Internal framebuffer resolution.
    pub fn internal_resolution(self) -> (u32, u32) {
        match self {
            SimProfile::Native => (640, 480),
            SimProfile::N64 => (320, 240),
            SimProfile::N3ds => (400, 240),
        }
    }

    /// Parse from `TRINO_SIM` env values / CLI flags.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "native" | "pc" => Some(SimProfile::Native),
            "n64" => Some(SimProfile::N64),
            "3ds" | "n3ds" => Some(SimProfile::N3ds),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_match_console_specs() {
        assert_eq!(SimProfile::N64.internal_resolution(), (320, 240));
        assert_eq!(SimProfile::N3ds.internal_resolution(), (400, 240));
        assert_eq!(SimProfile::N64.caps().texture_memory_bytes, 4096);
    }

    #[test]
    fn parse_accepts_aliases() {
        assert_eq!(SimProfile::parse("N64"), Some(SimProfile::N64));
        assert_eq!(SimProfile::parse("3ds"), Some(SimProfile::N3ds));
        assert_eq!(SimProfile::parse("pc"), Some(SimProfile::Native));
        assert_eq!(SimProfile::parse("dreamcast"), None);
    }
}
