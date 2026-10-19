//! Browser input as the engine's [`InputState`], without a browser: the
//! keyboard by `KeyboardEvent.code` (the physical key, whatever the
//! layout), and gamepads in the W3C *standard* mapping
//! (`navigator.getGamepads()`). The bindings are the PC's
//! (`trino_platform_pc::input`), so a game plays the same on both:
//! A/B = Z/X, X/Y = C/V, L/R = Q/E, Start = Enter, Select = Right Shift,
//! D-pad = arrows, stick = WASD; pads A/B/X/Y, L = LB+LT, R = RB+RT,
//! L2/R2 = LT/RT, L3/R3 = stick clicks, Start = Start+Guide,
//! Select = Back+LS.

use trino_core::{Button, InputState, Key, PadFamily, Vec2};

/// The [`Key`] bits a `KeyboardEvent.code` sets (empty for keys without
/// one). Like the PC: the numpad's 1–3, Enter, + and − alias the main row,
/// and `=` sets both [`Key::Equal`] and [`Key::Plus`].
pub fn keys_for(code: &str) -> &'static [Key] {
    match code {
        "Escape" => &[Key::Escape],
        "Enter" | "NumpadEnter" => &[Key::Enter],
        "Space" => &[Key::Space],
        "Tab" => &[Key::Tab],
        "Delete" => &[Key::Delete],
        "Backspace" => &[Key::Backspace],
        "Insert" => &[Key::Insert],
        "Home" => &[Key::Home],
        "End" => &[Key::End],
        "PageUp" => &[Key::PageUp],
        "PageDown" => &[Key::PageDown],
        "ArrowUp" => &[Key::ArrowUp],
        "ArrowDown" => &[Key::ArrowDown],
        "ArrowLeft" => &[Key::ArrowLeft],
        "ArrowRight" => &[Key::ArrowRight],
        "F1" => &[Key::F1],
        "F2" => &[Key::F2],
        "F3" => &[Key::F3],
        "F4" => &[Key::F4],
        "F5" => &[Key::F5],
        "F6" => &[Key::F6],
        "F7" => &[Key::F7],
        "F8" => &[Key::F8],
        "F9" => &[Key::F9],
        "F10" => &[Key::F10],
        "F11" => &[Key::F11],
        "F12" => &[Key::F12],
        "Digit0" => &[Key::Digit0],
        "Digit1" | "Numpad1" => &[Key::Digit1],
        "Digit2" | "Numpad2" => &[Key::Digit2],
        "Digit3" | "Numpad3" => &[Key::Digit3],
        "Digit4" => &[Key::Digit4],
        "Digit5" => &[Key::Digit5],
        "Digit6" => &[Key::Digit6],
        "Digit7" => &[Key::Digit7],
        "Digit8" => &[Key::Digit8],
        "Digit9" => &[Key::Digit9],
        "KeyA" => &[Key::A],
        "KeyB" => &[Key::B],
        "KeyC" => &[Key::C],
        "KeyD" => &[Key::D],
        "KeyE" => &[Key::E],
        "KeyF" => &[Key::F],
        "KeyG" => &[Key::G],
        "KeyH" => &[Key::H],
        "KeyI" => &[Key::I],
        "KeyJ" => &[Key::J],
        "KeyK" => &[Key::K],
        "KeyL" => &[Key::L],
        "KeyM" => &[Key::M],
        "KeyN" => &[Key::N],
        "KeyO" => &[Key::O],
        "KeyP" => &[Key::P],
        "KeyQ" => &[Key::Q],
        "KeyR" => &[Key::R],
        "KeyS" => &[Key::S],
        "KeyT" => &[Key::T],
        "KeyU" => &[Key::U],
        "KeyV" => &[Key::V],
        "KeyW" => &[Key::W],
        "KeyX" => &[Key::X],
        "KeyY" => &[Key::Y],
        "KeyZ" => &[Key::Z],
        "Equal" => &[Key::Equal, Key::Plus],
        "NumpadAdd" => &[Key::Plus],
        "Minus" | "NumpadSubtract" => &[Key::Minus],
        "ShiftLeft" | "ShiftRight" => &[Key::Shift],
        "ControlLeft" | "ControlRight" => &[Key::Ctrl],
        "AltLeft" | "AltRight" => &[Key::Alt],
        "MetaLeft" | "MetaRight" => &[Key::Super],
        "BracketLeft" => &[Key::BracketLeft],
        "BracketRight" => &[Key::BracketRight],
        "Semicolon" => &[Key::Semicolon],
        "Quote" => &[Key::Quote],
        "Comma" => &[Key::Comma],
        "Period" => &[Key::Period],
        "Slash" => &[Key::Slash],
        "Backslash" => &[Key::Backslash],
        "Backquote" => &[Key::Backquote],
        _ => &[],
    }
}

/// The console button a key presses (the PC's default bindings).
pub fn button_for(code: &str) -> Option<Button> {
    Some(match code {
        "KeyZ" => Button::A,
        "KeyX" => Button::B,
        "KeyC" => Button::X,
        "KeyV" => Button::Y,
        "KeyQ" => Button::L,
        "KeyE" => Button::R,
        "Enter" => Button::Start,
        "ShiftRight" => Button::Select,
        "ArrowUp" => Button::DpadUp,
        "ArrowDown" => Button::DpadDown,
        "ArrowLeft" => Button::DpadLeft,
        "ArrowRight" => Button::DpadRight,
        _ => return None,
    })
}

/// Keys whose browser default would take the game's input away (scroll
/// the page, move focus off the canvas, go back a page): the canvas calls
/// `preventDefault` on them. Browser and OS shortcuts (F5, F11, F12,
/// Ctrl+…) keep working.
pub fn is_game_key(code: &str) -> bool {
    button_for(code).is_some()
        || matches!(
            code,
            "Space"
                | "Tab"
                | "Backspace"
                | "PageUp"
                | "PageDown"
                | "Home"
                | "End"
                | "Slash"
                | "Quote"
                | "KeyW"
                | "KeyA"
                | "KeyS"
                | "KeyD"
        )
}

/// The keyboard's part of the input: held keys, bound buttons, the WASD
/// stick.
#[derive(Clone, Copy, Debug, Default)]
pub struct Keyboard {
    state: InputState,
    wasd: [bool; 4],
}

impl Keyboard {
    /// A key went down or up (repeats filtered by the caller).
    pub fn key(&mut self, code: &str, down: bool) {
        if let Some(b) = button_for(code) {
            self.state.set(b, down);
        }
        for k in keys_for(code) {
            self.state.set_key(*k, down);
        }
        match code {
            "KeyW" => self.wasd[0] = down,
            "KeyA" => self.wasd[1] = down,
            "KeyS" => self.wasd[2] = down,
            "KeyD" => self.wasd[3] = down,
            _ => {}
        }
        // Digital stick: Y-up positive, matching real console sticks.
        self.state.stick = Vec2::new(
            (self.wasd[3] as i8 - self.wasd[1] as i8) as f32,
            (self.wasd[0] as i8 - self.wasd[2] as i8) as f32,
        );
    }

    /// Every key up (the canvas lost focus: a key released elsewhere never
    /// reports its `keyup`).
    pub fn release_all(&mut self) {
        *self = Keyboard::default();
    }

    /// The keyboard's state for a frame.
    pub fn state(&self) -> InputState {
        self.state
    }
}

/// One gamepad as the Gamepad API reports it in the standard mapping:
/// `buttons[i]` (pressed, value 0..=1) and `axes` (left x, left y, right x,
/// right y; Y down).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PadReading {
    pub buttons: Vec<(bool, f32)>,
    pub axes: Vec<f32>,
}

/// The standard mapping's buttons, by index, and what each presses (the
/// PC's defaults, through gilrs's names).
const STANDARD: [&[Button]; 17] = [
    &[Button::A],                  // 0 bottom face
    &[Button::B],                  // 1 right face
    &[Button::X],                  // 2 left face
    &[Button::Y],                  // 3 top face
    &[Button::L],                  // 4 left bumper
    &[Button::R],                  // 5 right bumper
    &[Button::L, Button::L2],      // 6 left trigger
    &[Button::R, Button::R2],      // 7 right trigger
    &[Button::Select],             // 8 back / select
    &[Button::Start],              // 9 start
    &[Button::Select, Button::L3], // 10 left stick click
    &[Button::R3],                 // 11 right stick click
    &[Button::DpadUp],             // 12
    &[Button::DpadDown],           // 13
    &[Button::DpadLeft],           // 14
    &[Button::DpadRight],          // 15
    &[Button::Start],              // 16 guide
];

/// A stick counts as "the pad was used" past this radius (the PC's).
const DEADZONE: f32 = 0.25;

/// Fold one pad into `state` the way the PC folds a gilrs pad: bound
/// buttons OR'ed in, the stronger stick wins, triggers take the larger
/// value, and [`InputState::pad`] says the pad was used.
pub fn merge_pad(state: &mut InputState, pad: &PadReading) {
    for (i, &(pressed, _)) in pad.buttons.iter().enumerate() {
        if pressed && let Some(to) = STANDARD.get(i) {
            for b in *to {
                state.set(*b, true);
            }
            state.pad = true;
        }
    }
    let axis = |i: usize| pad.axes.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
    // The Gamepad API's Y axes point down; the engine's sticks point up.
    let left = Vec2::new(axis(0), -axis(1));
    let right = Vec2::new(axis(2), -axis(3));
    let len2 = |v: Vec2| v.x * v.x + v.y * v.y;
    for (into, v) in [(&mut state.stick, left), (&mut state.right_stick, right)] {
        if len2(v) > len2(*into) {
            *into = v;
        }
        if len2(v) > DEADZONE * DEADZONE {
            state.pad = true;
        }
    }
    let trigger = |i: usize| pad.buttons.get(i).map_or(0.0, |b| b.1.clamp(0.0, 1.0));
    let t = Vec2::new(trigger(6), trigger(7));
    if t.x > DEADZONE || t.y > DEADZONE {
        state.pad = true;
    }
    state.triggers = Vec2::new(state.triggers.x.max(t.x), state.triggers.y.max(t.y));
}

/// A `Gamepad.id` split into the pad's name and its USB vendor and product
/// ids, in each browser's form:
///
/// - Chrome and Edge: `DualSense Wireless Controller (STANDARD GAMEPAD
///   Vendor: 054c Product: 0ce6)`, `8BitDo SN30 Pro+ (Vendor: 2dc8 Product:
///   6002)`, or for XInput pads `Xbox 360 Controller (XInput STANDARD
///   GAMEPAD)` (no ids);
/// - Firefox: `054c-0ce6-DualSense Wireless Controller` (on some systems
///   without the leading zeros, `45e-28e-...`; on Windows a bare
///   `xinput`);
/// - Safari: the name alone.
pub fn parse_pad_id(id: &str) -> (&str, Option<(u16, u16)>) {
    let id = id.trim();
    // Chrome: the ids (if any) in a trailing parenthesis.
    if id.ends_with(')')
        && let Some(open) = id.rfind('(')
    {
        let inner = &id[open + 1..id.len() - 1];
        let hex_after = |key: &str| {
            let at = inner.find(key)? + key.len();
            let digits: &str = inner[at..].split_whitespace().next()?;
            u16::from_str_radix(digits, 16).ok()
        };
        let usb = hex_after("Vendor:").zip(hex_after("Product:"));
        if usb.is_some() || inner.contains("GAMEPAD") || inner.contains("XInput") {
            return (id[..open].trim_end(), usb);
        }
    }
    // Firefox: `vvvv-pppp-name`, 1 to 4 hex digits each.
    let mut parts = id.splitn(3, '-');
    if let (Some(v), Some(p), Some(name)) = (parts.next(), parts.next(), parts.next()) {
        let hex = |t: &str| (1..=4).contains(&t.len()) && t.bytes().all(|c| c.is_ascii_hexdigit());
        if hex(v) && hex(p) {
            let usb = u16::from_str_radix(v, 16)
                .ok()
                .zip(u16::from_str_radix(p, 16).ok());
            return (name.trim(), usb);
        }
    }
    (id, None)
}

/// The family of the pad a `Gamepad.id` describes ([`parse_pad_id`], then
/// [`PadFamily::detect`]: the ids where they name one, else the name).
pub fn pad_family(id: &str) -> PadFamily {
    let (name, usb) = parse_pad_id(id);
    PadFamily::detect(name, usb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_press_the_pcs_buttons_and_bits() {
        let mut k = Keyboard::default();
        k.key("KeyZ", true);
        k.key("Enter", true);
        k.key("ArrowLeft", true);
        let s = k.state();
        assert!(s.is_down(Button::A) && s.is_down(Button::Start) && s.is_down(Button::DpadLeft));
        assert!(s.key(Key::Z) && s.key(Key::Enter) && s.key(Key::ArrowLeft));
        k.key("KeyZ", false);
        assert!(!k.state().is_down(Button::A) && !k.state().key(Key::Z));
        // `=` is Equal and Plus; numpad 2 is Digit2.
        k.key("Equal", true);
        k.key("Numpad2", true);
        let s = k.state();
        assert!(s.key(Key::Equal) && s.key(Key::Plus) && s.key(Key::Digit2));
        k.release_all();
        assert_eq!(k.state(), InputState::default());
    }

    #[test]
    fn wasd_is_the_stick_and_opposites_cancel() {
        let mut k = Keyboard::default();
        k.key("KeyD", true);
        k.key("KeyW", true);
        assert_eq!(k.state().stick, Vec2::new(1.0, 1.0));
        k.key("KeyA", true);
        assert_eq!(k.state().stick, Vec2::new(0.0, 1.0));
    }

    #[test]
    fn game_keys_stop_the_page_scrolling_but_not_browser_shortcuts() {
        for code in ["ArrowDown", "Space", "Tab", "KeyZ", "KeyW"] {
            assert!(is_game_key(code), "{code}");
        }
        for code in ["F5", "F11", "F12", "KeyL", "ControlLeft"] {
            assert!(!is_game_key(code), "{code}");
        }
    }

    #[test]
    fn a_standard_pad_maps_like_the_pc() {
        let mut pad = PadReading {
            buttons: vec![(false, 0.0); 17],
            axes: vec![0.5, -1.0, 0.0, 0.1],
        };
        pad.buttons[0] = (true, 1.0);
        pad.buttons[7] = (true, 0.8);
        let mut s = InputState::default();
        merge_pad(&mut s, &pad);
        assert!(s.is_down(Button::A) && s.is_down(Button::R) && s.is_down(Button::R2));
        // Up on the pad is up for the game.
        assert_eq!(s.stick, Vec2::new(0.5, 1.0));
        assert_eq!(s.triggers, Vec2::new(0.0, 0.8));
        assert!(s.pad);
        // A resting pad leaves the keyboard's stick and says nothing.
        let mut k = Keyboard::default();
        k.key("KeyW", true);
        let mut s = k.state();
        merge_pad(
            &mut s,
            &PadReading {
                buttons: vec![(false, 0.0); 17],
                axes: vec![0.05, 0.05, 0.0, 0.0],
            },
        );
        assert_eq!(s.stick, Vec2::new(0.0, 1.0));
        assert!(!s.pad);
    }

    #[test]
    fn gamepad_ids_parse_in_every_browsers_form() {
        type Case = (&'static str, &'static str, Option<(u16, u16)>, PadFamily);
        let cases: [Case; 14] = [
            // Chrome / Edge.
            (
                "Xbox 360 Controller (XInput STANDARD GAMEPAD)",
                "Xbox 360 Controller",
                None,
                PadFamily::Xbox,
            ),
            (
                "Xbox Wireless Controller (STANDARD GAMEPAD Vendor: 045e Product: 0b13)",
                "Xbox Wireless Controller",
                Some((0x045e, 0x0b13)),
                PadFamily::Xbox,
            ),
            (
                "DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)",
                "DualSense Wireless Controller",
                Some((0x054c, 0x0ce6)),
                PadFamily::PlayStation,
            ),
            (
                "Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 09cc)",
                "Wireless Controller",
                Some((0x054c, 0x09cc)),
                PadFamily::PlayStation,
            ),
            (
                "Pro Controller (STANDARD GAMEPAD Vendor: 057e Product: 2009)",
                "Pro Controller",
                Some((0x057e, 0x2009)),
                PadFamily::Nintendo,
            ),
            (
                "8BitDo SN30 Pro+ (Vendor: 2dc8 Product: 6002)",
                "8BitDo SN30 Pro+",
                Some((0x2dc8, 0x6002)),
                PadFamily::EightBitDo,
            ),
            (
                "USB Gamepad (Vendor: 0079 Product: 0011)",
                "USB Gamepad",
                Some((0x0079, 0x0011)),
                PadFamily::Generic,
            ),
            // Firefox.
            (
                "054c-0ce6-DualSense Wireless Controller",
                "DualSense Wireless Controller",
                Some((0x054c, 0x0ce6)),
                PadFamily::PlayStation,
            ),
            (
                "45e-28e-Xbox 360 Wired Controller",
                "Xbox 360 Wired Controller",
                Some((0x045e, 0x028e)),
                PadFamily::Xbox,
            ),
            (
                "057e-2009-Pro Controller",
                "Pro Controller",
                Some((0x057e, 0x2009)),
                PadFamily::Nintendo,
            ),
            ("xinput", "xinput", None, PadFamily::Xbox),
            // Safari.
            (
                "DualSense Wireless Controller",
                "DualSense Wireless Controller",
                None,
                PadFamily::PlayStation,
            ),
            (
                "Xbox Wireless Controller Extended Gamepad",
                "Xbox Wireless Controller Extended Gamepad",
                None,
                PadFamily::Xbox,
            ),
            // A name with dashes and parentheses that aren't ids.
            ("Joy-Con (L)", "Joy-Con (L)", None, PadFamily::Nintendo),
        ];
        for (id, name, usb, family) in cases {
            assert_eq!(parse_pad_id(id), (name, usb), "{id}");
            assert_eq!(pad_family(id), family, "{id}");
        }
    }
}
