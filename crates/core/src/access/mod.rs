//! Accessibility (issue #60, ADR 0026): the player's settings and the
//! engine pieces that honor them on every target, the N64 included.
//!
//! - [`AccessSettings`]: color vision, text scale, high contrast, captions,
//!   reduced motion, the flash guard, hold-to-toggle. The PC and the Web
//!   keep them for the player (`<save>.access`, `localStorage`); a console
//!   keeps them for the session, and a game that wants them across power
//!   cycles stores [`AccessSettings::pack`] (3 bytes) in its own save.
//!   [`Platform::access`](crate::Platform::access) /
//!   [`set_access`](crate::Platform::set_access).
//! - **Color vision without shaders:** semantic colors in
//!   `assets/palettes.ron` with an alternative per [`CvdMode`], baked into
//!   the `access/data` blob ([`AccessData`]). A game asks
//!   [`AccessData::color`] or [`AccessData::palette`] and passes the answer
//!   as the tint or the [`Palette`](crate::render::Palette) it already
//!   draws with: no extra draw, no shader, the same on every target.
//!   [`AccessData::icon`] gives a sprite's (or a UI node's) second channel
//!   — a pattern or a shape — shown while a mode is on.
//! - **Text and contrast:** [`UiEnv::with_access`](crate::UiEnv::with_access)
//!   scales every UI text (whole pixels where the UI scales by whole pixels)
//!   and swaps in the high-contrast theme.
//! - **Captions:** sounds declare `caption = "key"` in the manifest;
//!   [`Captions::wrap`] plays through the game's audio and queues the
//!   caption of what it plays; [`Captions::draw`] shows the queue (the 3DS
//!   may put it on the touch screen).
//! - **Motion and flashes:** [`CameraFx`] (shake, punch, FOV kick) damps
//!   itself under `reduce_motion`; [`ScreenFlash`] keeps full-screen flashes
//!   to at most [`MAX_FLASHES_PER_SECOND`] under `flash_guard`;
//!   [`FlashMonitor`] finds raw strobes (the PC's strict mode).
//! - **Input:** [`HoldToggle`] turns a held action into a toggle.

pub mod captions;
pub mod color;
pub mod data;
pub mod motion;

pub use captions::{CaptionPlace, CaptionStyle, Captioned, Captions};
pub use color::{contrast_ratio, delta_e, delta_e_seen, luminance, simulate};
pub use data::{AccessBlob, AccessData, AccessError, CaptionInfo};
pub use motion::{CameraFx, FlashMonitor, HoldToggle, MAX_FLASHES_PER_SECOND, ScreenFlash};

use core::fmt::Write;

/// A color-vision mode: which deficiency the palette swap (and the PC's
/// simulation view) serves.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(u8)]
pub enum CvdMode {
    #[default]
    Off = 0,
    /// No working L cones (red-weak to red-blind).
    Protanopia = 1,
    /// No working M cones (the most common: green-weak to green-blind).
    Deuteranopia = 2,
    /// No working S cones (blue-yellow).
    Tritanopia = 3,
    /// No color at all (achromatopsia; also a check for print and glare).
    Grayscale = 4,
}

impl CvdMode {
    pub const ALL: [CvdMode; 5] = [
        CvdMode::Off,
        CvdMode::Protanopia,
        CvdMode::Deuteranopia,
        CvdMode::Tritanopia,
        CvdMode::Grayscale,
    ];

    /// The file and CLI name.
    pub const fn key(self) -> &'static str {
        match self {
            CvdMode::Off => "off",
            CvdMode::Protanopia => "protanopia",
            CvdMode::Deuteranopia => "deuteranopia",
            CvdMode::Tritanopia => "tritanopia",
            CvdMode::Grayscale => "grayscale",
        }
    }

    /// A menu label.
    pub const fn label(self) -> &'static str {
        match self {
            CvdMode::Off => "Off",
            CvdMode::Protanopia => "Protanopia",
            CvdMode::Deuteranopia => "Deuteranopia",
            CvdMode::Tritanopia => "Tritanopia",
            CvdMode::Grayscale => "Grayscale",
        }
    }

    /// The key or a common alias (`protan`, `deutan`, `tritan`, `gray`,
    /// `achromatopsia`, `none`), any case.
    pub fn parse(s: &str) -> Option<CvdMode> {
        let s = s.trim();
        let is = |k: &str| s.eq_ignore_ascii_case(k);
        Some(if is("off") || is("none") {
            CvdMode::Off
        } else if is("protanopia") || is("protan") {
            CvdMode::Protanopia
        } else if is("deuteranopia") || is("deutan") {
            CvdMode::Deuteranopia
        } else if is("tritanopia") || is("tritan") {
            CvdMode::Tritanopia
        } else if is("grayscale") || is("greyscale") || is("gray") || is("achromatopsia") {
            CvdMode::Grayscale
        } else {
            return None;
        })
    }

    pub const fn from_index(i: u8) -> Option<CvdMode> {
        match i {
            0 => Some(CvdMode::Off),
            1 => Some(CvdMode::Protanopia),
            2 => Some(CvdMode::Deuteranopia),
            3 => Some(CvdMode::Tritanopia),
            4 => Some(CvdMode::Grayscale),
            _ => None,
        }
    }

    /// The next mode, wrapping (a menu row that cycles).
    pub const fn next(self) -> CvdMode {
        match CvdMode::from_index(self as u8 + 1) {
            Some(m) => m,
            None => CvdMode::Off,
        }
    }
}

/// Which sounds show a caption.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(u8)]
pub enum CaptionMode {
    #[default]
    Off = 0,
    /// Only sounds marked `important = true` in the manifest (a whistle, an
    /// alarm, a voice line the game depends on).
    Important = 1,
    /// Every sound with a caption.
    All = 2,
}

impl CaptionMode {
    pub const ALL: [CaptionMode; 3] = [CaptionMode::Off, CaptionMode::Important, CaptionMode::All];

    pub const fn key(self) -> &'static str {
        match self {
            CaptionMode::Off => "off",
            CaptionMode::Important => "important",
            CaptionMode::All => "all",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            CaptionMode::Off => "Off",
            CaptionMode::Important => "Important",
            CaptionMode::All => "All",
        }
    }

    pub fn parse(s: &str) -> Option<CaptionMode> {
        CaptionMode::ALL
            .into_iter()
            .find(|m| s.trim().eq_ignore_ascii_case(m.key()))
    }

    pub const fn from_index(i: u8) -> Option<CaptionMode> {
        match i {
            0 => Some(CaptionMode::Off),
            1 => Some(CaptionMode::Important),
            2 => Some(CaptionMode::All),
            _ => None,
        }
    }

    pub const fn next(self) -> CaptionMode {
        match CaptionMode::from_index(self as u8 + 1) {
            Some(m) => m,
            None => CaptionMode::Off,
        }
    }

    /// Whether a sound with this importance gets a caption.
    pub const fn shows(self, important: bool) -> bool {
        match self {
            CaptionMode::Off => false,
            CaptionMode::Important => important,
            CaptionMode::All => true,
        }
    }
}

/// The player's accessibility settings.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AccessSettings {
    /// The color-vision mode semantic palettes answer for.
    pub cvd: CvdMode,
    /// UI text size in percent, [`TEXT_SCALE_MIN`]..=[`TEXT_SCALE_MAX`].
    pub text_scale: u8,
    /// The UI's high-contrast theme; games may also outline key objects.
    pub high_contrast: bool,
    pub captions: CaptionMode,
    /// Camera shake, punches and FOV kicks damp themselves ([`CameraFx`]);
    /// games read it for their own effects.
    pub reduce_motion: bool,
    /// Full-screen flashes stay at or under [`MAX_FLASHES_PER_SECOND`] and
    /// soften ([`ScreenFlash`]). On by default.
    pub flash_guard: bool,
    /// A held action becomes a toggle ([`HoldToggle`]).
    pub hold_to_toggle: bool,
}

/// The smallest text scale, percent.
pub const TEXT_SCALE_MIN: u8 = 100;
/// The largest text scale, percent.
pub const TEXT_SCALE_MAX: u8 = 200;
/// Bytes of [`AccessSettings::pack`].
pub const PACKED_LEN: usize = 3;

impl AccessSettings {
    /// Everything off but the flash guard; text at 100 %.
    pub const DEFAULT: AccessSettings = AccessSettings {
        cvd: CvdMode::Off,
        text_scale: 100,
        high_contrast: false,
        captions: CaptionMode::Off,
        reduce_motion: false,
        flash_guard: true,
        hold_to_toggle: false,
    };

    /// The same, the text scale within its range.
    pub const fn clamped(self) -> AccessSettings {
        let t = self.text_scale;
        AccessSettings {
            text_scale: if t < TEXT_SCALE_MIN {
                TEXT_SCALE_MIN
            } else if t > TEXT_SCALE_MAX {
                TEXT_SCALE_MAX
            } else {
                t
            },
            ..self
        }
    }

    /// The text scale as a factor (1.0 ..= 2.0).
    pub fn text_factor(&self) -> f32 {
        self.clamped().text_scale as f32 / 100.0
    }

    /// Three bytes for a console save (the N64 EEPROM has 512 B or 2 KB):
    /// byte 0 = color vision (bits 0–2), captions (3–4), high contrast (5),
    /// reduce motion (6), flash guard (7); byte 1 = text scale %; byte 2 =
    /// hold-to-toggle (bit 0), the rest zero.
    pub const fn pack(&self) -> [u8; PACKED_LEN] {
        let s = self.clamped();
        [
            s.cvd as u8
                | (s.captions as u8) << 3
                | (s.high_contrast as u8) << 5
                | (s.reduce_motion as u8) << 6
                | (s.flash_guard as u8) << 7,
            s.text_scale,
            s.hold_to_toggle as u8,
        ]
    }

    /// Read [`pack`](Self::pack)'s bytes; `None` for bytes it never writes
    /// (a blank or damaged save: all zeros has no valid text scale).
    pub const fn unpack(b: [u8; PACKED_LEN]) -> Option<AccessSettings> {
        let Some(cvd) = CvdMode::from_index(b[0] & 7) else {
            return None;
        };
        let Some(captions) = CaptionMode::from_index((b[0] >> 3) & 3) else {
            return None;
        };
        if b[1] < TEXT_SCALE_MIN || b[1] > TEXT_SCALE_MAX || b[2] & !1 != 0 {
            return None;
        }
        Some(AccessSettings {
            cvd,
            text_scale: b[1],
            high_contrast: b[0] & (1 << 5) != 0,
            captions,
            reduce_motion: b[0] & (1 << 6) != 0,
            flash_guard: b[0] & (1 << 7) != 0,
            hold_to_toggle: b[2] & 1 != 0,
        })
    }

    /// The settings as `key=value` lines (the PC's `<save>.access` file,
    /// the Web's `localStorage`): every field, one per line.
    pub fn write_text(&self, out: &mut impl Write) -> core::fmt::Result {
        let on = |b: bool| if b { "on" } else { "off" };
        writeln!(out, "cvd={}", self.cvd.key())?;
        writeln!(out, "text_scale={}", self.text_scale)?;
        writeln!(out, "high_contrast={}", on(self.high_contrast))?;
        writeln!(out, "captions={}", self.captions.key())?;
        writeln!(out, "reduce_motion={}", on(self.reduce_motion))?;
        writeln!(out, "flash_guard={}", on(self.flash_guard))?;
        writeln!(out, "hold_to_toggle={}", on(self.hold_to_toggle))
    }

    /// Apply one `key=value` pair; `false` when the key is unknown or the
    /// value doesn't parse (the field keeps its value).
    pub fn set_field(&mut self, key: &str, value: &str) -> bool {
        let v = value.trim();
        let flag = |v: &str| {
            if ["on", "1", "true", "yes"]
                .iter()
                .any(|k| v.eq_ignore_ascii_case(k))
            {
                Some(true)
            } else if ["off", "0", "false", "no"]
                .iter()
                .any(|k| v.eq_ignore_ascii_case(k))
            {
                Some(false)
            } else {
                None
            }
        };
        match key.trim() {
            "cvd" | "color_vision" => CvdMode::parse(v).map(|m| self.cvd = m).is_some(),
            "text_scale" | "text" => v
                .trim_end_matches('%')
                .parse::<u16>()
                .map(|n| self.text_scale = n.min(255) as u8)
                .is_ok(),
            "high_contrast" | "contrast" => flag(v).map(|b| self.high_contrast = b).is_some(),
            "captions" => CaptionMode::parse(v).map(|m| self.captions = m).is_some(),
            "reduce_motion" => flag(v).map(|b| self.reduce_motion = b).is_some(),
            "flash_guard" => flag(v).map(|b| self.flash_guard = b).is_some(),
            "hold_to_toggle" => flag(v).map(|b| self.hold_to_toggle = b).is_some(),
            _ => false,
        }
    }

    /// Read [`write_text`](Self::write_text) lines (or the same pairs split
    /// by commas: `TRINO_ACCESS=cvd=deuteranopia,text_scale=200`) over
    /// `base`: unknown keys, broken values, blank lines and `#` comments keep
    /// `base`'s values. The result is [`clamped`](Self::clamped).
    pub fn parse_text(base: AccessSettings, text: &str) -> AccessSettings {
        let mut s = base;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            for pair in line.split(',') {
                if let Some((k, v)) = pair.split_once('=') {
                    s.set_field(k, v);
                }
            }
        }
        s.clamped()
    }
}

impl Default for AccessSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A platform's copy of the settings (every backend holds one): what
/// [`Platform::access`](crate::Platform::access) returns, what
/// [`set_access`](crate::Platform::set_access) changes at once, and whether
/// the host still has to persist a change (PC, Web).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct HostedAccess {
    settings: AccessSettings,
    changed: bool,
}

impl HostedAccess {
    pub const fn new() -> HostedAccess {
        HostedAccess {
            settings: AccessSettings::DEFAULT,
            changed: false,
        }
    }

    pub const fn get(&self) -> AccessSettings {
        self.settings
    }

    /// The game's change: applied now, persisted by the host later.
    pub fn set(&mut self, settings: AccessSettings) {
        let s = settings.clamped();
        if s != self.settings {
            self.settings = s;
            self.changed = true;
        }
    }

    /// The host's start-up value (a saved file, a forced setting): not a
    /// change to persist.
    pub fn load(&mut self, settings: AccessSettings) {
        self.settings = settings.clamped();
        self.changed = false;
    }

    /// The settings to persist, once per change.
    pub fn take_change(&mut self) -> Option<AccessSettings> {
        core::mem::take(&mut self.changed).then_some(self.settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_round_trips_every_mode_and_refuses_blank_bytes() {
        for cvd in CvdMode::ALL {
            for captions in CaptionMode::ALL {
                for bits in 0..16u8 {
                    let s = AccessSettings {
                        cvd,
                        captions,
                        text_scale: 100 + bits * 6,
                        high_contrast: bits & 1 != 0,
                        reduce_motion: bits & 2 != 0,
                        flash_guard: bits & 4 != 0,
                        hold_to_toggle: bits & 8 != 0,
                    };
                    assert_eq!(AccessSettings::unpack(s.pack()), Some(s));
                }
            }
        }
        assert_eq!(AccessSettings::unpack([0, 0, 0]), None);
        assert_eq!(AccessSettings::unpack([0xff, 0xff, 0xff]), None);
        assert_eq!(AccessSettings::unpack([5, 100, 0]), None, "cvd 5");
        assert_eq!(AccessSettings::unpack([3 << 3, 100, 0]), None, "captions 3");
        assert_eq!(AccessSettings::unpack([0, 150, 2]), None, "reserved bit");
    }

    #[test]
    fn text_round_trips_and_repairs() {
        let s = AccessSettings {
            cvd: CvdMode::Tritanopia,
            text_scale: 175,
            high_contrast: true,
            captions: CaptionMode::Important,
            reduce_motion: true,
            flash_guard: false,
            hold_to_toggle: true,
        };
        let mut text = crate::text::TextBuf::<256>::new();
        s.write_text(&mut text).unwrap();
        assert_eq!(
            AccessSettings::parse_text(AccessSettings::DEFAULT, text.as_str()),
            s
        );
        // Broken lines keep the base; out-of-range values clamp.
        let r = AccessSettings::parse_text(
            AccessSettings::DEFAULT,
            "# comment\ncvd=martian\ntext_scale=900\nbogus=1\ncaptions=all",
        );
        assert_eq!(r.cvd, CvdMode::Off);
        assert_eq!(r.text_scale, TEXT_SCALE_MAX);
        assert_eq!(r.captions, CaptionMode::All);
        // The comma form of TRINO_ACCESS.
        let e =
            AccessSettings::parse_text(AccessSettings::DEFAULT, "cvd=deutan,text=150%,contrast=on");
        assert_eq!(
            (e.cvd, e.text_scale, e.high_contrast),
            (CvdMode::Deuteranopia, 150, true)
        );
    }

    #[test]
    fn modes_cycle_and_parse_their_keys() {
        let mut m = CvdMode::Off;
        for _ in 0..CvdMode::ALL.len() {
            assert_eq!(CvdMode::parse(m.key()), Some(m));
            m = m.next();
        }
        assert_eq!(m, CvdMode::Off);
        assert_eq!(CaptionMode::All.next(), CaptionMode::Off);
        assert!(CaptionMode::Important.shows(true) && !CaptionMode::Important.shows(false));
        assert!(!CaptionMode::Off.shows(true) && CaptionMode::All.shows(false));
    }
}
