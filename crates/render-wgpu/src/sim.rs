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
    /// A browser (issue #47): the native profile's renderer and picture
    /// (the GPU T&L path, per-pixel lights, SSAO), with the Web's budgets
    /// ([`Caps::WEB`]). `trino-platform-web` renders with it, and the
    /// editor's Game view simulates a browser with it.
    Web,
}

impl SimProfile {
    pub fn caps(self) -> Caps {
        match self {
            SimProfile::Native => Caps::PC,
            SimProfile::N64 => Caps::N64,
            SimProfile::N3ds => Caps::N3DS,
            SimProfile::Web => Caps::WEB,
        }
    }

    /// Internal framebuffer resolution.
    pub fn internal_resolution(self) -> (u32, u32) {
        match self {
            SimProfile::Native => (640, 480),
            SimProfile::N64 => (320, 240),
            SimProfile::N3ds => (400, 240),
            SimProfile::Web => (640, 480),
        }
    }

    /// The PC's own picture rather than a console's: the native profile and
    /// the Web, which renders like it (resolution from the output, MSAA,
    /// the GPU T&L path, per-pixel lights). The console profiles keep their
    /// console's look.
    pub fn is_native(self) -> bool {
        matches!(self, SimProfile::Native | SimProfile::Web)
    }

    /// Parse from `TRINO_SIM` env values / CLI flags.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "native" | "pc" => Some(SimProfile::Native),
            "n64" => Some(SimProfile::N64),
            "3ds" | "n3ds" => Some(SimProfile::N3ds),
            "web" | "browser" => Some(SimProfile::Web),
            _ => None,
        }
    }
}

/// Widest picture the native profile renders before pillarboxing (64:27,
/// "21:9"): a 32:9 monitor shows the 21:9 picture with black side bars.
pub const MAX_ASPECT: f32 = 64.0 / 27.0;
/// Narrowest (4:3): a taller window gets bars above and below.
pub const MIN_ASPECT: f32 = 4.0 / 3.0;
/// Fewest lines the native profile renders (the interface is laid out for
/// at least 240).
pub const MIN_LINES: u32 = 240;

/// The auto pixel scale for an output `height`: the largest integer that
/// keeps at least 300 lines (1080p → 3 = 360 lines, 1440p → 4, 4K → 7).
pub fn auto_pixel_scale(height: u32) -> u32 {
    (height / 300).max(1)
}

/// The native profile's internal resolution for an output of `output`
/// physical pixels at pixel `scale` (0 = [`auto_pixel_scale`]): the output
/// divided by the scale, the aspect kept within
/// [`MIN_ASPECT`]..=[`MAX_ASPECT`] (the rest becomes black bars when
/// presented), and never under [`MIN_LINES`] lines (the scale drops
/// instead). Returns `(width, height, scale)`; `width × scale` and
/// `height × scale` always fit the output, so the upscale is a whole
/// number.
pub fn native_internal(output: (u32, u32), scale: u32) -> (u32, u32, u32) {
    let (ow, oh) = (output.0.max(1), output.1.max(1));
    let mut k = if scale == 0 {
        auto_pixel_scale(oh)
    } else {
        scale
    };
    // Keep the interface's minimum height (and width at 4:3).
    while k > 1 && (oh / k < MIN_LINES || ow / k < MIN_LINES * 4 / 3) {
        k -= 1;
    }
    let (w, h) = (ow / k, oh / k);
    let w_max = (h as f32 * MAX_ASPECT) as u32;
    let h_max = (w as f32 / MIN_ASPECT) as u32;
    // Even sizes keep centered pictures on whole pixels.
    let w = w.min(w_max) & !1;
    let h = h.min(h_max) & !1;
    (w.max(2), h.max(2), k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_internal_fits_and_clamps_the_aspect() {
        // 1080p: 3× → 640×360, fills the screen exactly.
        assert_eq!(native_internal((1920, 1080), 0), (640, 360, 3));
        // 1440p and 4K keep about 300–360 lines.
        assert_eq!(native_internal((2560, 1440), 0), (640, 360, 4));
        assert_eq!(native_internal((3840, 2160), 0), (548, 308, 7));
        // Ultrawide 21:9 fills; 32:9 is clamped to 21:9 (side bars).
        assert_eq!(native_internal((2560, 1080), 0), (852, 360, 3));
        let (w, h, k) = native_internal((3840, 1080), 0);
        assert_eq!((h, k), (360, 3));
        assert!((w as f32 / h as f32 - MAX_ASPECT).abs() < 0.01);
        assert!(w * k < 3840);
        // 5:4 is clamped to 4:3 (bars above and below).
        assert_eq!(native_internal((1280, 1024), 1), (1280, 960, 1));
        // A chosen scale; too big a scale drops to keep 240 lines.
        assert_eq!(native_internal((1920, 1080), 1), (1920, 1080, 1));
        assert_eq!(native_internal((1280, 720), 6), (426, 240, 3));
        // A small window still renders something sane.
        assert_eq!(native_internal((640, 360), 0), (640, 360, 1));
    }

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
        assert_eq!(SimProfile::parse("web"), Some(SimProfile::Web));
        assert_eq!(SimProfile::parse("dreamcast"), None);
    }
}
