//! Button glyphs and controller diagrams per gamepad family (issue #86
//! item 7): what the menus print and draw for a button once the platform
//! has told which pad is plugged in ([`trino_core::Platform::gamepad_family`],
//! PC and Web; `None` on the consoles and without a pad).
//!
//! Everything here is data and pure functions over `trino_core`'s
//! [`PadFamily`] tables, so the menus stay simple:
//!
//! - [`glyph`]: a game button's label (ASCII within the fonts' characters),
//!   badge colors and shape on the connected family's pad. The engine's
//!   buttons are positional on the PC and the Web (`Button::A` is the
//!   bottom face button), so on a Nintendo pad `A` prints `B`.
//! - [`spans`]: the PlayStation symbols (cross, circle, square, triangle)
//!   as horizontal runs, one or two per row, for `fill_rect` (no textures,
//!   as everything in [`crate::ui`]).
//! - [`chassis`]: where each family's face buttons, D-pad, sticks and
//!   small buttons sit, in coordinates normalized to the diagram's box, for
//!   the controls page's drawing.

use trino_core::{Button, Color, GlyphShape, PadFamily, PadSlot};

/// How a glyph is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// The label in a round badge (face buttons with letters).
    Round,
    /// The label in a pill (shoulders, sticks, small buttons, the D-pad).
    Pill,
    /// PlayStation's symbols, drawn with [`spans`] in the ink color.
    Cross,
    Circle,
    Square,
    Triangle,
}

impl Shape {
    /// A symbol drawn by [`spans`] (no text).
    pub const fn is_symbol(self) -> bool {
        matches!(
            self,
            Shape::Cross | Shape::Circle | Shape::Square | Shape::Triangle
        )
    }
}

/// A button as one family's pad shows it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Glyph {
    /// Printed on the badge: the letter or the short name ("A", "LB", "ZL",
    /// "+"). Empty for a symbol ([`Shape::is_symbol`]).
    pub label: &'static str,
    /// Spelled out, for lists and captions ("A", "CROSS", "L1"). Always
    /// within the fonts' characters (`crate::ui::FONT_CHARS`).
    pub name: &'static str,
    /// The badge.
    pub fill: Color,
    /// The label or symbol on it.
    pub ink: Color,
    pub shape: Shape,
}

/// A dark badge, as on the pads themselves.
const BADGE: Color = Color::rgb(28, 30, 40);
const LIGHT: Color = Color::rgb(236, 240, 255);

/// `b` on a pad of `family`.
pub fn glyph(family: PadFamily, b: Button) -> Glyph {
    let slot = PadSlot::standard(b);
    let g = family.glyph(slot);
    let shape = match g.shape {
        GlyphShape::Cross => Shape::Cross,
        GlyphShape::Circle => Shape::Circle,
        GlyphShape::Square => Shape::Square,
        GlyphShape::Triangle => Shape::Triangle,
        GlyphShape::Label if slot.is_face() => Shape::Round,
        GlyphShape::Label => Shape::Pill,
    };
    // The 8BitDo's colored face buttons carry dark letters; every other
    // pad's face is a dark button with its color in the letter or symbol.
    let (fill, ink) = match (family, shape) {
        (PadFamily::EightBitDo, Shape::Round) => (g.color, BADGE),
        (_, Shape::Pill) => (Color::rgb(60, 66, 92), LIGHT),
        _ => (BADGE, g.color),
    };
    Glyph {
        label: g.short,
        name: g.name,
        fill,
        ink,
        shape,
    }
}

/// The label for a list: the short name, or the spelled-out symbol on a
/// PlayStation pad ("CROSS"), whose symbols the fonts don't have.
pub fn label(family: PadFamily, b: Button) -> &'static str {
    let g = glyph(family, b);
    if g.label.is_empty() { g.name } else { g.label }
}

/// Row `row` (0 = top) of a symbol `size` pixels square: up to two
/// horizontal runs `[x0, x1)`, for `fill_rect(x + x0, y + row, x1 - x0, 1)`.
/// Strokes are `size / 6` thick (at least 1). `None` for the text shapes.
pub fn spans(shape: Shape, size: u8, row: u8) -> [Option<(u8, u8)>; 2] {
    if row >= size || size < 3 {
        return [None, None];
    }
    let n = size as i32;
    let y = row as i32;
    let t = (n / 6).max(1);
    let run = |a: i32, b: i32| -> Option<(u8, u8)> {
        let (a, b) = (a.clamp(0, n), b.clamp(0, n));
        (b > a).then_some((a as u8, b as u8))
    };
    match shape {
        Shape::Round | Shape::Pill => [None, None],
        Shape::Square => {
            if y < t || y >= n - t {
                [run(0, n), None]
            } else {
                [run(0, t), run(n - t, n)]
            }
        }
        Shape::Cross => {
            // Two diagonals, `t` wide each: one along x = y, the other its
            // mirror image (so the symbol stays symmetric when clamped).
            let a = (y - t / 2).clamp(0, n - t);
            let (lo, hi) = (a.min(n - a - t), a.max(n - a - t));
            if hi - lo <= t {
                [run(lo, hi + t), None]
            } else {
                [run(lo, lo + t), run(hi, hi + t)]
            }
        }
        Shape::Circle => {
            // A ring between radii r - t and r, sampled at pixel centers.
            let r2 = n * n; // (2r)², in half-pixel units: r = n / 2
            let inner = (n - 2 * t) * (n - 2 * t);
            let dy = 2 * y + 1 - n;
            let mut first = [None, None];
            let mut k = 0;
            let mut start: Option<i32> = None;
            for x in 0..=n {
                let dx = 2 * x + 1 - n;
                let d = dx * dx + dy * dy;
                let on = x < n && d <= r2 && d >= inner;
                match (on, start) {
                    (true, None) => start = Some(x),
                    (false, Some(s)) => {
                        if k < 2 {
                            first[k] = run(s, x);
                            k += 1;
                        }
                        start = None;
                    }
                    _ => {}
                }
            }
            first
        }
        Shape::Triangle => {
            // Apex at the top center, base on the last row.
            let half = |y: i32| (y * n) / (2 * (n - 1).max(1));
            let (l, r) = (n / 2 - half(y) - 1, (n + 1) / 2 + half(y) + 1);
            if y >= n - t {
                [run(0, n), None]
            } else if r - l <= 2 * t {
                [run(l, r), None]
            } else {
                [run(l, l + t), run(r - t, r)]
            }
        }
    }
}

/// Where a family's controls sit on its diagram, each `(x, y)` normalized
/// to the diagram's box (0..1, y down). The face buttons are a diamond of
/// half-width `face_gap` around `face`; shoulders and triggers sit on the
/// top edge.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Chassis {
    pub face: (f32, f32),
    pub face_gap: f32,
    pub dpad: (f32, f32),
    pub left_stick: (f32, f32),
    pub right_stick: (f32, f32),
    pub select: (f32, f32),
    pub start: (f32, f32),
    pub home: (f32, f32),
    pub l1: (f32, f32),
    pub r1: (f32, f32),
    pub l2: (f32, f32),
    pub r2: (f32, f32),
}

/// Offset sticks (left stick high, D-pad low): Xbox and the Switch Pro.
const OFFSET: Chassis = Chassis {
    face: (0.74, 0.36),
    face_gap: 0.075,
    dpad: (0.37, 0.62),
    left_stick: (0.26, 0.36),
    right_stick: (0.63, 0.62),
    select: (0.42, 0.36),
    start: (0.58, 0.36),
    home: (0.5, 0.2),
    l1: (0.24, 0.1),
    r1: (0.76, 0.1),
    l2: (0.24, 0.0),
    r2: (0.76, 0.0),
};

/// Sticks low and side by side, D-pad high: PlayStation, most generic
/// DirectInput pads and the 8BitDo SN30 Pro (wider, SNES-like).
const SYMMETRIC: Chassis = Chassis {
    face: (0.75, 0.38),
    face_gap: 0.075,
    dpad: (0.25, 0.38),
    left_stick: (0.37, 0.66),
    right_stick: (0.63, 0.66),
    select: (0.33, 0.2),
    start: (0.67, 0.2),
    home: (0.5, 0.6),
    l1: (0.25, 0.1),
    r1: (0.75, 0.1),
    l2: (0.25, 0.0),
    r2: (0.75, 0.0),
};

const SN30: Chassis = Chassis {
    face: (0.79, 0.4),
    dpad: (0.21, 0.4),
    left_stick: (0.38, 0.7),
    right_stick: (0.62, 0.7),
    select: (0.43, 0.46),
    start: (0.57, 0.46),
    home: (0.5, 0.3),
    ..SYMMETRIC
};

/// `family`'s diagram layout.
pub fn chassis(family: PadFamily) -> &'static Chassis {
    match family {
        PadFamily::Xbox | PadFamily::Nintendo => &OFFSET,
        PadFamily::EightBitDo => &SN30,
        PadFamily::PlayStation | PadFamily::Generic => &SYMMETRIC,
    }
}

/// Where button `b` sits on `family`'s diagram (normalized, see
/// [`Chassis`]). Face buttons are positional: `Button::A` is the bottom of
/// the diamond on every family (labelled `B` on a Nintendo pad).
pub fn spot(family: PadFamily, b: Button) -> (f32, f32) {
    let c = chassis(family);
    let (fx, fy) = c.face;
    let g = c.face_gap;
    // The diamond is drawn on a box about 1.6 wide per 1 tall.
    let gy = g * 1.6;
    let d = 0.06;
    match PadSlot::standard(b) {
        PadSlot::South => (fx, fy + gy),
        PadSlot::East => (fx + g, fy),
        PadSlot::West => (fx - g, fy),
        PadSlot::North => (fx, fy - gy),
        PadSlot::L1 => c.l1,
        PadSlot::R1 => c.r1,
        PadSlot::L2 => c.l2,
        PadSlot::R2 => c.r2,
        PadSlot::L3 => c.left_stick,
        PadSlot::R3 => c.right_stick,
        PadSlot::Start => c.start,
        PadSlot::Select => c.select,
        PadSlot::Home => c.home,
        PadSlot::DpadUp => (c.dpad.0, c.dpad.1 - d * 1.6),
        PadSlot::DpadDown => (c.dpad.0, c.dpad.1 + d * 1.6),
        PadSlot::DpadLeft => (c.dpad.0 - d, c.dpad.1),
        PadSlot::DpadRight => (c.dpad.0 + d, c.dpad.1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_fit_the_fonts() {
        for f in PadFamily::ALL {
            for b in Button::ALL {
                let g = glyph(f, b);
                for s in [g.label, g.name, label(f, b)] {
                    assert!(
                        s.chars().all(|c| super::super::FONT_CHARS.contains(c)),
                        "{f:?} {b:?}: {s:?}"
                    );
                }
                assert!(!label(f, b).is_empty(), "{f:?} {b:?}");
                assert!(g.label.is_empty() == g.shape.is_symbol(), "{f:?} {b:?}");
            }
        }
    }

    #[test]
    fn each_family_prints_its_own_face_buttons() {
        use Button::*;
        let names = |f| [A, B, X, Y].map(|b| label(f, b));
        assert_eq!(names(PadFamily::Xbox), ["A", "B", "X", "Y"]);
        assert_eq!(
            names(PadFamily::PlayStation),
            ["CROSS", "CIRCLE", "SQUARE", "TRIANGLE"]
        );
        // Nintendo's letters sit swapped: the bottom button is B.
        assert_eq!(names(PadFamily::Nintendo), ["B", "A", "Y", "X"]);
        assert_eq!(names(PadFamily::EightBitDo), ["B", "A", "Y", "X"]);
        assert_eq!(names(PadFamily::Generic), ["A", "B", "X", "Y"]);
        assert_eq!(label(PadFamily::Xbox, L), "LB");
        assert_eq!(label(PadFamily::PlayStation, R2), "R2");
        assert_eq!(label(PadFamily::Nintendo, L2), "ZL");
        assert_eq!(label(PadFamily::Nintendo, Start), "+");
        // PlayStation: shapes, no letters; Xbox: colored letters.
        assert_eq!(glyph(PadFamily::PlayStation, A).shape, Shape::Cross);
        assert_eq!(glyph(PadFamily::PlayStation, Y).shape, Shape::Triangle);
        let xa = glyph(PadFamily::Xbox, A);
        assert_eq!((xa.shape, xa.label), (Shape::Round, "A"));
        assert!(xa.ink.g > xa.ink.r && xa.ink.g > xa.ink.b, "A is green");
        assert!(glyph(PadFamily::Xbox, B).ink.r > 150, "B is red");
        assert_eq!(glyph(PadFamily::Xbox, L).shape, Shape::Pill);
    }

    #[test]
    fn symbols_are_closed_strokes_inside_their_square() {
        for size in [5u8, 9, 12, 16] {
            for shape in [Shape::Cross, Shape::Circle, Shape::Square, Shape::Triangle] {
                let mut px = 0;
                for row in 0..size {
                    for (a, b) in spans(shape, size, row).into_iter().flatten() {
                        assert!(a < b && b <= size, "{shape:?} {size} row {row}");
                        px += (b - a) as u32;
                    }
                }
                let area = size as u32 * size as u32;
                assert!(px > 0 && px < area, "{shape:?} {size}: {px} of {area}");
            }
            // Text shapes have no runs; rows past the size neither.
            assert_eq!(spans(Shape::Round, size, 0), [None, None]);
            assert_eq!(spans(Shape::Square, size, size), [None, None]);
        }
        // A square's top row is solid, its middle two strokes; a circle's
        // middle row two strokes, its top one run; a cross's middle row
        // one run (the crossing), its top row two.
        let row = |s, r| spans(s, 12, r).iter().flatten().count();
        assert_eq!((row(Shape::Square, 0), row(Shape::Square, 6)), (1, 2));
        assert_eq!((row(Shape::Circle, 0), row(Shape::Circle, 6)), (1, 2));
        assert_eq!((row(Shape::Cross, 0), row(Shape::Cross, 6)), (2, 1));
        assert_eq!(row(Shape::Triangle, 11), 1, "the base");
        // Symmetric about the vertical axis.
        for shape in [Shape::Cross, Shape::Circle, Shape::Square] {
            for r in 0..12 {
                let s = spans(shape, 12, r);
                let w: Vec<_> = s.iter().flatten().map(|&(a, b)| (12 - b, 12 - a)).collect();
                let mut m: Vec<_> = s.iter().flatten().copied().collect();
                m.sort();
                let mut w = w;
                w.sort();
                assert_eq!(m, w, "{shape:?} row {r}");
            }
        }
    }

    #[test]
    fn diagrams_place_every_button_inside_the_box() {
        for f in PadFamily::ALL {
            let mut seen: Vec<(f32, f32)> = Vec::new();
            for b in Button::ALL {
                let (x, y) = spot(f, b);
                assert!(
                    (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y),
                    "{f:?} {b:?}"
                );
                assert!(!seen.contains(&(x, y)), "{f:?} {b:?} on another button");
                seen.push((x, y));
            }
            // The face diamond: A (bottom) below Y (top), X left of B.
            let (a, bb, x, y) = (
                spot(f, Button::A),
                spot(f, Button::B),
                spot(f, Button::X),
                spot(f, Button::Y),
            );
            assert!(a.1 > y.1 && x.0 < bb.0, "{f:?}");
        }
        // Xbox and Switch Pro: the left stick above the D-pad; PlayStation
        // the other way round.
        let c = chassis(PadFamily::Xbox);
        assert!(c.left_stick.1 < c.dpad.1);
        let c = chassis(PadFamily::PlayStation);
        assert!(c.left_stick.1 > c.dpad.1);
    }
}
