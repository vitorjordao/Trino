//! Gamepad families through the public API (issue #86): the detection a
//! PC or Web host runs on what its pad layer reports, and the glyphs a
//! menu prints for an engine button on each family.

use trino_core::{Button, GlyphShape, PadFamily, PadSlot};

#[test]
fn detection_and_glyphs_through_the_public_api() {
    // Hosts' inputs: (name, USB ids) as gilrs or a browser gives them.
    let pads = [
        (
            "Xbox 360 Controller",
            Some((0x045e, 0x028e)),
            PadFamily::Xbox,
        ),
        (
            "DualSense Wireless Controller",
            Some((0x054c, 0x0ce6)),
            PadFamily::PlayStation,
        ),
        (
            "Wireless Controller",
            Some((0x054c, 0x05c4)),
            PadFamily::PlayStation,
        ),
        (
            "Pro Controller",
            Some((0x057e, 0x2009)),
            PadFamily::Nintendo,
        ),
        (
            "8BitDo SN30 Pro",
            Some((0x2dc8, 0x6001)),
            PadFamily::EightBitDo,
        ),
        ("DualShock 4", None, PadFamily::PlayStation),
        ("Gamepad", None, PadFamily::Generic),
    ];
    for (name, usb, want) in pads {
        assert_eq!(PadFamily::detect(name, usb), want, "{name}");
    }
    // The menu's side: every engine button has a glyph on every family,
    // positional (A is the bottom face button).
    for f in PadFamily::ALL {
        for b in Button::ALL {
            let g = f.button_glyph(b);
            assert_eq!(g, f.glyph(PadSlot::standard(b)));
            assert!(!g.name.is_empty(), "{f:?} {b:?}");
        }
    }
    assert_eq!(PadFamily::Xbox.button_glyph(Button::A).short, "A");
    assert_eq!(PadFamily::Nintendo.button_glyph(Button::A).short, "B");
    assert_eq!(PadFamily::Nintendo.button_glyph(Button::B).short, "A");
    let cross = PadFamily::PlayStation.button_glyph(Button::A);
    assert_eq!(
        (cross.short, cross.name, cross.shape),
        ("", "CROSS", GlyphShape::Cross)
    );
    assert_eq!(PadFamily::PlayStation.button_glyph(Button::L).short, "L1");
}
