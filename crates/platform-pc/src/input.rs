//! Keyboard + mouse + gamepad input backend. Gamepads come through gilrs
//! with the `gamepad` feature (Xbox-style layout: A/B/X/Y = South/East/
//! West/North, L/R = bumpers or triggers, the left stick and the D-pad);
//! every connected pad is merged with the keyboard.
//!
//! The mouse is the [`InputState::pointer`]: the host converts window
//! pixels to game pixels ([`crate::PcRenderer::window_to_game`]) and feeds
//! [`PcInput::handle_pointer`], [`PcInput::handle_mouse_button`] and
//! [`PcInput::handle_wheel`]. Left = `Primary`, right = `Secondary`,
//! middle = `Middle`.
//!
//! Default mapping (chosen to avoid key overlaps):
//!
//! | Trino    | Key         |
//! |----------|-------------|
//! | A / B    | Z / X       |
//! | X / Y    | C / V       |
//! | L / R    | Q / E       |
//! | Start    | Enter       |
//! | Select   | Right Shift |
//! | D-pad    | Arrows      |
//! | Stick    | WASD        |
//!
//! The console buttons can be **rebound** (keyboard and gamepad separately):
//! [`Bindings`] holds the table, [`PcInput::apply`] starts a capture (the
//! next key or pad button pressed becomes the button's binding), and
//! [`bindings_to_text`] / [`bindings_from_text`] persist it (the runner
//! keeps `<save_name>.input`). The stick's WASD and the [`Key`] bits stay
//! fixed.
//!
//! Every key with a [`Key`] also sets that bit in [`InputState::keys`]
//! (Esc, Enter, Space, Tab, Delete, Backspace, F1–F8, 1–3, Page Up/Down,
//! Home, R, W/A/S/D, +/−, Shift, Ctrl), so a game can offer PC shortcuts on
//! top of the console mapping.

use trino_core::{
    BindDevice, Button, GamepadStatus, Input, InputInfo, InputState, Key, PointerButton, Vec2,
};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// The console buttons that can be rebound, in [`Bindings`] order.
pub const BINDABLE: [Button; 12] = [
    Button::A,
    Button::B,
    Button::X,
    Button::Y,
    Button::L,
    Button::R,
    Button::Start,
    Button::Select,
    Button::DpadUp,
    Button::DpadDown,
    Button::DpadLeft,
    Button::DpadRight,
];

/// A gamepad button (the standard Xbox-style layout gilrs reports).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PadButton {
    South,
    East,
    North,
    West,
    LeftTrigger,
    LeftTrigger2,
    RightTrigger,
    RightTrigger2,
    Select,
    Start,
    Mode,
    LeftThumb,
    RightThumb,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

impl PadButton {
    pub const ALL: [PadButton; 17] = [
        PadButton::South,
        PadButton::East,
        PadButton::North,
        PadButton::West,
        PadButton::LeftTrigger,
        PadButton::LeftTrigger2,
        PadButton::RightTrigger,
        PadButton::RightTrigger2,
        PadButton::Select,
        PadButton::Start,
        PadButton::Mode,
        PadButton::LeftThumb,
        PadButton::RightThumb,
        PadButton::DPadUp,
        PadButton::DPadDown,
        PadButton::DPadLeft,
        PadButton::DPadRight,
    ];

    /// The name printed on an Xbox-style pad (ASCII, short).
    pub fn label(self) -> &'static str {
        match self {
            PadButton::South => "A",
            PadButton::East => "B",
            PadButton::North => "Y",
            PadButton::West => "X",
            PadButton::LeftTrigger => "LB",
            PadButton::LeftTrigger2 => "LT",
            PadButton::RightTrigger => "RB",
            PadButton::RightTrigger2 => "RT",
            PadButton::Select => "BACK",
            PadButton::Start => "START",
            PadButton::Mode => "GUIDE",
            PadButton::LeftThumb => "LS",
            PadButton::RightThumb => "RS",
            PadButton::DPadUp => "DPAD UP",
            PadButton::DPadDown => "DPAD DOWN",
            PadButton::DPadLeft => "DPAD LEFT",
            PadButton::DPadRight => "DPAD RIGHT",
        }
    }

    /// The name in a bindings file.
    fn token(self) -> String {
        format!("{self:?}")
    }

    #[cfg(feature = "gamepad")]
    fn to_gilrs(self) -> gilrs::Button {
        use gilrs::Button as B;
        match self {
            PadButton::South => B::South,
            PadButton::East => B::East,
            PadButton::North => B::North,
            PadButton::West => B::West,
            PadButton::LeftTrigger => B::LeftTrigger,
            PadButton::LeftTrigger2 => B::LeftTrigger2,
            PadButton::RightTrigger => B::RightTrigger,
            PadButton::RightTrigger2 => B::RightTrigger2,
            PadButton::Select => B::Select,
            PadButton::Start => B::Start,
            PadButton::Mode => B::Mode,
            PadButton::LeftThumb => B::LeftThumb,
            PadButton::RightThumb => B::RightThumb,
            PadButton::DPadUp => B::DPadUp,
            PadButton::DPadDown => B::DPadDown,
            PadButton::DPadLeft => B::DPadLeft,
            PadButton::DPadRight => B::DPadRight,
        }
    }

    #[cfg(feature = "gamepad")]
    fn from_gilrs(b: gilrs::Button) -> Option<PadButton> {
        PadButton::ALL.into_iter().find(|p| p.to_gilrs() == b)
    }
}

/// The keys a binding may use, with their names (file and screen).
const KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::KeyA, "A"),
    (KeyCode::KeyB, "B"),
    (KeyCode::KeyC, "C"),
    (KeyCode::KeyD, "D"),
    (KeyCode::KeyE, "E"),
    (KeyCode::KeyF, "F"),
    (KeyCode::KeyG, "G"),
    (KeyCode::KeyH, "H"),
    (KeyCode::KeyI, "I"),
    (KeyCode::KeyJ, "J"),
    (KeyCode::KeyK, "K"),
    (KeyCode::KeyL, "L"),
    (KeyCode::KeyM, "M"),
    (KeyCode::KeyN, "N"),
    (KeyCode::KeyO, "O"),
    (KeyCode::KeyP, "P"),
    (KeyCode::KeyQ, "Q"),
    (KeyCode::KeyR, "R"),
    (KeyCode::KeyS, "S"),
    (KeyCode::KeyT, "T"),
    (KeyCode::KeyU, "U"),
    (KeyCode::KeyV, "V"),
    (KeyCode::KeyW, "W"),
    (KeyCode::KeyX, "X"),
    (KeyCode::KeyY, "Y"),
    (KeyCode::KeyZ, "Z"),
    (KeyCode::Digit0, "0"),
    (KeyCode::Digit1, "1"),
    (KeyCode::Digit2, "2"),
    (KeyCode::Digit3, "3"),
    (KeyCode::Digit4, "4"),
    (KeyCode::Digit5, "5"),
    (KeyCode::Digit6, "6"),
    (KeyCode::Digit7, "7"),
    (KeyCode::Digit8, "8"),
    (KeyCode::Digit9, "9"),
    (KeyCode::ArrowUp, "UP"),
    (KeyCode::ArrowDown, "DOWN"),
    (KeyCode::ArrowLeft, "LEFT"),
    (KeyCode::ArrowRight, "RIGHT"),
    (KeyCode::Enter, "ENTER"),
    (KeyCode::Space, "SPACE"),
    (KeyCode::Tab, "TAB"),
    (KeyCode::Backspace, "BACKSPACE"),
    (KeyCode::ShiftLeft, "LSHIFT"),
    (KeyCode::ShiftRight, "RSHIFT"),
    (KeyCode::ControlLeft, "LCTRL"),
    (KeyCode::ControlRight, "RCTRL"),
    (KeyCode::AltLeft, "LALT"),
    (KeyCode::AltRight, "RALT"),
    (KeyCode::Insert, "INSERT"),
    (KeyCode::Delete, "DELETE"),
    (KeyCode::Home, "HOME"),
    (KeyCode::End, "END"),
    (KeyCode::PageUp, "PGUP"),
    (KeyCode::PageDown, "PGDN"),
    (KeyCode::Comma, ","),
    (KeyCode::Period, "."),
    (KeyCode::Slash, "/"),
    (KeyCode::Semicolon, ";"),
    (KeyCode::Quote, "'"),
    (KeyCode::BracketLeft, "["),
    (KeyCode::BracketRight, "]"),
    (KeyCode::Backslash, "\\"),
    (KeyCode::Minus, "-"),
    (KeyCode::Equal, "="),
    (KeyCode::Numpad0, "NUM0"),
    (KeyCode::Numpad1, "NUM1"),
    (KeyCode::Numpad2, "NUM2"),
    (KeyCode::Numpad3, "NUM3"),
    (KeyCode::Numpad4, "NUM4"),
    (KeyCode::Numpad5, "NUM5"),
    (KeyCode::Numpad6, "NUM6"),
    (KeyCode::Numpad7, "NUM7"),
    (KeyCode::Numpad8, "NUM8"),
    (KeyCode::Numpad9, "NUM9"),
    (KeyCode::NumpadEnter, "NUMENTER"),
];

/// A key's name, if a binding may use it.
pub fn key_label(code: KeyCode) -> Option<&'static str> {
    KEYS.iter().find(|(k, _)| *k == code).map(|(_, n)| *n)
}

fn key_from_label(name: &str) -> Option<KeyCode> {
    KEYS.iter().find(|(_, n)| *n == name).map(|(k, _)| *k)
}

/// Which key and which pad buttons press each console button
/// ([`BINDABLE`] order). A pad button may have a second binding (the
/// defaults put L and R on both the bumpers and the triggers).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bindings {
    pub keys: [Option<KeyCode>; 12],
    pub pad: [[Option<PadButton>; 2]; 12],
}

impl Default for Bindings {
    fn default() -> Self {
        use PadButton as P;
        Bindings {
            keys: [
                Some(KeyCode::KeyZ),
                Some(KeyCode::KeyX),
                Some(KeyCode::KeyC),
                Some(KeyCode::KeyV),
                Some(KeyCode::KeyQ),
                Some(KeyCode::KeyE),
                Some(KeyCode::Enter),
                Some(KeyCode::ShiftRight),
                Some(KeyCode::ArrowUp),
                Some(KeyCode::ArrowDown),
                Some(KeyCode::ArrowLeft),
                Some(KeyCode::ArrowRight),
            ],
            pad: [
                [Some(P::South), None],
                [Some(P::East), None],
                [Some(P::West), None],
                [Some(P::North), None],
                [Some(P::LeftTrigger), Some(P::LeftTrigger2)],
                [Some(P::RightTrigger), Some(P::RightTrigger2)],
                [Some(P::Start), Some(P::Mode)],
                [Some(P::Select), Some(P::LeftThumb)],
                [Some(P::DPadUp), None],
                [Some(P::DPadDown), None],
                [Some(P::DPadLeft), None],
                [Some(P::DPadRight), None],
            ],
        }
    }
}

impl Bindings {
    fn slot(button: Button) -> usize {
        BINDABLE.iter().position(|b| *b == button).unwrap_or(0)
    }

    /// `button` on the keyboard is `code` from now on; no other button keeps
    /// it.
    pub fn bind_key(&mut self, button: Button, code: KeyCode) {
        for k in self.keys.iter_mut() {
            if *k == Some(code) {
                *k = None;
            }
        }
        self.keys[Self::slot(button)] = Some(code);
    }

    /// `button` on a gamepad is `pad` from now on (alone); no other button
    /// keeps it.
    pub fn bind_pad(&mut self, button: Button, pad: PadButton) {
        for slots in self.pad.iter_mut() {
            for p in slots.iter_mut() {
                if *p == Some(pad) {
                    *p = None;
                }
            }
        }
        self.pad[Self::slot(button)] = [Some(pad), None];
    }

    /// What `button` is bound to on `device`, for a screen ("Z", "LB/LT").
    pub fn label(&self, button: Button, device: BindDevice) -> Option<String> {
        let i = Self::slot(button);
        match device {
            BindDevice::Keyboard => self.keys[i].and_then(key_label).map(str::to_string),
            BindDevice::Gamepad => {
                let names: Vec<&str> = self.pad[i].iter().flatten().map(|p| p.label()).collect();
                (!names.is_empty()).then(|| names.join("/"))
            }
        }
    }
}

/// The bindings as text, one line per button: `a = Z | South LeftTrigger2`
/// (`-` for none).
pub fn bindings_to_text(b: &Bindings) -> String {
    let mut out = String::new();
    for (i, button) in BINDABLE.iter().enumerate() {
        let key = b.keys[i].and_then(key_label).unwrap_or("-");
        let pads: Vec<String> = b.pad[i].iter().flatten().map(|p| p.token()).collect();
        let pads = if pads.is_empty() {
            "-".to_string()
        } else {
            pads.join(" ")
        };
        out.push_str(&format!("{button:?} = {key} | {pads}\n"));
    }
    out
}

/// Parse [`bindings_to_text`]; unknown or broken lines keep the default.
pub fn bindings_from_text(text: &str) -> Bindings {
    let mut b = Bindings::default();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once('=') else {
            continue;
        };
        let Some(i) = BINDABLE
            .iter()
            .position(|x| format!("{x:?}") == name.trim())
        else {
            continue;
        };
        let Some((key, pads)) = rest.split_once('|') else {
            continue;
        };
        let key = key.trim();
        b.keys[i] = if key == "-" {
            None
        } else {
            key_from_label(key).or(b.keys[i])
        };
        let pads: Vec<PadButton> = pads
            .split_whitespace()
            .filter_map(|t| PadButton::ALL.into_iter().find(|p| p.token() == t))
            .collect();
        if pads.is_empty() && pads_is_dash(rest) {
            b.pad[i] = [None, None];
        } else if !pads.is_empty() {
            b.pad[i] = [pads.first().copied(), pads.get(1).copied()];
        }
    }
    b
}

fn pads_is_dash(rest: &str) -> bool {
    rest.split_once('|').is_some_and(|(_, p)| p.trim() == "-")
}

/// What the host asks of the input backend for the game
/// ([`trino_core::Platform::rebind`] and friends, through
/// [`crate::PcPlatform::take_input_request`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputRequest {
    Rebind(Button, BindDevice),
    Cancel,
    Reset,
}

#[derive(Default)]
pub struct PcInput {
    state: InputState,
    // W, A, S, D held state for the digital stick.
    wasd: [bool; 4],
    bindings: Bindings,
    capture: Option<(Button, BindDevice)>,
    /// The bindings changed since [`PcInput::take_changed`] (persist them).
    changed: bool,
    /// The pad button that just completed a rebind: it presses nothing
    /// until released (else the game would see the press that bound it).
    held_after_bind: Option<PadButton>,
    #[cfg(feature = "gamepad")]
    pads: Option<gilrs::Gilrs>,
}

impl PcInput {
    pub fn new() -> Self {
        #[cfg(feature = "gamepad")]
        {
            // No pad subsystem (headless CI, missing permissions): keyboard
            // and mouse still work.
            Self {
                pads: gilrs::Gilrs::new().ok(),
                ..Self::default()
            }
        }
        #[cfg(not(feature = "gamepad"))]
        Self::default()
    }

    /// The bindings in effect.
    pub fn bindings(&self) -> &Bindings {
        &self.bindings
    }

    /// Replace the bindings (a saved file, at start-up). Buttons held by the
    /// old ones are released.
    pub fn set_bindings(&mut self, b: Bindings) {
        self.bindings = b;
        for button in BINDABLE {
            self.state.set(button, false);
        }
    }

    /// Start or stop a rebind, or reset (from [`crate::PcPlatform`]).
    pub fn apply(&mut self, req: InputRequest) {
        match req {
            InputRequest::Rebind(button, device) => {
                // A pad rebind needs a pad layer to hear the press.
                if device == BindDevice::Gamepad
                    && self.gamepad_status() == GamepadStatus::Unsupported
                {
                    return;
                }
                self.capture = Some((button, device));
            }
            InputRequest::Cancel => self.capture = None,
            InputRequest::Reset => {
                self.capture = None;
                self.set_bindings(Bindings::default());
                self.changed = true;
            }
        }
    }

    /// Did the bindings change since the last call (time to save them)?
    pub fn take_changed(&mut self) -> bool {
        core::mem::take(&mut self.changed)
    }

    /// Gamepad support and connection, and a pending rebind.
    pub fn info(&self) -> InputInfo {
        InputInfo {
            gamepad: self.gamepad_status(),
            capturing: self.capture,
        }
    }

    pub fn gamepad_status(&self) -> GamepadStatus {
        #[cfg(feature = "gamepad")]
        {
            match &self.pads {
                None => GamepadStatus::Unsupported,
                Some(g) if g.gamepads().next().is_some() => GamepadStatus::Connected,
                Some(_) => GamepadStatus::Disconnected,
            }
        }
        #[cfg(not(feature = "gamepad"))]
        GamepadStatus::Unsupported
    }

    /// The first connected gamepad's name.
    pub fn gamepad_name(&self) -> Option<String> {
        #[cfg(feature = "gamepad")]
        {
            self.pads
                .as_ref()
                .and_then(|g| g.gamepads().next().map(|(_, p)| p.name().to_string()))
        }
        #[cfg(not(feature = "gamepad"))]
        None
    }

    /// A pad button went down: completes a pending pad rebind (true), else
    /// nothing.
    pub fn pad_pressed(&mut self, pad: PadButton) -> bool {
        if let Some((button, BindDevice::Gamepad)) = self.capture {
            self.bindings.bind_pad(button, pad);
            self.capture = None;
            self.changed = true;
            self.held_after_bind = Some(pad);
            return true;
        }
        false
    }

    /// Feed a winit keyboard event. `down` is true for press, false for
    /// release; key repeats should be filtered out by the caller. While a
    /// keyboard rebind waits, the next bindable key pressed becomes the
    /// binding (and does nothing else); Escape cancels any rebind.
    pub fn handle_key(&mut self, code: KeyCode, down: bool) {
        if down && self.capture.is_some() {
            if code == KeyCode::Escape {
                self.capture = None;
                return;
            }
            if let Some((button, BindDevice::Keyboard)) = self.capture
                && key_label(code).is_some()
            {
                self.state.set(button, false);
                self.bindings.bind_key(button, code);
                self.capture = None;
                self.changed = true;
                return;
            }
        }
        for (i, button) in BINDABLE.iter().enumerate() {
            if self.bindings.keys[i] == Some(code) {
                self.state.set(*button, down);
            }
        }
        let key = match code {
            KeyCode::Escape => Some(Key::Escape),
            KeyCode::Enter | KeyCode::NumpadEnter => Some(Key::Enter),
            KeyCode::Space => Some(Key::Space),
            KeyCode::Tab => Some(Key::Tab),
            KeyCode::Delete => Some(Key::Delete),
            KeyCode::Backspace => Some(Key::Backspace),
            KeyCode::F1 => Some(Key::F1),
            KeyCode::F2 => Some(Key::F2),
            KeyCode::F3 => Some(Key::F3),
            KeyCode::F4 => Some(Key::F4),
            KeyCode::F5 => Some(Key::F5),
            KeyCode::F6 => Some(Key::F6),
            KeyCode::F7 => Some(Key::F7),
            KeyCode::F8 => Some(Key::F8),
            KeyCode::Digit1 | KeyCode::Numpad1 => Some(Key::Digit1),
            KeyCode::Digit2 | KeyCode::Numpad2 => Some(Key::Digit2),
            KeyCode::Digit3 | KeyCode::Numpad3 => Some(Key::Digit3),
            KeyCode::PageUp => Some(Key::PageUp),
            KeyCode::PageDown => Some(Key::PageDown),
            KeyCode::Home => Some(Key::Home),
            KeyCode::KeyR => Some(Key::R),
            KeyCode::KeyW => Some(Key::W),
            KeyCode::KeyA => Some(Key::A),
            KeyCode::KeyS => Some(Key::S),
            KeyCode::KeyD => Some(Key::D),
            KeyCode::Equal | KeyCode::NumpadAdd => Some(Key::Plus),
            KeyCode::Minus | KeyCode::NumpadSubtract => Some(Key::Minus),
            KeyCode::ShiftLeft | KeyCode::ShiftRight => Some(Key::Shift),
            KeyCode::ControlLeft | KeyCode::ControlRight => Some(Key::Ctrl),
            _ => None,
        };
        if let Some(k) = key {
            self.state.set_key(k, down);
        }
        match code {
            KeyCode::KeyW => self.wasd[0] = down,
            KeyCode::KeyA => self.wasd[1] = down,
            KeyCode::KeyS => self.wasd[2] = down,
            KeyCode::KeyD => self.wasd[3] = down,
            _ => {}
        }
        // Digital stick: Y-up positive, matching real console sticks.
        self.state.stick = Vec2::new(
            (self.wasd[3] as i8 - self.wasd[1] as i8) as f32,
            (self.wasd[0] as i8 - self.wasd[2] as i8) as f32,
        );
    }
}

impl PcInput {
    /// Feed any winit window event: keyboard (no repeats), cursor motion
    /// (mapped to game pixels through `renderer`), mouse buttons and wheel.
    /// Returns true when the event was input.
    pub fn handle_window_event(
        &mut self,
        event: &WindowEvent,
        renderer: &crate::PcRenderer,
    ) -> bool {
        match event {
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat: false,
                        ..
                    },
                ..
            } => self.handle_key(*code, *state == ElementState::Pressed),
            WindowEvent::CursorMoved { position, .. } => {
                match renderer.window_to_pointer(position.x, position.y) {
                    Some((p, screen)) => self.handle_pointer_on(Some(p), screen),
                    None => self.handle_pointer(None),
                }
            }
            WindowEvent::CursorLeft { .. } => self.handle_pointer(None),
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(*button, *state == ElementState::Pressed);
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_wheel(match delta {
                MouseScrollDelta::LineDelta(_, y) => *y,
                // Touchpads report pixels; ~40 px per notch.
                MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
            }),
            _ => return false,
        }
        true
    }

    /// Mouse moved: `pos` in game pixels, `None` outside the picture.
    pub fn handle_pointer(&mut self, pos: Option<Vec2>) {
        self.handle_pointer_on(pos, 0);
    }

    /// Mouse moved onto screen `screen` (1 = the simulated 3DS touch
    /// screen), `pos` in that screen's pixels.
    pub fn handle_pointer_on(&mut self, pos: Option<Vec2>, screen: u8) {
        self.state.pointer = pos;
        self.state.pointer_screen = screen;
    }

    /// Feed a winit mouse button event.
    pub fn handle_mouse_button(&mut self, button: MouseButton, down: bool) {
        let b = match button {
            MouseButton::Left => PointerButton::Primary,
            MouseButton::Right => PointerButton::Secondary,
            MouseButton::Middle => PointerButton::Middle,
            _ => return,
        };
        self.state.set_pointer_button(b, down);
    }

    /// Wheel motion in notches (positive = away from the user). Accumulates
    /// until the next [`Input::poll`].
    pub fn handle_wheel(&mut self, notches: f32) {
        self.state.wheel += notches;
    }
}

#[cfg(feature = "gamepad")]
impl PcInput {
    /// Fold every connected gamepad into `state` (buttons OR'ed, the
    /// strongest stick wins over the keyboard's digital stick).
    fn merge_pads(&mut self, state: &mut InputState) {
        use gilrs::{Axis, EventType};
        let mut pressed = Vec::new();
        {
            let Some(g) = self.pads.as_mut() else { return };
            while let Some(ev) = g.next_event() {
                if let EventType::ButtonPressed(b, _) = ev.event
                    && let Some(p) = PadButton::from_gilrs(b)
                {
                    pressed.push(p);
                }
            }
        }
        for p in pressed {
            self.pad_pressed(p);
        }
        let bindings = self.bindings;
        let held = self.held_after_bind;
        let Some(g) = self.pads.as_mut() else { return };
        if let Some(h) = held
            && !g.gamepads().any(|(_, p)| p.is_pressed(h.to_gilrs()))
        {
            self.held_after_bind = None;
        }
        let held = self.held_after_bind;
        let Some(g) = self.pads.as_mut() else { return };
        for (_, pad) in g.gamepads() {
            for (i, to) in BINDABLE.iter().enumerate() {
                for from in bindings.pad[i].iter().flatten() {
                    if Some(*from) != held && pad.is_pressed(from.to_gilrs()) {
                        state.set(*to, true);
                        state.pad = true;
                    }
                }
            }
            // gilrs sticks are Y-up, like InputState.
            let s = Vec2::new(pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
            if s.x * s.x + s.y * s.y > 0.25 * 0.25 {
                state.pad = true;
            }
            if s.x * s.x + s.y * s.y > state.stick.x * state.stick.x + state.stick.y * state.stick.y
            {
                state.stick = s;
            }
        }
    }
}

impl Input for PcInput {
    fn poll(&mut self) -> InputState {
        #[allow(unused_mut)]
        let mut state = self.state;
        #[cfg(feature = "gamepad")]
        self.merge_pads(&mut state);
        // The wheel is a per-frame delta; everything else is held state.
        self.state.wheel = 0.0;
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_map() {
        let mut input = PcInput::new();
        input.handle_key(KeyCode::KeyZ, true);
        input.handle_key(KeyCode::Enter, true);
        input.handle_key(KeyCode::ArrowLeft, true);
        let s = input.poll();
        assert!(s.is_down(Button::A));
        assert!(s.is_down(Button::Start));
        assert!(s.is_down(Button::DpadLeft));
        assert!(!s.is_down(Button::B));

        input.handle_key(KeyCode::KeyZ, false);
        assert!(!input.poll().is_down(Button::A));
    }

    #[test]
    fn wasd_builds_stick_vector() {
        let mut input = PcInput::new();
        input.handle_key(KeyCode::KeyD, true);
        input.handle_key(KeyCode::KeyW, true);
        assert_eq!(input.poll().stick, Vec2::new(1.0, 1.0));

        input.handle_key(KeyCode::KeyD, false);
        input.handle_key(KeyCode::KeyA, true);
        input.handle_key(KeyCode::KeyW, false);
        input.handle_key(KeyCode::KeyS, true);
        assert_eq!(input.poll().stick, Vec2::new(-1.0, -1.0));

        input.handle_key(KeyCode::KeyA, false);
        input.handle_key(KeyCode::KeyS, false);
        assert_eq!(input.poll().stick, Vec2::ZERO);
    }

    #[test]
    fn mouse_buttons_pointer_and_wheel() {
        let mut input = PcInput::new();
        input.handle_pointer(Some(Vec2::new(12.0, 34.0)));
        input.handle_mouse_button(MouseButton::Left, true);
        input.handle_mouse_button(MouseButton::Right, true);
        input.handle_wheel(1.0);
        input.handle_wheel(0.5);
        let s = input.poll();
        assert_eq!(s.pointer, Some(Vec2::new(12.0, 34.0)));
        assert!(s.pointer_down(PointerButton::Primary));
        assert!(s.pointer_down(PointerButton::Secondary));
        assert_eq!(s.wheel, 1.5);
        // The wheel resets per poll; buttons and position are held.
        let s = input.poll();
        assert_eq!(s.wheel, 0.0);
        assert!(s.pointer_down(PointerButton::Primary));
        input.handle_mouse_button(MouseButton::Left, false);
        input.handle_pointer(None);
        let s = input.poll();
        assert!(!s.pointer_down(PointerButton::Primary));
        assert_eq!(s.pointer, None);
    }

    #[test]
    fn pc_keys_set_their_bits_too() {
        let mut input = PcInput::new();
        input.handle_key(KeyCode::Escape, true);
        input.handle_key(KeyCode::KeyW, true);
        input.handle_key(KeyCode::F3, true);
        input.handle_key(KeyCode::Numpad2, true);
        input.handle_key(KeyCode::Enter, true);
        let s = input.poll();
        assert!(s.key(Key::Escape) && s.key(Key::W) && s.key(Key::F3) && s.key(Key::Digit2));
        // Enter keeps its console button (Start) and gets its own bit.
        assert!(s.key(Key::Enter) && s.is_down(Button::Start));
        // W still drives the digital stick (console-style games).
        assert_eq!(s.stick, Vec2::new(0.0, 1.0));
        assert!(!s.pad);
        input.handle_key(KeyCode::Escape, false);
        assert!(!input.poll().key(Key::Escape));
    }

    #[test]
    fn opposite_keys_cancel() {
        let mut input = PcInput::new();
        input.handle_key(KeyCode::KeyA, true);
        input.handle_key(KeyCode::KeyD, true);
        assert_eq!(input.poll().stick.x, 0.0);
    }
}
