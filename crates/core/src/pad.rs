//! Gamepad families (issue #86 item 7): which kind of pad the player holds
//! on the PC and the Web, so a game can print the buttons as they are
//! printed on it — Xbox letters in their colors, PlayStation's symbols,
//! Nintendo's letters in their swapped places — and draw a matching
//! diagram.
//!
//! - [`PadFamily`]: the family, detected by the host from the pad's USB
//!   vendor and product ids ([`PadFamily::from_usb`]) and its name
//!   ([`PadFamily::from_name`]); [`PadFamily::detect`] combines them, ids
//!   first. Pure functions over strings and numbers: the PC (gilrs) and the
//!   Web (`Gamepad.id`) hosts feed them, and tests run them without a pad.
//! - [`PadSlot`]: a button by its *position* on a standard pad (the bottom
//!   face button, the left bumper). The engine's [`Button`]s are positional
//!   on the PC and the Web: `Button::A` is the bottom face button whatever
//!   it is labelled ([`PadSlot::standard`] — the default bindings of both
//!   hosts).
//! - [`PadGlyph`]: what a family prints at a slot ([`PadFamily::glyph`]):
//!   a short label, a spelled-out name, the accent color and, for
//!   PlayStation's face buttons, the symbol's shape. Labels are ASCII
//!   (uppercase, digits, `+`, `-`), so every bitmap font can print them.
//!
//! The platform reports the connected pad's family through
//! [`crate::Platform::gamepad_family`] (`None` on the consoles, whose
//! buttons are fixed and known to the game, and when no pad is connected).

use crate::input::Button;
use crate::math::Color;

/// A kind of gamepad, by the glyphs printed on it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PadFamily {
    /// Xbox 360 / One / Series and XInput pads: A/B/X/Y, colored.
    Xbox,
    /// DualShock 4 and DualSense: cross, circle, square, triangle.
    PlayStation,
    /// Switch Pro Controller and Joy-Cons: A/B and X/Y in swapped places.
    Nintendo,
    /// 8BitDo pads in their own (Switch/D-input) modes: Nintendo's places,
    /// SNES colors.
    EightBitDo,
    /// Anything else: the standard positions with Xbox letters, uncolored.
    Generic,
}

impl PadFamily {
    pub const ALL: [PadFamily; 5] = [
        PadFamily::Xbox,
        PadFamily::PlayStation,
        PadFamily::Nintendo,
        PadFamily::EightBitDo,
        PadFamily::Generic,
    ];

    /// `xbox` / `playstation` / `nintendo` / `8bitdo` / `generic`.
    pub const fn key(self) -> &'static str {
        match self {
            PadFamily::Xbox => "xbox",
            PadFamily::PlayStation => "playstation",
            PadFamily::Nintendo => "nintendo",
            PadFamily::EightBitDo => "8bitdo",
            PadFamily::Generic => "generic",
        }
    }

    /// For a screen (ASCII uppercase).
    pub const fn label(self) -> &'static str {
        match self {
            PadFamily::Xbox => "XBOX",
            PadFamily::PlayStation => "PLAYSTATION",
            PadFamily::Nintendo => "NINTENDO",
            PadFamily::EightBitDo => "8BITDO",
            PadFamily::Generic => "GAMEPAD",
        }
    }

    pub fn parse(s: &str) -> Option<PadFamily> {
        PadFamily::ALL
            .into_iter()
            .find(|f| s.trim().eq_ignore_ascii_case(f.key()))
    }

    /// The family of a USB vendor (and product) id, where the vendor makes
    /// one family: Microsoft `045e`, Sony `054c` (DualShock 4 `05c4`/`09cc`,
    /// DualSense `0ce6`/`0df2`), Nintendo `057e` (Switch Pro `2009`,
    /// Joy-Cons `2006`/`2007`), 8BitDo `2dc8`. Third parties that make pads
    /// of several families (PDP, PowerA, Hori, Logitech) are `None`: their
    /// name decides. An 8BitDo pad in XInput mode reports Microsoft's ids
    /// (`045e:028e`) and in Switch mode Nintendo's (`057e:2009`): it then
    /// is, and labels its buttons as, that family.
    pub fn from_usb(vendor: u16, product: u16) -> Option<PadFamily> {
        let _ = product;
        match vendor {
            0x045e => Some(PadFamily::Xbox),
            0x054c => Some(PadFamily::PlayStation),
            0x057e => Some(PadFamily::Nintendo),
            0x2dc8 => Some(PadFamily::EightBitDo),
            _ => None,
        }
    }

    /// The family a pad's name names, case-insensitively ("Xbox Wireless
    /// Controller", "DualSense Wireless Controller", "Pro Controller",
    /// "8BitDo SN30 Pro", Firefox-on-Windows' bare "xinput"). `None` when
    /// it names none.
    pub fn from_name(name: &str) -> Option<PadFamily> {
        const WORDS: [(&str, PadFamily); 16] = [
            // 8BitDo first: "8BitDo Pro 2" must not read as a Nintendo pad.
            ("8bitdo", PadFamily::EightBitDo),
            ("8bit do", PadFamily::EightBitDo),
            ("xbox", PadFamily::Xbox),
            ("x-box", PadFamily::Xbox),
            ("xinput", PadFamily::Xbox),
            ("microsoft", PadFamily::Xbox),
            ("dualsense", PadFamily::PlayStation),
            ("dualshock", PadFamily::PlayStation),
            ("playstation", PadFamily::PlayStation),
            ("ps5 ", PadFamily::PlayStation),
            ("ps4 ", PadFamily::PlayStation),
            ("sony", PadFamily::PlayStation),
            ("pro controller", PadFamily::Nintendo),
            ("joy-con", PadFamily::Nintendo),
            ("nintendo", PadFamily::Nintendo),
            ("switch", PadFamily::Nintendo),
        ];
        WORDS
            .iter()
            .find(|(w, _)| contains_ignore_case(name, w) || ends_with_word(name, w))
            .map(|(_, f)| *f)
    }

    /// A pad's family from what its host knows: the USB ids where they name
    /// a family, else the name, else [`PadFamily::Generic`].
    pub fn detect(name: &str, usb: Option<(u16, u16)>) -> PadFamily {
        usb.and_then(|(v, p)| PadFamily::from_usb(v, p))
            .or_else(|| PadFamily::from_name(name))
            .unwrap_or(PadFamily::Generic)
    }

    /// What this family prints at `slot`.
    pub const fn glyph(self, slot: PadSlot) -> PadGlyph {
        use PadFamily as F;
        use PadSlot as S;
        // Face buttons by position (south, east, west, north).
        let face = match self {
            F::Xbox => [
                PadGlyph::letter("A", XBOX_GREEN),
                PadGlyph::letter("B", XBOX_RED),
                PadGlyph::letter("X", XBOX_BLUE),
                PadGlyph::letter("Y", XBOX_YELLOW),
            ],
            F::PlayStation => [
                PadGlyph::symbol("CROSS", PS_BLUE, GlyphShape::Cross),
                PadGlyph::symbol("CIRCLE", PS_RED, GlyphShape::Circle),
                PadGlyph::symbol("SQUARE", PS_PINK, GlyphShape::Square),
                PadGlyph::symbol("TRIANGLE", PS_GREEN, GlyphShape::Triangle),
            ],
            F::Nintendo => [
                PadGlyph::letter("B", NEUTRAL),
                PadGlyph::letter("A", NEUTRAL),
                PadGlyph::letter("Y", NEUTRAL),
                PadGlyph::letter("X", NEUTRAL),
            ],
            F::EightBitDo => [
                PadGlyph::letter("B", SNES_YELLOW),
                PadGlyph::letter("A", SNES_RED),
                PadGlyph::letter("Y", SNES_GREEN),
                PadGlyph::letter("X", SNES_BLUE),
            ],
            F::Generic => [
                PadGlyph::letter("A", NEUTRAL),
                PadGlyph::letter("B", NEUTRAL),
                PadGlyph::letter("X", NEUTRAL),
                PadGlyph::letter("Y", NEUTRAL),
            ],
        };
        // Shoulders, triggers, sticks, start, select, home.
        let small: [&'static str; 9] = match self {
            F::Xbox => ["LB", "RB", "LT", "RT", "LS", "RS", "MENU", "VIEW", "XBOX"],
            F::PlayStation => ["L1", "R1", "L2", "R2", "L3", "R3", "OPTIONS", "SHARE", "PS"],
            F::Nintendo => ["L", "R", "ZL", "ZR", "LS", "RS", "+", "-", "HOME"],
            F::EightBitDo => ["L", "R", "L2", "R2", "L3", "R3", "START", "SELECT", "HOME"],
            F::Generic => [
                "L1", "R1", "L2", "R2", "L3", "R3", "START", "SELECT", "HOME",
            ],
        };
        match slot {
            S::South => face[0],
            S::East => face[1],
            S::West => face[2],
            S::North => face[3],
            S::L1 => PadGlyph::small(small[0]),
            S::R1 => PadGlyph::small(small[1]),
            S::L2 => PadGlyph::small(small[2]),
            S::R2 => PadGlyph::small(small[3]),
            S::L3 => PadGlyph::small(small[4]),
            S::R3 => PadGlyph::small(small[5]),
            S::Start => PadGlyph::small(small[6]),
            S::Select => PadGlyph::small(small[7]),
            S::Home => PadGlyph::small(small[8]),
            S::DpadUp => PadGlyph::small("UP"),
            S::DpadDown => PadGlyph::small("DOWN"),
            S::DpadLeft => PadGlyph::small("LEFT"),
            S::DpadRight => PadGlyph::small("RIGHT"),
        }
    }

    /// The glyph of engine button `b` on this family's pad, at the standard
    /// position the PC and the Web give it ([`PadSlot::standard`]).
    pub const fn button_glyph(self, b: Button) -> PadGlyph {
        self.glyph(PadSlot::standard(b))
    }
}

const XBOX_GREEN: Color = Color::rgb(96, 186, 70);
const XBOX_RED: Color = Color::rgb(224, 64, 56);
const XBOX_BLUE: Color = Color::rgb(52, 120, 228);
const XBOX_YELLOW: Color = Color::rgb(240, 192, 40);
const PS_BLUE: Color = Color::rgb(124, 178, 232);
const PS_RED: Color = Color::rgb(255, 102, 102);
const PS_PINK: Color = Color::rgb(222, 132, 204);
const PS_GREEN: Color = Color::rgb(64, 226, 160);
const SNES_RED: Color = Color::rgb(218, 40, 40);
const SNES_YELLOW: Color = Color::rgb(244, 196, 40);
const SNES_BLUE: Color = Color::rgb(56, 92, 214);
const SNES_GREEN: Color = Color::rgb(40, 160, 72);
const NEUTRAL: Color = Color::rgb(214, 216, 224);

/// A button by its place on a standard pad (the W3C standard mapping's and
/// SDL's positions).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PadSlot {
    /// The face buttons: bottom, right, left, top.
    South,
    East,
    West,
    North,
    L1,
    R1,
    L2,
    R2,
    L3,
    R3,
    Start,
    Select,
    Home,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

impl PadSlot {
    pub const ALL: [PadSlot; 17] = [
        PadSlot::South,
        PadSlot::East,
        PadSlot::West,
        PadSlot::North,
        PadSlot::L1,
        PadSlot::R1,
        PadSlot::L2,
        PadSlot::R2,
        PadSlot::L3,
        PadSlot::R3,
        PadSlot::Start,
        PadSlot::Select,
        PadSlot::Home,
        PadSlot::DpadUp,
        PadSlot::DpadDown,
        PadSlot::DpadLeft,
        PadSlot::DpadRight,
    ];

    /// Where engine button `b` is on a pad by the PC's and the Web's
    /// default bindings: A, B, X, Y = bottom, right, left, top; L/R the
    /// bumpers, L2/R2 the triggers, L3/R3 the stick clicks. (A rebound pad
    /// button is the player's: its label is `Platform::binding_label`.)
    pub const fn standard(b: Button) -> PadSlot {
        match b {
            Button::A => PadSlot::South,
            Button::B => PadSlot::East,
            Button::X => PadSlot::West,
            Button::Y => PadSlot::North,
            Button::L => PadSlot::L1,
            Button::R => PadSlot::R1,
            Button::L2 => PadSlot::L2,
            Button::R2 => PadSlot::R2,
            Button::L3 => PadSlot::L3,
            Button::R3 => PadSlot::R3,
            Button::Start => PadSlot::Start,
            Button::Select => PadSlot::Select,
            Button::DpadUp => PadSlot::DpadUp,
            Button::DpadDown => PadSlot::DpadDown,
            Button::DpadLeft => PadSlot::DpadLeft,
            Button::DpadRight => PadSlot::DpadRight,
        }
    }

    /// One of the four face buttons.
    pub const fn is_face(self) -> bool {
        matches!(
            self,
            PadSlot::South | PadSlot::East | PadSlot::West | PadSlot::North
        )
    }
}

/// How a glyph is drawn: its label as text, or a PlayStation symbol.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GlyphShape {
    Label,
    Cross,
    Circle,
    Square,
    Triangle,
}

/// What a pad prints at one place ([`PadFamily::glyph`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PadGlyph {
    /// On the button: a letter or short name ("A", "LB", "ZL", "+"); empty
    /// for a symbol, which [`shape`](Self::shape) draws.
    pub short: &'static str,
    /// Spelled out, for text ("A", "CROSS", "OPTIONS").
    pub name: &'static str,
    /// The letter's or symbol's color on the pad (a neutral light gray
    /// where the pad prints none).
    pub color: Color,
    pub shape: GlyphShape,
}

impl PadGlyph {
    const fn letter(s: &'static str, color: Color) -> PadGlyph {
        PadGlyph {
            short: s,
            name: s,
            color,
            shape: GlyphShape::Label,
        }
    }

    const fn symbol(name: &'static str, color: Color, shape: GlyphShape) -> PadGlyph {
        PadGlyph {
            short: "",
            name,
            color,
            shape,
        }
    }

    const fn small(s: &'static str) -> PadGlyph {
        PadGlyph::letter(s, NEUTRAL)
    }
}

/// `needle` (lowercase ASCII) somewhere in `hay`, ignoring ASCII case.
fn contains_ignore_case(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    n.len() <= h.len()
        && h.windows(n.len())
            .any(|w| w.iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b))
}

/// A needle that ends in a space ("ps4 ") also matches at the very end of
/// `hay` ("Controller PS4").
fn ends_with_word(hay: &str, needle: &str) -> bool {
    let Some(word) = needle.strip_suffix(' ') else {
        return false;
    };
    let (h, w) = (hay.as_bytes(), word.as_bytes());
    w.len() <= h.len()
        && h[h.len() - w.len()..]
            .iter()
            .zip(w)
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_vendors_name_their_family() {
        assert_eq!(PadFamily::from_usb(0x045e, 0x028e), Some(PadFamily::Xbox));
        assert_eq!(PadFamily::from_usb(0x045e, 0x0b13), Some(PadFamily::Xbox));
        assert_eq!(
            PadFamily::from_usb(0x054c, 0x0ce6),
            Some(PadFamily::PlayStation)
        );
        assert_eq!(
            PadFamily::from_usb(0x054c, 0x05c4),
            Some(PadFamily::PlayStation)
        );
        assert_eq!(
            PadFamily::from_usb(0x054c, 0x09cc),
            Some(PadFamily::PlayStation)
        );
        assert_eq!(
            PadFamily::from_usb(0x057e, 0x2009),
            Some(PadFamily::Nintendo)
        );
        assert_eq!(
            PadFamily::from_usb(0x2dc8, 0x6101),
            Some(PadFamily::EightBitDo)
        );
        // Makers of several families: the name decides.
        for v in [0x0e6f, 0x24c6, 0x0f0d, 0x046d] {
            assert_eq!(PadFamily::from_usb(v, 0x0001), None, "{v:04x}");
        }
    }

    #[test]
    fn names_name_their_family() {
        let cases = [
            ("Xbox Wireless Controller", Some(PadFamily::Xbox)),
            ("Microsoft X-Box 360 pad", Some(PadFamily::Xbox)),
            ("xinput", Some(PadFamily::Xbox)),
            (
                "DualSense Wireless Controller",
                Some(PadFamily::PlayStation),
            ),
            (
                "Sony Interactive Entertainment Wireless Controller",
                Some(PadFamily::PlayStation),
            ),
            ("PS4 Controller", Some(PadFamily::PlayStation)),
            ("Controller PS4", Some(PadFamily::PlayStation)),
            ("Nintendo Switch Pro Controller", Some(PadFamily::Nintendo)),
            ("Pro Controller", Some(PadFamily::Nintendo)),
            ("Joy-Con (L/R)", Some(PadFamily::Nintendo)),
            ("8BitDo Pro 2", Some(PadFamily::EightBitDo)),
            ("8BitDo SN30 Pro+", Some(PadFamily::EightBitDo)),
            ("Logitech Dual Action", None),
            ("Wireless Controller", None),
            ("", None),
        ];
        for (name, want) in cases {
            assert_eq!(PadFamily::from_name(name), want, "{name:?}");
        }
    }

    #[test]
    fn ids_win_over_names_and_unknowns_are_generic() {
        // An 8BitDo in XInput mode: Microsoft's ids, labelled Xbox.
        assert_eq!(
            PadFamily::detect("8BitDo Pro 2", Some((0x045e, 0x028e))),
            PadFamily::Xbox
        );
        assert_eq!(
            PadFamily::detect("PDP Xbox Controller", Some((0x0e6f, 0x02a8))),
            PadFamily::Xbox
        );
        assert_eq!(
            PadFamily::detect("Logitech Dual Action", Some((0x046d, 0xc216))),
            PadFamily::Generic
        );
        assert_eq!(PadFamily::detect("", None), PadFamily::Generic);
        for f in PadFamily::ALL {
            assert_eq!(PadFamily::parse(f.key()), Some(f));
        }
    }

    #[test]
    fn glyph_tables_follow_each_pad() {
        let face = |f: PadFamily| {
            [Button::A, Button::B, Button::X, Button::Y].map(|b| f.button_glyph(b).name)
        };
        assert_eq!(face(PadFamily::Xbox), ["A", "B", "X", "Y"]);
        assert_eq!(
            face(PadFamily::PlayStation),
            ["CROSS", "CIRCLE", "SQUARE", "TRIANGLE"]
        );
        assert_eq!(face(PadFamily::Nintendo), ["B", "A", "Y", "X"]);
        assert_eq!(face(PadFamily::EightBitDo), ["B", "A", "Y", "X"]);
        assert_eq!(face(PadFamily::Generic), ["A", "B", "X", "Y"]);
        // PlayStation: shapes and no letters.
        for s in [PadSlot::South, PadSlot::East, PadSlot::West, PadSlot::North] {
            let g = PadFamily::PlayStation.glyph(s);
            assert!(g.short.is_empty() && g.shape != GlyphShape::Label);
        }
        // Xbox: four different colors, green A.
        let x = PadFamily::Xbox;
        let a = x.glyph(PadSlot::South).color;
        assert!(a.g > a.r && a.g > a.b);
        let colors = [PadSlot::South, PadSlot::East, PadSlot::West, PadSlot::North]
            .map(|s| x.glyph(s).color);
        for i in 0..4 {
            for j in i + 1..4 {
                assert_ne!(colors[i], colors[j]);
            }
        }
        // Every label is short ASCII a bitmap font has.
        for f in PadFamily::ALL {
            for s in PadSlot::ALL {
                let g = f.glyph(s);
                for t in [g.short, g.name] {
                    assert!(
                        t.bytes().all(|c| c.is_ascii_uppercase()
                            || c.is_ascii_digit()
                            || c == b'+'
                            || c == b'-'),
                        "{f:?} {s:?} {t:?}"
                    );
                }
                assert!(!g.name.is_empty());
            }
        }
        assert_eq!(PadSlot::standard(Button::L2), PadSlot::L2);
        assert!(PadSlot::South.is_face() && !PadSlot::L1.is_face());
    }
}
