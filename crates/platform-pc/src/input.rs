//! Keyboard + mouse + gamepad input backend. Gamepads come through gilrs
//! with the `gamepad` feature (Xbox-style layout: A/B/X/Y = South/East/
//! West/North, L/R = bumpers or triggers, L2/R2 = triggers, L3/R3 = stick
//! clicks, both sticks, the analog triggers and the D-pad); every connected
//! pad is merged with the keyboard ([`PcInput::merge_pad`]).
//!
//! The mouse is the [`InputState::pointer`]: the host converts window
//! pixels to game pixels ([`crate::PcRenderer::window_to_game`]) and feeds
//! [`PcInput::handle_pointer`], [`PcInput::handle_mouse_button`] and
//! [`PcInput::handle_wheel`]. Left = `Primary`, right = `Secondary`,
//! middle = `Middle`.
//!
//! Default mapping (chosen to avoid key overlaps):
//!
//! | Trino     | Key         | Gamepad         |
//! |-----------|-------------|-----------------|
//! | A / B     | Z / X       | A / B           |
//! | X / Y     | C / V       | X / Y           |
//! | L / R     | Q / E       | LB, LT / RB, RT |
//! | L2 / R2   | (none)      | LT / RT         |
//! | L3 / R3   | (none)      | LS / RS         |
//! | Start     | Enter       | START, GUIDE    |
//! | Select    | Right Shift | BACK, LS        |
//! | D-pad     | Arrows      | D-pad           |
//! | Stick     | WASD        | Left stick      |
//!
//! `right_stick` is the pad's right stick; `triggers` the pad's analog
//! triggers, or 1.0 while a key bound to L2/R2 is held.
//!
//! The console buttons can be **rebound** (keyboard and gamepad separately):
//! [`Bindings`] holds the table, [`PcInput::apply`] starts a capture (the
//! next key or pad button pressed becomes the button's binding), and
//! [`bindings_to_text`] / [`bindings_from_text`] persist it (the runner
//! keeps `<save_name>.input`). The stick's WASD and the shortcut keys stay
//! fixed (see [`key_label`]).
//!
//! Every key also sets its [`Key`] bit in [`InputState::keys`]
//! ([`keys_for`]: letters, digits, F1–F12, arrows, navigation, modifiers,
//! punctuation), so a game can offer PC shortcuts on top of the console
//! mapping. Bound keys set theirs too (Z sets `Key::Z` and presses A).

use trino_core::{
    BindDevice, Button, DeviceKind, GamepadStatus, Input, InputInfo, InputState, Key, MAX_PLAYERS,
    PadFamily, PlayersInput, PointerButton, Seats, Vec2,
};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// The console buttons that can be rebound, in [`Bindings`] order (and the
/// order of a bindings file's lines).
pub const BINDABLE: [Button; 16] = Button::ALL;

/// How many buttons [`Bindings`] holds.
pub const BINDINGS: usize = BINDABLE.len();

/// A gamepad button (the standard Xbox-style layout gilrs reports).
/// Discriminants follow [`PadButton::ALL`] (bits of [`PadReading::held`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
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

/// The keys a binding may use, with their names (file and screen). Keys
/// with a fixed role are left out, so a rebind never makes one press do two
/// things: W/A/S/D (the stick) and the shortcut keys that had a [`Key`]
/// bit before the full keyboard (Esc, Space, Tab, Delete, Backspace, F1–F8,
/// 1–3, Page Up/Down, Home, R, T, =/+, −, Ctrl, numpad 1–3/+/−), plus Super
/// (the OS takes it). Every other key is bindable, though it also sets its
/// own [`Key`] bit ([`keys_for`]).
const KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::KeyB, "B"),
    (KeyCode::KeyC, "C"),
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
    (KeyCode::KeyT, "T"),
    (KeyCode::KeyU, "U"),
    (KeyCode::KeyV, "V"),
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
    (KeyCode::Space, "SPACE"),
    (KeyCode::Tab, "TAB"),
    (KeyCode::Backspace, "BACKSPACE"),
    (KeyCode::ArrowUp, "UP"),
    (KeyCode::ArrowDown, "DOWN"),
    (KeyCode::ArrowLeft, "LEFT"),
    (KeyCode::ArrowRight, "RIGHT"),
    (KeyCode::Enter, "ENTER"),
    (KeyCode::ShiftLeft, "LSHIFT"),
    (KeyCode::ShiftRight, "RSHIFT"),
    (KeyCode::AltLeft, "LALT"),
    (KeyCode::AltRight, "RALT"),
    (KeyCode::Insert, "INSERT"),
    (KeyCode::End, "END"),
    (KeyCode::Comma, ","),
    (KeyCode::Period, "."),
    (KeyCode::Slash, "/"),
    (KeyCode::Semicolon, ";"),
    (KeyCode::Quote, "'"),
    (KeyCode::BracketLeft, "["),
    (KeyCode::BracketRight, "]"),
    (KeyCode::Backslash, "\\"),
    (KeyCode::Backquote, "`"),
    (KeyCode::F9, "F9"),
    (KeyCode::F10, "F10"),
    (KeyCode::F11, "F11"),
    (KeyCode::F12, "F12"),
    (KeyCode::Numpad0, "NUM0"),
    (KeyCode::Numpad4, "NUM4"),
    (KeyCode::Numpad5, "NUM5"),
    (KeyCode::Numpad6, "NUM6"),
    (KeyCode::Numpad7, "NUM7"),
    (KeyCode::Numpad8, "NUM8"),
    (KeyCode::Numpad9, "NUM9"),
    (KeyCode::NumpadEnter, "NUMENTER"),
];

/// The [`Key`] bits a key sets (empty for keys without one). The numpad's
/// 1–3, Enter, + and − alias their main-row keys; the main row's `=` sets
/// both [`Key::Equal`] and [`Key::Plus`].
pub fn keys_for(code: KeyCode) -> &'static [Key] {
    use KeyCode as C;
    match code {
        C::Escape => &[Key::Escape],
        C::Enter | C::NumpadEnter => &[Key::Enter],
        C::Space => &[Key::Space],
        C::Tab => &[Key::Tab],
        C::Delete => &[Key::Delete],
        C::Backspace => &[Key::Backspace],
        C::Insert => &[Key::Insert],
        C::Home => &[Key::Home],
        C::End => &[Key::End],
        C::PageUp => &[Key::PageUp],
        C::PageDown => &[Key::PageDown],
        C::ArrowUp => &[Key::ArrowUp],
        C::ArrowDown => &[Key::ArrowDown],
        C::ArrowLeft => &[Key::ArrowLeft],
        C::ArrowRight => &[Key::ArrowRight],
        C::F1 => &[Key::F1],
        C::F2 => &[Key::F2],
        C::F3 => &[Key::F3],
        C::F4 => &[Key::F4],
        C::F5 => &[Key::F5],
        C::F6 => &[Key::F6],
        C::F7 => &[Key::F7],
        C::F8 => &[Key::F8],
        C::F9 => &[Key::F9],
        C::F10 => &[Key::F10],
        C::F11 => &[Key::F11],
        C::F12 => &[Key::F12],
        C::Digit0 => &[Key::Digit0],
        C::Digit1 | C::Numpad1 => &[Key::Digit1],
        C::Digit2 | C::Numpad2 => &[Key::Digit2],
        C::Digit3 | C::Numpad3 => &[Key::Digit3],
        C::Digit4 => &[Key::Digit4],
        C::Digit5 => &[Key::Digit5],
        C::Digit6 => &[Key::Digit6],
        C::Digit7 => &[Key::Digit7],
        C::Digit8 => &[Key::Digit8],
        C::Digit9 => &[Key::Digit9],
        C::KeyA => &[Key::A],
        C::KeyB => &[Key::B],
        C::KeyC => &[Key::C],
        C::KeyD => &[Key::D],
        C::KeyE => &[Key::E],
        C::KeyF => &[Key::F],
        C::KeyG => &[Key::G],
        C::KeyH => &[Key::H],
        C::KeyI => &[Key::I],
        C::KeyJ => &[Key::J],
        C::KeyK => &[Key::K],
        C::KeyL => &[Key::L],
        C::KeyM => &[Key::M],
        C::KeyN => &[Key::N],
        C::KeyO => &[Key::O],
        C::KeyP => &[Key::P],
        C::KeyQ => &[Key::Q],
        C::KeyR => &[Key::R],
        C::KeyS => &[Key::S],
        C::KeyT => &[Key::T],
        C::KeyU => &[Key::U],
        C::KeyV => &[Key::V],
        C::KeyW => &[Key::W],
        C::KeyX => &[Key::X],
        C::KeyY => &[Key::Y],
        C::KeyZ => &[Key::Z],
        C::Equal => &[Key::Equal, Key::Plus],
        C::NumpadAdd => &[Key::Plus],
        C::Minus | C::NumpadSubtract => &[Key::Minus],
        C::ShiftLeft | C::ShiftRight => &[Key::Shift],
        C::ControlLeft | C::ControlRight => &[Key::Ctrl],
        C::AltLeft | C::AltRight => &[Key::Alt],
        C::SuperLeft | C::SuperRight => &[Key::Super],
        C::BracketLeft => &[Key::BracketLeft],
        C::BracketRight => &[Key::BracketRight],
        C::Semicolon => &[Key::Semicolon],
        C::Quote => &[Key::Quote],
        C::Comma => &[Key::Comma],
        C::Period => &[Key::Period],
        C::Slash => &[Key::Slash],
        C::Backslash => &[Key::Backslash],
        C::Backquote => &[Key::Backquote],
        _ => &[],
    }
}

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
    pub keys: [Option<KeyCode>; BINDINGS],
    pub pad: [[Option<PadButton>; 2]; BINDINGS],
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
                // L2, R2, L3, R3: no key (every free key is someone's
                // shortcut); a player can bind them.
                None,
                None,
                None,
                None,
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
                // The triggers and stick clicks also keep pressing L, R and
                // Select, as they did before L2–R3 existed.
                [Some(P::LeftTrigger2), None],
                [Some(P::RightTrigger2), None],
                [Some(P::LeftThumb), None],
                [Some(P::RightThumb), None],
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

/// The bindings as text, one line per button: `A = Z | South LeftTrigger2`
/// (`-` for none). Files written before L2–R3 existed have only the first
/// twelve lines; [`bindings_from_text`] reads them unchanged.
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
///
/// A button without a line (an older file) keeps its defaults, except a
/// key or pad button the file gives to another button that didn't have it
/// by default: a binding the player moved is not handed back.
pub fn bindings_from_text(text: &str) -> Bindings {
    let mut b = Bindings::default();
    let mut seen = [false; BINDINGS];
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
        seen[i] = true;
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
    // A default the file moved elsewhere stays with the button it went to.
    let (def, parsed) = (Bindings::default(), b);
    let moved_key = |k: KeyCode| {
        (0..BINDINGS).any(|j| seen[j] && parsed.keys[j] == Some(k) && def.keys[j] != Some(k))
    };
    let moved_pad = |p: PadButton| {
        (0..BINDINGS)
            .any(|j| seen[j] && parsed.pad[j].contains(&Some(p)) && !def.pad[j].contains(&Some(p)))
    };
    for i in (0..BINDINGS).filter(|i| !seen[*i]) {
        b.keys[i] = b.keys[i].filter(|k| !moved_key(*k));
        let mut kept = b.pad[i].into_iter().flatten().filter(|p| !moved_pad(*p));
        b.pad[i] = [kept.next(), kept.next()];
    }
    b
}

fn pads_is_dash(rest: &str) -> bool {
    rest.split_once('|').is_some_and(|(_, p)| p.trim() == "-")
}

/// Every player's bindings as text: player 1's lines as
/// [`bindings_to_text`] writes them (a single-player file is unchanged
/// apart from its first line), then, for each other player whose bindings
/// differ from the defaults, the same lines prefixed `P2 `, `P3 `, `P4 `.
/// The first line names the version (`# trino-input 2`); readers before
/// issue #67 skip it and the prefixed lines.
pub fn players_bindings_to_text(all: &[Bindings; MAX_PLAYERS]) -> String {
    let mut out = String::from("# trino-input 2\n");
    out.push_str(&bindings_to_text(&all[0]));
    for (p, b) in all.iter().enumerate().skip(1) {
        if *b == Bindings::default() {
            continue;
        }
        for line in bindings_to_text(b).lines() {
            out.push_str(&format!("P{} {line}\n", p + 1));
        }
    }
    out
}

/// Parse [`players_bindings_to_text`] (and every older file: unprefixed
/// lines are player 1's, a player without lines keeps the defaults).
pub fn players_bindings_from_text(text: &str) -> [Bindings; MAX_PLAYERS] {
    let mut per: [String; MAX_PLAYERS] = Default::default();
    for line in text.lines() {
        let t = line.trim_start();
        let player = t
            .strip_prefix('P')
            .and_then(|r| r.split_once(' '))
            .and_then(|(n, rest)| Some((n.parse::<usize>().ok()?, rest)))
            .filter(|(n, _)| (2..=MAX_PLAYERS).contains(n));
        match player {
            Some((n, rest)) => {
                per[n - 1].push_str(rest);
                per[n - 1].push('\n');
            }
            None => {
                per[0].push_str(line);
                per[0].push('\n');
            }
        }
    }
    core::array::from_fn(|p| bindings_from_text(&per[p]))
}

/// What the host asks of the input backend for the game
/// ([`trino_core::Platform::rebind`] and friends, through
/// [`crate::PcPlatform::take_input_request`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputRequest {
    /// Player `.0`'s `button` on `device` takes the next press.
    Rebind(u8, Button, BindDevice),
    Cancel,
    /// Every player's bindings back to the defaults.
    Reset,
}

#[derive(Default)]
pub struct PcInput {
    /// The keyboard and mouse: keys, WASD stick, pointer, wheel. Its bound
    /// buttons come from the held keys at each poll (its seat's bindings).
    state: InputState,
    /// Bindable keys held, in press order.
    held: Vec<KeyCode>,
    // W, A, S, D held state for the digital stick.
    wasd: [bool; 4],
    /// Each player's bindings (issue #67): a device plays with its seat's.
    bindings: [Bindings; MAX_PLAYERS],
    capture: Option<(u8, Button, BindDevice)>,
    /// The bindings changed since [`PcInput::take_changed`] (persist them).
    changed: bool,
    /// The pad button that just completed a rebind: it presses nothing
    /// until released (else the game would see the press that bound it).
    held_after_bind: Option<PadButton>,
    /// The first connected pad as the pad layer names it ([`PcInput::note_pad`]).
    pad: Option<PadIdent>,
    /// The pads this frame, by the pad layer's own id: what
    /// [`feed_pads`](Self::feed_pads) was told.
    pads_now: Vec<FedPad>,
    /// The keyboard and the pads as devices with seats (issue #67).
    seats: Seats,
    #[cfg(feature = "gamepad")]
    gilrs: Option<gilrs::Gilrs>,
}

/// One pad as the pad layer (or a test, or the editor's split-input
/// simulator) reports it for a frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FedPad {
    /// The pad layer's id (stable while the pad stays connected).
    pub key: u32,
    pub reading: PadReading,
    pub family: Option<PadFamily>,
}

/// A connected pad's identity, copied out of the pad layer: its name and,
/// where the system reports them, its USB vendor and product ids.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PadIdent {
    pub name: String,
    pub vendor: Option<u16>,
    pub product: Option<u16>,
}

impl PadIdent {
    /// Its family ([`PadFamily::detect`]: the ids where they name one,
    /// else the name, else generic).
    pub fn family(&self) -> PadFamily {
        let usb = self.vendor.zip(self.product);
        PadFamily::detect(&self.name, usb)
    }
}

impl PcInput {
    pub fn new() -> Self {
        #[cfg(feature = "gamepad")]
        {
            // No pad subsystem (headless CI, missing permissions): keyboard
            // and mouse still work.
            Self {
                gilrs: gilrs::Gilrs::new().ok(),
                ..Self::default()
            }
        }
        #[cfg(not(feature = "gamepad"))]
        Self::default()
    }

    /// Player 1's bindings.
    pub fn bindings(&self) -> &Bindings {
        &self.bindings[0]
    }

    /// Player `player`'s bindings (player 1's past [`MAX_PLAYERS`]).
    pub fn bindings_for(&self, player: u8) -> &Bindings {
        &self.bindings[(player as usize).min(MAX_PLAYERS - 1)]
    }

    /// Every player's bindings (what the runner persists).
    pub fn all_bindings(&self) -> &[Bindings; MAX_PLAYERS] {
        &self.bindings
    }

    /// Replace player 1's bindings (a saved file, at start-up). Keys held
    /// under the old ones are released.
    pub fn set_bindings(&mut self, b: Bindings) {
        self.bindings[0] = b;
        self.held.clear();
    }

    /// Replace every player's bindings (a saved file, at start-up).
    pub fn set_all_bindings(&mut self, all: [Bindings; MAX_PLAYERS]) {
        self.bindings = all;
        self.held.clear();
    }

    /// The keyboard and pads as devices with seats.
    pub fn seats(&self) -> &Seats {
        &self.seats
    }

    pub fn seats_mut(&mut self) -> &mut Seats {
        &mut self.seats
    }

    /// Start or stop a rebind, or reset (from [`crate::PcPlatform`]).
    pub fn apply(&mut self, req: InputRequest) {
        match req {
            InputRequest::Rebind(player, button, device) => {
                // A pad rebind needs a pad layer to hear the press.
                if device == BindDevice::Gamepad
                    && self.gamepad_status() == GamepadStatus::Unsupported
                {
                    return;
                }
                let player = player.min(MAX_PLAYERS as u8 - 1);
                self.capture = Some((player, button, device));
            }
            InputRequest::Cancel => self.capture = None,
            InputRequest::Reset => {
                self.capture = None;
                self.set_all_bindings([Bindings::default(); MAX_PLAYERS]);
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
            capturing: self.capture.map(|(_, b, d)| (b, d)),
        }
    }

    /// The player a pending rebind is for.
    pub fn capturing_player(&self) -> Option<u8> {
        self.capture.map(|(p, _, _)| p)
    }

    pub fn gamepad_status(&self) -> GamepadStatus {
        #[cfg(feature = "gamepad")]
        {
            match &self.gilrs {
                None => GamepadStatus::Unsupported,
                Some(g) if g.gamepads().next().is_some() => GamepadStatus::Connected,
                Some(_) if !self.pads_now.is_empty() => GamepadStatus::Connected,
                Some(_) => GamepadStatus::Disconnected,
            }
        }
        #[cfg(not(feature = "gamepad"))]
        if self.pads_now.is_empty() {
            GamepadStatus::Unsupported
        } else {
            GamepadStatus::Connected
        }
    }

    /// The first connected gamepad's name.
    pub fn gamepad_name(&self) -> Option<String> {
        if let Some(p) = &self.pad {
            return Some(p.name.clone());
        }
        #[cfg(feature = "gamepad")]
        {
            self.gilrs
                .as_ref()
                .and_then(|g| g.gamepads().next().map(|(_, p)| p.name().to_string()))
        }
        #[cfg(not(feature = "gamepad"))]
        None
    }

    /// The first connected gamepad's family (`None` without one), from what
    /// [`note_pad`](Self::note_pad) was told.
    pub fn gamepad_family(&self) -> Option<PadFamily> {
        self.pad.as_ref().map(PadIdent::family)
    }

    /// The pad layer's first connected pad (`None`: none). The gilrs glue
    /// calls it every poll with copied values; a host without gilrs, or a
    /// test, calls it directly.
    pub fn note_pad(&mut self, pad: Option<PadIdent>) {
        self.pad = pad;
    }

    /// The pads connected this frame (the gilrs glue calls it before every
    /// poll; tests and the editor's split-input simulator call it
    /// directly). Each becomes a [`DeviceKind::Gamepad`] with its seat.
    pub fn feed_pads(&mut self, pads: &[FedPad]) {
        self.pads_now.clear();
        self.pads_now.extend_from_slice(pads);
        if let Some(h) = self.held_after_bind
            && !pads.iter().any(|p| p.reading.is_held(h))
        {
            self.held_after_bind = None;
        }
    }

    /// A pad button went down: completes a pending pad rebind (true), else
    /// nothing.
    pub fn pad_pressed(&mut self, pad: PadButton) -> bool {
        if let Some((player, button, BindDevice::Gamepad)) = self.capture {
            self.bindings[player as usize].bind_pad(button, pad);
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
            if let Some((player, button, BindDevice::Keyboard)) = self.capture
                && key_label(code).is_some()
            {
                self.held.retain(|k| *k != code);
                self.bindings[player as usize].bind_key(button, code);
                self.capture = None;
                self.changed = true;
                return;
            }
        }
        if down {
            if !self.held.contains(&code) {
                self.held.push(code);
            }
        } else {
            self.held.retain(|k| *k != code);
        }
        for k in keys_for(code) {
            self.state.set_key(*k, down);
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

    /// The keyboard's input with `bindings`: its keys, stick and pointer,
    /// the buttons its held keys are bound to, full triggers for keys on
    /// L2/R2.
    fn keyboard_state(&self, bindings: &Bindings) -> InputState {
        let mut state = self.state;
        for (i, button) in BINDABLE.iter().enumerate() {
            if bindings.keys[i].is_some_and(|k| self.held.contains(&k)) {
                state.set(*button, true);
            }
        }
        // Keys bound to L2/R2 are fully pulled triggers.
        state.triggers = Vec2::new(
            state.is_down(Button::L2) as u8 as f32,
            state.is_down(Button::R2) as u8 as f32,
        );
        state
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

/// One gamepad's state for a frame, independent of gilrs: what
/// [`PcInput::merge_pad`] folds into [`InputState`] (and what tests build
/// without a device).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct PadReading {
    /// Held buttons: bit `p as u32` for each [`PadButton`] `p`.
    pub held: u32,
    /// Left stick, -1..=1 per axis, Y-up.
    pub left_stick: Vec2,
    /// Right stick, -1..=1 per axis, Y-up.
    pub right_stick: Vec2,
    /// Analog triggers, 0..=1: `x` = left (LT), `y` = right (RT).
    pub triggers: Vec2,
}

impl PadReading {
    pub fn is_held(&self, p: PadButton) -> bool {
        self.held & (1 << p as u32) != 0
    }

    pub fn press(&mut self, p: PadButton) {
        self.held |= 1 << p as u32;
    }
}

/// A trigger's travel, 0..=1, from what gilrs reports: the trigger button's
/// analog value (SDL-mapped pads), its Z axis (raw pads, negative at rest
/// on some), whichever is larger. A trigger reported pressed with no
/// analog value (a digital trigger) reads 1.
pub fn trigger_value(pressed: bool, button_value: f32, z_axis: f32) -> f32 {
    let v = button_value.max(z_axis).clamp(0.0, 1.0);
    if pressed && v <= 0.0 { 1.0 } else { v }
}

/// A stick counts as "the pad was used" past this radius.
const PAD_DEADZONE: f32 = 0.25;

/// The stronger of two stick vectors (the keyboard's and a pad's, or two
/// pads'). Returns whether `pad` is past the deadzone.
fn fold_stick(into: &mut Vec2, pad: Vec2) -> bool {
    let len2 = |v: Vec2| v.x * v.x + v.y * v.y;
    if len2(pad) > len2(*into) {
        *into = pad;
    }
    len2(pad) > PAD_DEADZONE * PAD_DEADZONE
}

impl PcInput {
    /// Fold one gamepad into `state` with player 1's bindings, as
    /// [`Input::poll`] does for every pad on player 1: bound buttons are
    /// OR'ed in, the stronger stick wins (each of the two), triggers take
    /// the larger value. Sets [`InputState::pad`] when a button, a stick
    /// past the deadzone or a trigger past a quarter is used.
    pub fn merge_pad(&self, state: &mut InputState, pad: &PadReading) {
        self.merge_pad_with(&self.bindings[0], state, pad);
    }

    fn merge_pad_with(&self, bindings: &Bindings, state: &mut InputState, pad: &PadReading) {
        for (i, to) in BINDABLE.iter().enumerate() {
            for from in bindings.pad[i].iter().flatten() {
                if Some(*from) != self.held_after_bind && pad.is_held(*from) {
                    state.set(*to, true);
                    state.pad = true;
                }
            }
        }
        if fold_stick(&mut state.stick, pad.left_stick) {
            state.pad = true;
        }
        if fold_stick(&mut state.right_stick, pad.right_stick) {
            state.pad = true;
        }
        let t = Vec2::new(
            pad.triggers.x.clamp(0.0, 1.0),
            pad.triggers.y.clamp(0.0, 1.0),
        );
        if t.x > PAD_DEADZONE || t.y > PAD_DEADZONE {
            state.pad = true;
        }
        state.triggers = Vec2::new(state.triggers.x.max(t.x), state.triggers.y.max(t.y));
    }

    /// The bindings a device seated on `seat` plays with (player 1's when
    /// unseated, or for one player).
    fn seat_bindings(&self, seat: Option<u8>) -> &Bindings {
        match seat {
            Some(s) if self.seats.setup().max > 1 => self.bindings_for(s),
            _ => &self.bindings[0],
        }
    }
}

#[cfg(feature = "gamepad")]
impl PcInput {
    /// One gilrs pad as a [`PadReading`].
    fn read_gilrs(pad: &gilrs::Gamepad<'_>) -> PadReading {
        use gilrs::{Axis, Button as B};
        let mut r = PadReading::default();
        for p in PadButton::ALL {
            if pad.is_pressed(p.to_gilrs()) {
                r.press(p);
            }
        }
        let value = |b: B| pad.button_data(b).map_or(0.0, |d| d.value());
        // gilrs sticks are Y-up, like InputState.
        r.left_stick = Vec2::new(pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
        r.right_stick = Vec2::new(pad.value(Axis::RightStickX), pad.value(Axis::RightStickY));
        r.triggers = Vec2::new(
            trigger_value(
                r.is_held(PadButton::LeftTrigger2),
                value(B::LeftTrigger2),
                pad.value(Axis::LeftZ),
            ),
            trigger_value(
                r.is_held(PadButton::RightTrigger2),
                value(B::RightTrigger2),
                pad.value(Axis::RightZ),
            ),
        );
        r
    }

    /// Read every connected gamepad ([`PcInput::feed_pads`]).
    fn read_pads(&mut self) {
        use gilrs::EventType;
        let mut pressed = Vec::new();
        {
            let Some(g) = self.gilrs.as_mut() else { return };
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
        let Some(g) = self.gilrs.as_ref() else { return };
        let fed: Vec<FedPad> = g
            .gamepads()
            .map(|(id, p)| FedPad {
                key: usize::from(id) as u32,
                reading: Self::read_gilrs(&p),
                family: Some(
                    PadIdent {
                        name: p.name().to_string(),
                        vendor: p.vendor_id(),
                        product: p.product_id(),
                    }
                    .family(),
                ),
            })
            .collect();
        let first = g.gamepads().next().map(|(_, p)| PadIdent {
            name: p.name().to_string(),
            vendor: p.vendor_id(),
            product: p.product_id(),
        });
        self.note_pad(first);
        // No pad on gilrs leaves what a test or the editor fed this frame.
        if !fed.is_empty() || self.pads_now.is_empty() {
            self.feed_pads(&fed);
        }
    }
}

impl Input for PcInput {
    fn poll(&mut self) -> InputState {
        self.poll_players().players[0]
    }

    /// The keyboard and every pad as devices, folded by seat (issue #67).
    /// With one player (the default) every device is player 0: the
    /// keyboard's state with each pad merged in, as before #67.
    fn poll_players(&mut self) -> PlayersInput {
        #[cfg(feature = "gamepad")]
        self.read_pads();
        self.seats.begin();
        // Each device is reported first to learn its seat (a new one is
        // seated then), then again with its input under that seat's
        // bindings.
        let kb_seat = self
            .seats
            .report(DeviceKind::Keyboard, 0, None, InputState::IDLE)
            .and_then(|id| self.seats.seat_of(id));
        let kb = self.keyboard_state(self.seat_bindings(kb_seat.or(Some(0))));
        self.seats.report(DeviceKind::Keyboard, 0, None, kb);
        let pads = core::mem::take(&mut self.pads_now);
        for p in &pads {
            let seat = self
                .seats
                .report(DeviceKind::Gamepad, p.key, p.family, InputState::IDLE)
                .and_then(|id| self.seats.seat_of(id));
            let mut state = InputState::IDLE;
            self.merge_pad_with(self.seat_bindings(seat), &mut state, &p.reading);
            self.seats
                .report(DeviceKind::Gamepad, p.key, p.family, state);
        }
        self.pads_now = pads;
        // The wheel is a per-frame delta; everything else is held state.
        self.state.wheel = 0.0;
        *self.seats.finish()
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

    /// gilrs' names and ids, as the glue copies them, to a family.
    #[test]
    fn pad_idents_detect_their_family() {
        let id = |name: &str, usb: Option<(u16, u16)>| PadIdent {
            name: name.into(),
            vendor: usb.map(|u| u.0),
            product: usb.map(|u| u.1),
        };
        let cases = [
            (
                id("Xbox 360 Controller", Some((0x045e, 0x028e))),
                PadFamily::Xbox,
            ),
            (
                id("Xbox Wireless Controller", Some((0x045e, 0x0b13))),
                PadFamily::Xbox,
            ),
            (
                id("DualSense Wireless Controller", Some((0x054c, 0x0ce6))),
                PadFamily::PlayStation,
            ),
            (
                id("PS4 Controller", Some((0x054c, 0x05c4))),
                PadFamily::PlayStation,
            ),
            (
                id("Wireless Controller", Some((0x054c, 0x09cc))),
                PadFamily::PlayStation,
            ),
            (
                id("Nintendo Switch Pro Controller", Some((0x057e, 0x2009))),
                PadFamily::Nintendo,
            ),
            (
                id("8BitDo SN30 Pro+", Some((0x2dc8, 0x6002))),
                PadFamily::EightBitDo,
            ),
            // XInput mode: Microsoft's ids win over the 8BitDo name.
            (id("8BitDo Pro 2", Some((0x045e, 0x028e))), PadFamily::Xbox),
            // No ids (some backends): the name decides.
            (
                id("Sony Interactive Entertainment Wireless Controller", None),
                PadFamily::PlayStation,
            ),
            (
                id("PowerA Xbox Series X Controller", Some((0x24c6, 0x581a))),
                PadFamily::Xbox,
            ),
            (
                id("Logitech Dual Action", Some((0x046d, 0xc216))),
                PadFamily::Generic,
            ),
            (id("USB Gamepad", None), PadFamily::Generic),
        ];
        for (ident, want) in cases {
            assert_eq!(ident.family(), want, "{ident:?}");
        }
        let mut input = PcInput::new();
        assert_eq!(input.gamepad_family(), None);
        input.note_pad(Some(id("Pro Controller", Some((0x057e, 0x2009)))));
        assert_eq!(input.gamepad_family(), Some(PadFamily::Nintendo));
        assert_eq!(input.gamepad_name().as_deref(), Some("Pro Controller"));
        input.note_pad(None);
        assert_eq!(input.gamepad_family(), None);
    }

    #[cfg(feature = "gamepad")]
    #[test]
    fn gilrs_buttons_map_one_to_one() {
        for (i, p) in PadButton::ALL.iter().enumerate() {
            assert_eq!(*p as usize, i, "PadButton::ALL is in discriminant order");
            assert_eq!(PadButton::from_gilrs(p.to_gilrs()), Some(*p));
        }
        assert_eq!(PadButton::from_gilrs(gilrs::Button::Unknown), None);
        // The issue's mapping: LT2/RT2 → L2/R2, thumbs → L3/R3.
        let b = Bindings::default();
        for (button, pad) in [
            (Button::L2, gilrs::Button::LeftTrigger2),
            (Button::R2, gilrs::Button::RightTrigger2),
            (Button::L3, gilrs::Button::LeftThumb),
            (Button::R3, gilrs::Button::RightThumb),
        ] {
            let p = PadButton::from_gilrs(pad).unwrap();
            assert_eq!(b.pad[Bindings::slot(button)], [Some(p), None]);
        }
    }

    #[test]
    fn pad_buttons_are_distinct_bits() {
        for (i, p) in PadButton::ALL.iter().enumerate() {
            assert_eq!(*p as usize, i);
            let mut r = PadReading::default();
            r.press(*p);
            assert_eq!(r.held, 1 << i);
            assert!(PadButton::ALL.iter().all(|o| r.is_held(*o) == (o == p)));
        }
    }

    #[test]
    fn opposite_keys_cancel() {
        let mut input = PcInput::new();
        input.handle_key(KeyCode::KeyA, true);
        input.handle_key(KeyCode::KeyD, true);
        assert_eq!(input.poll().stick.x, 0.0);
    }
}
