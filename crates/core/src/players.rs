//! Local multiplayer (issue #67, ADR 0036): up to [`MAX_PLAYERS`] players
//! on one machine, each with their own [`InputState`].
//!
//! - [`PlayersInput`]: the frame's input per player and which players have
//!   a controller. Player 0 is the `input` every host already hands to
//!   [`Game::update_with`](crate::Game::update_with), so a single-player
//!   game never notices. Games read the rest through
//!   [`Platform::players`](crate::Platform::players).
//! - **Devices and seats.** A host reports each controller it sees as an
//!   [`InputDevice`] (the PC's keyboard and mouse, each gamepad, each N64
//!   controller port) and [`Seats`] folds the devices into players by
//!   their *seat* (the player index they play for):
//!   - an N64 port `k` is player `k` (a fixed seat; a game may still move
//!     it with [`Platform::assign_player`](crate::Platform::assign_player));
//!   - with [`PlayerSetup::max`] at 1 (the default) every keyboard and pad
//!     plays for player 0, as before this module existed;
//!   - with `max` above 1 and `auto_seat` on, the keyboard and the first
//!     pad are player 0 and every further pad takes the lowest seat no pad
//!     holds yet; with `auto_seat` off a new pad waits unseated (a join
//!     screen seats it: [`Join`]).
//! - [`HostedPlayers`]: what a backend's [`Platform`](crate::Platform)
//!   holds: the frame's players and devices as the host published them, and
//!   the game's requests (a seat, a new setup) for the host to hand back to
//!   its input's [`Seats`] after the frame.
//! - [`Join`]: the assignment screen helper — each device presses A (or
//!   Start) to take a seat and B to leave it; it draws the seats with each
//!   pad's family glyphs ([`crate::PadFamily`]).
//!
//! Per target: the N64 polls its four ports (`connected` is the port
//! mask); the 3DS has one player; the PC and the Web report the keyboard
//! and every pad. A recording or a network session replaces the devices
//! with recorded or remote input ([`crate::replay`]).

use crate::input::{Button, InputState};
use crate::math::{Color, Rect, Vec2};
use crate::pad::PadFamily;
use crate::render::{FontId, Renderer, TextAlign, TextParams};

/// The most local players a host reports (the N64's four ports).
pub const MAX_PLAYERS: usize = 4;

/// The most input devices [`Seats`] tracks (a keyboard and seven pads).
pub const MAX_DEVICES: usize = 8;

/// Every local player's input for one frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlayersInput {
    pub players: [InputState; MAX_PLAYERS],
    /// Bit `i` set: player `i` has a controller this frame (an N64 port
    /// with a pad in it, a seated keyboard or pad).
    pub connected: u8,
}

impl PlayersInput {
    /// Nobody: no player connected, every input idle.
    pub const NONE: PlayersInput = PlayersInput {
        players: [InputState::IDLE; MAX_PLAYERS],
        connected: 0,
    };

    /// One player (player 0), connected, holding `input`.
    pub const fn single(input: InputState) -> PlayersInput {
        PlayersInput {
            players: [input, InputState::IDLE, InputState::IDLE, InputState::IDLE],
            connected: 1,
        }
    }

    /// Player `i`'s input (idle past [`MAX_PLAYERS`]).
    pub fn get(&self, i: usize) -> &InputState {
        self.players.get(i).unwrap_or(&InputState::IDLE)
    }

    /// Does player `i` have a controller?
    pub fn is_connected(&self, i: usize) -> bool {
        i < MAX_PLAYERS && self.connected & (1 << i) != 0
    }

    /// How many players have a controller.
    pub fn count(&self) -> u32 {
        (self.connected & ((1 << MAX_PLAYERS) - 1)).count_ones()
    }
}

impl Default for PlayersInput {
    fn default() -> Self {
        Self::NONE
    }
}

/// A device's handle, stable while the host runs (a pad that comes back
/// keeps its id and its seat).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct DeviceId(pub u8);

/// What kind of controller a device is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DeviceKind {
    /// The PC's or the Web's keyboard and mouse.
    Keyboard,
    /// A PC or Web gamepad.
    Gamepad,
    /// A console's controller port (the N64's four, the 3DS's one).
    Port,
}

/// One controller as the host sees it this frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct InputDevice {
    pub id: DeviceId,
    pub kind: DeviceKind,
    /// The host's own index: the port number, the pad's slot.
    pub slot: u8,
    /// A pad's family (`None` for keyboards and console ports).
    pub family: Option<PadFamily>,
    /// The player it plays for; `None`: it plays for nobody (an unseated
    /// pad on a join screen).
    pub seat: Option<u8>,
    /// Plugged in this frame (a device that left keeps its seat for when it
    /// comes back).
    pub connected: bool,
    /// Its own input this frame, whatever its seat.
    pub input: InputState,
}

impl InputDevice {
    const EMPTY: InputDevice = InputDevice {
        id: DeviceId(0),
        kind: DeviceKind::Keyboard,
        slot: 0,
        family: None,
        seat: None,
        connected: false,
        input: InputState::IDLE,
    };

    /// A short uppercase name for a seat panel: `KEYBOARD`, `XBOX PAD`,
    /// `PORT 2`.
    pub fn label(&self) -> &'static str {
        match self.kind {
            DeviceKind::Keyboard => "KEYBOARD",
            DeviceKind::Port => match self.slot {
                0 => "PORT 1",
                1 => "PORT 2",
                2 => "PORT 3",
                _ => "PORT 4",
            },
            DeviceKind::Gamepad => match self.family {
                Some(PadFamily::Xbox) => "XBOX PAD",
                Some(PadFamily::PlayStation) => "PLAYSTATION PAD",
                Some(PadFamily::Nintendo) => "NINTENDO PAD",
                Some(PadFamily::EightBitDo) => "8BITDO PAD",
                _ => "GAMEPAD",
            },
        }
    }
}

/// How a host seats its devices.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayerSetup {
    /// The most players the game takes (1..=[`MAX_PLAYERS`]). At 1 every
    /// keyboard and pad plays for player 0 (console ports keep their own).
    pub max: u8,
    /// Seat a new device as it appears (the keyboard and the first pad on
    /// player 0, each further pad on the lowest seat without a pad). Off: a
    /// new pad waits unseated until the game seats it ([`Join`]).
    pub auto_seat: bool,
}

impl PlayerSetup {
    /// One player, every device on it: what every host did before #67.
    pub const SINGLE: PlayerSetup = PlayerSetup {
        max: 1,
        auto_seat: true,
    };

    /// Up to `max` players, seated as devices appear.
    pub const fn up_to(max: u8) -> PlayerSetup {
        PlayerSetup {
            max: if max == 0 {
                1
            } else if max as usize > MAX_PLAYERS {
                MAX_PLAYERS as u8
            } else {
                max
            },
            auto_seat: true,
        }
    }
}

impl Default for PlayerSetup {
    fn default() -> Self {
        Self::SINGLE
    }
}

/// The input backend's side: devices by the host's key, their seats, and
/// the fold into [`PlayersInput`]. Each frame: [`begin`](Self::begin), one
/// [`report`](Self::report) per device the host sees, then
/// [`finish`](Self::finish).
#[derive(Clone, Debug)]
pub struct Seats {
    setup: PlayerSetup,
    devices: [InputDevice; MAX_DEVICES],
    /// The host's key per device (a pad's own id, the port number).
    keys: [u32; MAX_DEVICES],
    len: usize,
    players: PlayersInput,
}

impl Default for Seats {
    fn default() -> Self {
        Self::new()
    }
}

impl Seats {
    pub const fn new() -> Seats {
        Seats {
            setup: PlayerSetup::SINGLE,
            devices: [InputDevice::EMPTY; MAX_DEVICES],
            keys: [0; MAX_DEVICES],
            len: 0,
            players: PlayersInput::NONE,
        }
    }

    pub fn setup(&self) -> PlayerSetup {
        self.setup
    }

    /// A new setup. With `auto_seat`, the keyboard and the pads are seated
    /// again in the order they first appeared; without it, seats past
    /// `max` are cleared and the rest stay.
    pub fn set_setup(&mut self, setup: PlayerSetup) {
        let setup = PlayerSetup {
            max: PlayerSetup::up_to(setup.max).max,
            ..setup
        };
        self.setup = setup;
        if setup.auto_seat {
            for i in 0..self.len {
                if self.devices[i].kind != DeviceKind::Port {
                    self.devices[i].seat = None;
                }
            }
            for i in 0..self.len {
                if self.devices[i].kind != DeviceKind::Port {
                    self.devices[i].seat = self.auto_seat_for(i);
                }
            }
        } else {
            for d in self.devices[..self.len].iter_mut() {
                if d.kind != DeviceKind::Port && d.seat.is_some_and(|s| s >= setup.max) {
                    d.seat = None;
                }
            }
        }
    }

    /// Start a frame: every device reads unplugged until reported.
    pub fn begin(&mut self) {
        for d in self.devices[..self.len].iter_mut() {
            d.connected = false;
            d.input = InputState::IDLE;
        }
    }

    /// The host sees device `key` of `kind` holding `input`. Returns its id.
    /// A device seen for the first time is seated by the setup; past
    /// [`MAX_DEVICES`] the oldest unplugged one makes room (or, with every
    /// slot plugged, the device is ignored and `None` comes back).
    pub fn report(
        &mut self,
        kind: DeviceKind,
        key: u32,
        family: Option<PadFamily>,
        input: InputState,
    ) -> Option<DeviceId> {
        let i = match (0..self.len).find(|&i| self.devices[i].kind == kind && self.keys[i] == key) {
            Some(i) => i,
            None => {
                let i = if self.len < MAX_DEVICES {
                    self.len += 1;
                    self.len - 1
                } else {
                    (0..self.len).find(|&i| !self.devices[i].connected)?
                };
                self.devices[i] = InputDevice {
                    id: DeviceId(i as u8),
                    kind,
                    slot: key.min(255) as u8,
                    family,
                    seat: None,
                    connected: false,
                    input: InputState::IDLE,
                };
                self.keys[i] = key;
                self.devices[i].seat = match kind {
                    DeviceKind::Port => (key < MAX_PLAYERS as u32).then_some(key as u8),
                    _ if self.setup.auto_seat => self.auto_seat_for(i),
                    _ => None,
                };
                i
            }
        };
        let d = &mut self.devices[i];
        d.connected = true;
        d.family = family;
        d.input = input;
        Some(d.id)
    }

    /// The seat a newly seen keyboard or pad takes with `auto_seat`.
    #[inline(never)]
    fn auto_seat_for(&self, i: usize) -> Option<u8> {
        let max = self.setup.max;
        if max <= 1 {
            return Some(0);
        }
        match self.devices[i].kind {
            DeviceKind::Keyboard => Some(0),
            DeviceKind::Port => None,
            DeviceKind::Gamepad => (0..max).find(|&s| {
                !self.devices[..self.len]
                    .iter()
                    .enumerate()
                    .any(|(j, d)| j != i && d.kind == DeviceKind::Gamepad && d.seat == Some(s))
            }),
        }
    }

    /// Seat `device` on `seat` (`None`: it plays for nobody). A seat at or
    /// past the setup's `max` (or [`MAX_PLAYERS`] for a console port)
    /// unseats it. Unknown devices are ignored.
    pub fn assign(&mut self, device: DeviceId, seat: Option<u8>) {
        let Some(d) = self.devices[..self.len].get_mut(device.0 as usize) else {
            return;
        };
        let limit = match d.kind {
            DeviceKind::Port => MAX_PLAYERS as u8,
            _ => self.setup.max,
        };
        d.seat = seat.filter(|s| *s < limit);
    }

    /// Unseat every keyboard and pad (a join screen starts empty). Console
    /// ports keep their seats.
    pub fn clear(&mut self) {
        for d in self.devices[..self.len].iter_mut() {
            if d.kind != DeviceKind::Port {
                d.seat = None;
            }
        }
    }

    /// The seat a device's input goes to this frame: its own, or player 0
    /// for any keyboard or pad when the setup takes one player.
    fn effective_seat(&self, d: &InputDevice) -> Option<u8> {
        match d.kind {
            DeviceKind::Port => d.seat,
            _ if self.setup.max <= 1 => d.seat.map(|_| 0),
            _ => d.seat,
        }
    }

    /// End the frame: fold every plugged device into its player, in the
    /// order the devices first appeared (the keyboard before the pads).
    pub fn finish(&mut self) -> &PlayersInput {
        let mut out = PlayersInput::NONE;
        for d in &self.devices[..self.len] {
            if !d.connected {
                continue;
            }
            if let Some(s) = self.effective_seat(d) {
                let s = s as usize;
                if s < MAX_PLAYERS {
                    if out.connected & (1 << s) == 0 {
                        out.players[s] = d.input;
                        out.connected |= 1 << s;
                    } else {
                        out.players[s].merge(&d.input);
                    }
                }
            }
        }
        self.players = out;
        &self.players
    }

    /// The last [`finish`](Self::finish)'s players.
    pub fn players(&self) -> &PlayersInput {
        &self.players
    }

    /// Every device seen since the host started (unplugged ones included,
    /// `connected` false), in the order they first appeared.
    pub fn devices(&self) -> &[InputDevice] {
        &self.devices[..self.len]
    }

    /// The seat of `device` (`None`: unseated or unknown).
    pub fn seat_of(&self, device: DeviceId) -> Option<u8> {
        self.devices[..self.len]
            .get(device.0 as usize)
            .and_then(|d| d.seat)
    }

    /// Apply a game's request ([`HostedPlayers::take_request`]).
    pub fn apply(&mut self, req: PlayersRequest) {
        match req {
            PlayersRequest::Setup(s) => self.set_setup(s),
            PlayersRequest::Assign(d, s) => self.assign(d, s),
            PlayersRequest::Clear => self.clear(),
        }
    }
}

/// What a game asks of the host's seats.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayersRequest {
    Setup(PlayerSetup),
    Assign(DeviceId, Option<u8>),
    Clear,
}

/// The most requests a frame keeps (later ones replace the last).
const MAX_REQUESTS: usize = 8;

/// The players as a backend's platform holds them: the host publishes the
/// frame's players and devices before the game's update, and takes the
/// game's requests after it ([`Seats::apply`]). A request also applies to
/// this copy at once, so a game reads its own change in the same frame.
#[derive(Clone, Debug)]
pub struct HostedPlayers {
    players: PlayersInput,
    devices: [InputDevice; MAX_DEVICES],
    len: usize,
    setup: PlayerSetup,
    requests: [Option<PlayersRequest>; MAX_REQUESTS],
}

impl Default for HostedPlayers {
    fn default() -> Self {
        Self::new()
    }
}

impl HostedPlayers {
    pub const fn new() -> HostedPlayers {
        HostedPlayers {
            players: PlayersInput::NONE,
            devices: [InputDevice::EMPTY; MAX_DEVICES],
            len: 0,
            setup: PlayerSetup::SINGLE,
            requests: [None; MAX_REQUESTS],
        }
    }

    /// The host's frame: what its [`Seats`] folded.
    pub fn publish(&mut self, seats: &Seats) {
        self.players = *seats.players();
        let devices = seats.devices();
        self.len = devices.len();
        self.devices[..self.len].copy_from_slice(devices);
        self.setup = seats.setup();
    }

    /// The host's frame from players it did not poll (a replay, a network
    /// session, a script): no devices.
    pub fn publish_players(&mut self, players: &PlayersInput) {
        self.players = *players;
        self.len = 0;
    }

    pub fn players(&self) -> &PlayersInput {
        &self.players
    }

    pub fn devices(&self) -> &[InputDevice] {
        &self.devices[..self.len]
    }

    pub fn setup(&self) -> PlayerSetup {
        self.setup
    }

    /// Queue a request (and apply it to this copy).
    pub fn request(&mut self, req: PlayersRequest) {
        match req {
            PlayersRequest::Setup(s) => {
                self.setup = PlayerSetup {
                    max: PlayerSetup::up_to(s.max).max,
                    ..s
                }
            }
            PlayersRequest::Assign(id, seat) => {
                let max = self.setup.max;
                if let Some(d) = self.devices[..self.len].get_mut(id.0 as usize) {
                    let limit = if d.kind == DeviceKind::Port {
                        MAX_PLAYERS as u8
                    } else {
                        max
                    };
                    d.seat = seat.filter(|s| *s < limit);
                }
            }
            PlayersRequest::Clear => {
                for d in self.devices[..self.len].iter_mut() {
                    if d.kind != DeviceKind::Port {
                        d.seat = None;
                    }
                }
            }
        }
        match self.requests.iter_mut().find(|r| r.is_none()) {
            Some(slot) => *slot = Some(req),
            None => self.requests[MAX_REQUESTS - 1] = Some(req),
        }
    }

    /// The oldest request the host has not applied yet.
    pub fn take_request(&mut self) -> Option<PlayersRequest> {
        let first = self.requests[0].take()?;
        // A plain shift: `rotate_left` costs the N64 a kilobyte of code.
        for i in 1..MAX_REQUESTS {
            self.requests[i - 1] = self.requests[i].take();
        }
        Some(first)
    }
}

/// What a [`Join`] frame saw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JoinEvent {
    /// Nothing new.
    None,
    /// A device took a seat.
    Joined { device: DeviceId, seat: u8 },
    /// A device left its seat.
    Left { device: DeviceId, seat: u8 },
    /// A seated player pressed Start: the game may begin.
    Start,
}

/// The assignment screen: every keyboard and pad starts unseated; a device
/// pressing A or Start takes the lowest free seat, B leaves it, and a
/// seated player's Start begins. Console ports stay where they are (an N64
/// port is its player), so on the N64 the screen only shows who is
/// plugged in and waits for Start.
#[derive(Clone, Debug)]
pub struct Join {
    max: u8,
    /// Each device's buttons last frame, by device index.
    prev: [u32; MAX_DEVICES],
}

impl Join {
    /// Open the screen for up to `max` players: the host stops seating new
    /// pads by itself and every keyboard and pad is unseated.
    pub fn begin(platform: &mut dyn crate::Platform, max: u8) -> Join {
        let max = PlayerSetup::up_to(max).max;
        platform.set_player_setup(PlayerSetup {
            max,
            auto_seat: false,
        });
        platform.clear_players();
        let mut prev = [0u32; MAX_DEVICES];
        for d in platform.input_devices() {
            if let Some(p) = prev.get_mut(d.id.0 as usize) {
                // Whatever is held now (the press that opened the screen)
                // joins nobody until it is released.
                *p = d.input.button_bits();
            }
        }
        Join { max, prev }
    }

    /// The most players this screen seats.
    pub fn max(&self) -> u8 {
        self.max
    }

    /// One frame: at most one event (the first device in order that did
    /// something).
    pub fn update(&mut self, platform: &mut dyn crate::Platform) -> JoinEvent {
        let mut taken = [false; MAX_PLAYERS];
        let mut presses: [(DeviceId, DeviceKind, Option<u8>, u32); MAX_DEVICES] =
            [(DeviceId(0), DeviceKind::Keyboard, None, 0); MAX_DEVICES];
        let mut n = 0;
        for d in platform.input_devices() {
            if let Some(s) = d.seat
                && (s as usize) < MAX_PLAYERS
            {
                taken[s as usize] = true;
            }
            let i = d.id.0 as usize;
            if i >= MAX_DEVICES {
                continue;
            }
            let now = if d.connected {
                d.input.button_bits()
            } else {
                0
            };
            let pressed = now & !self.prev[i];
            self.prev[i] = now;
            if pressed != 0 && n < MAX_DEVICES {
                presses[n] = (d.id, d.kind, d.seat.filter(|_| d.connected), pressed);
                n += 1;
            }
        }
        let bit = |b: Button| 1u32 << b as u32;
        for &(id, kind, seat, pressed) in &presses[..n] {
            match seat {
                None if pressed & (bit(Button::A) | bit(Button::Start)) != 0 => {
                    if let Some(s) = (0..self.max).find(|&s| !taken[s as usize]) {
                        platform.assign_player(id, Some(s));
                        return JoinEvent::Joined {
                            device: id,
                            seat: s,
                        };
                    }
                }
                Some(_) if pressed & bit(Button::Start) != 0 => return JoinEvent::Start,
                Some(s) if kind != DeviceKind::Port && pressed & bit(Button::B) != 0 => {
                    platform.assign_player(id, None);
                    return JoinEvent::Left {
                        device: id,
                        seat: s,
                    };
                }
                _ => {}
            }
        }
        JoinEvent::None
    }

    /// Who sits where: per seat, the first plugged device on it.
    pub fn seated(&self, platform: &dyn crate::Platform) -> [Option<InputDevice>; MAX_PLAYERS] {
        let mut out = [None; MAX_PLAYERS];
        for d in platform.input_devices() {
            if !d.connected {
                continue;
            }
            if let Some(s) = d.seat
                && (s as usize) < MAX_PLAYERS
                && s < self.max
                && out[s as usize].is_none()
            {
                out[s as usize] = Some(*d);
            }
        }
        out
    }

    /// Draw the seats side by side inside `area`: `P1`..`P4`, the device on
    /// each (with "PRESS <A>" in its family's glyph for an empty one) and,
    /// once someone sits, "START TO PLAY". 2D only (`fill_rect` and
    /// `draw_text`), so every backend draws it.
    pub fn draw(
        &self,
        platform: &dyn crate::Platform,
        renderer: &mut dyn Renderer,
        font: FontId,
        area: Rect,
    ) {
        let seated = self.seated(platform);
        let family = platform
            .input_devices()
            .iter()
            .find(|d| d.connected && d.seat.is_none() && d.kind == crate::DeviceKind::Gamepad)
            .and_then(|d| d.family);
        let max = self.max.max(1) as f32;
        let gap = 4.0;
        let w = ((area.size.x - gap * (max - 1.0)) / max) as i32 as f32;
        let h = area.size.y;
        let text = |scale: f32, tint: Color| TextParams {
            scale,
            tint,
            align: TextAlign::Center,
        };
        const NAMES: [&str; MAX_PLAYERS] = ["P1", "P2", "P3", "P4"];
        const SEAT: [Color; MAX_PLAYERS] = [
            Color::rgb(220, 70, 60),
            Color::rgb(60, 120, 230),
            Color::rgb(60, 180, 90),
            Color::rgb(230, 190, 50),
        ];
        for s in 0..self.max as usize {
            let x = area.pos.x + s as f32 * (w + gap);
            let panel = Rect::new(x, area.pos.y, w, h);
            let on = seated[s];
            renderer.fill_rect(
                panel,
                if on.is_some() {
                    Color::rgba(30, 30, 40, 230)
                } else {
                    Color::rgba(20, 20, 24, 160)
                },
            );
            renderer.fill_rect(Rect::new(x, area.pos.y, w, 3.0), SEAT[s]);
            let cx = x + w * 0.5;
            renderer.draw_text(
                font,
                Vec2::new(cx, area.pos.y + 8.0),
                NAMES[s],
                &text(2.0, SEAT[s]),
            );
            match on {
                Some(d) => renderer.draw_text(
                    font,
                    Vec2::new(cx, area.pos.y + h * 0.5),
                    d.label(),
                    &text(1.0, Color::WHITE),
                ),
                None => {
                    let a = family.unwrap_or(PadFamily::Generic).button_glyph(Button::A);
                    let label = if a.short.is_empty() { a.name } else { a.short };
                    renderer.draw_text(
                        font,
                        Vec2::new(cx, area.pos.y + h * 0.5 - 6.0),
                        "PRESS",
                        &text(1.0, Color::rgb(170, 170, 180)),
                    );
                    renderer.draw_text(
                        font,
                        Vec2::new(cx, area.pos.y + h * 0.5 + 4.0),
                        label,
                        &text(1.0, a.color),
                    );
                }
            }
        }
        if seated.iter().any(Option::is_some) {
            renderer.draw_text(
                font,
                Vec2::new(area.pos.x + area.size.x * 0.5, area.pos.y + h - 12.0),
                "START TO PLAY",
                &text(1.0, Color::WHITE),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vec2;

    fn press(b: Button) -> InputState {
        let mut s = InputState::IDLE;
        s.set(b, true);
        s
    }

    fn stick(x: f32) -> InputState {
        let mut s = InputState::IDLE;
        s.stick = Vec2::new(x, 0.0);
        s
    }

    #[test]
    fn single_is_player_zero_connected() {
        let p = PlayersInput::single(press(Button::A));
        assert!(p.is_connected(0) && !p.is_connected(1));
        assert_eq!(p.count(), 1);
        assert!(p.get(0).is_down(Button::A));
        assert_eq!(*p.get(9), InputState::IDLE);
        assert_eq!(PlayersInput::default().count(), 0);
    }

    #[test]
    fn one_player_setup_folds_every_device_into_player_zero() {
        let mut seats = Seats::new();
        seats.begin();
        seats.report(DeviceKind::Keyboard, 0, None, press(Button::A));
        seats.report(
            DeviceKind::Gamepad,
            7,
            Some(PadFamily::Xbox),
            press(Button::B),
        );
        seats.report(DeviceKind::Gamepad, 9, None, stick(0.5));
        let p = *seats.finish();
        assert_eq!(p.connected, 1);
        let p0 = p.get(0);
        assert!(p0.is_down(Button::A) && p0.is_down(Button::B));
        assert_eq!(p0.stick, Vec2::new(0.5, 0.0));
    }

    #[test]
    fn more_players_seat_the_keyboard_and_first_pad_together_then_one_pad_each() {
        let mut seats = Seats::new();
        seats.set_setup(PlayerSetup::up_to(4));
        seats.begin();
        let kb = seats
            .report(DeviceKind::Keyboard, 0, None, press(Button::A))
            .unwrap();
        let p1 = seats
            .report(DeviceKind::Gamepad, 7, None, press(Button::X))
            .unwrap();
        let p2 = seats
            .report(DeviceKind::Gamepad, 9, None, press(Button::B))
            .unwrap();
        let p = *seats.finish();
        assert_eq!(seats.seat_of(kb), Some(0));
        assert_eq!(seats.seat_of(p1), Some(0));
        assert_eq!(seats.seat_of(p2), Some(1));
        assert_eq!(p.connected, 0b11);
        assert!(p.get(0).is_down(Button::A) && p.get(0).is_down(Button::X));
        assert!(p.get(1).is_down(Button::B) && !p.get(1).is_down(Button::A));
        // The first pad leaves and comes back: same id, same seat; a new pad
        // meanwhile takes the free seat 0 is not, because pad 1 still holds
        // it (it left, but keeps the seat).
        seats.begin();
        seats.report(DeviceKind::Keyboard, 0, None, InputState::IDLE);
        let p3 = seats
            .report(DeviceKind::Gamepad, 11, None, InputState::IDLE)
            .unwrap();
        seats.finish();
        assert_eq!(seats.seat_of(p3), Some(2));
        seats.begin();
        assert_eq!(
            seats.report(DeviceKind::Gamepad, 7, None, InputState::IDLE),
            Some(p1)
        );
        seats.finish();
        assert_eq!(seats.seat_of(p1), Some(0));
    }

    #[test]
    fn console_ports_are_their_players_whatever_the_setup() {
        let mut seats = Seats::new();
        seats.begin();
        seats.report(DeviceKind::Port, 0, None, press(Button::A));
        seats.report(DeviceKind::Port, 2, None, press(Button::B));
        let p = *seats.finish();
        assert_eq!(p.connected, 0b101);
        assert!(p.get(0).is_down(Button::A));
        assert!(p.get(2).is_down(Button::B));
        assert_eq!(*p.get(1), InputState::IDLE);
        // A port with no pad is not reported: it reads disconnected.
        seats.begin();
        seats.report(DeviceKind::Port, 2, None, InputState::IDLE);
        assert_eq!(seats.finish().connected, 0b100);
        // Clearing seats keeps the ports.
        seats.clear();
        seats.begin();
        seats.report(DeviceKind::Port, 0, None, InputState::IDLE);
        assert_eq!(seats.finish().connected, 0b001);
    }

    #[test]
    fn assignment_moves_a_device_and_respects_the_maximum() {
        let mut seats = Seats::new();
        seats.set_setup(PlayerSetup::up_to(2));
        seats.begin();
        let kb = seats
            .report(DeviceKind::Keyboard, 0, None, press(Button::A))
            .unwrap();
        seats.finish();
        seats.assign(kb, Some(1));
        seats.begin();
        seats.report(DeviceKind::Keyboard, 0, None, press(Button::A));
        let p = *seats.finish();
        assert_eq!(p.connected, 0b10);
        assert!(p.get(1).is_down(Button::A));
        seats.assign(kb, Some(3)); // past max: unseated
        assert_eq!(seats.seat_of(kb), None);
        seats.begin();
        seats.report(DeviceKind::Keyboard, 0, None, press(Button::A));
        assert_eq!(seats.finish().connected, 0);
    }

    #[test]
    fn a_full_table_reuses_the_oldest_unplugged_device() {
        let mut seats = Seats::new();
        seats.begin();
        for k in 0..MAX_DEVICES as u32 {
            assert!(
                seats
                    .report(DeviceKind::Gamepad, k, None, InputState::IDLE)
                    .is_some()
            );
        }
        assert_eq!(
            seats.report(DeviceKind::Gamepad, 99, None, InputState::IDLE),
            None
        );
        seats.finish();
        seats.begin();
        seats.report(DeviceKind::Gamepad, 0, None, InputState::IDLE);
        assert_eq!(
            seats.report(DeviceKind::Gamepad, 99, None, InputState::IDLE),
            Some(DeviceId(1))
        );
    }

    /// A platform holding [`HostedPlayers`] over a [`Seats`], as the hosts
    /// do.
    struct Host {
        seats: Seats,
        hosted: HostedPlayers,
    }

    impl crate::Platform for Host {
        fn now_micros(&self) -> u64 {
            0
        }
        fn log(&self, _: &str) {}
        fn hosted_players(&self) -> Option<&HostedPlayers> {
            Some(&self.hosted)
        }
        fn hosted_players_mut(&mut self) -> Option<&mut HostedPlayers> {
            Some(&mut self.hosted)
        }
    }

    impl Host {
        fn frame(&mut self, devices: &[(DeviceKind, u32, InputState)]) {
            while let Some(r) = self.hosted.take_request() {
                self.seats.apply(r);
            }
            self.seats.begin();
            for (k, key, s) in devices {
                self.seats.report(*k, *key, None, *s);
            }
            self.seats.finish();
            self.hosted.publish(&self.seats);
        }
    }

    #[test]
    fn the_join_screen_seats_devices_by_their_presses() {
        use crate::Platform;
        let mut host = Host {
            seats: Seats::new(),
            hosted: HostedPlayers::new(),
        };
        let idle = InputState::IDLE;
        host.frame(&[
            (DeviceKind::Keyboard, 0, press(Button::A)),
            (DeviceKind::Gamepad, 5, idle),
        ]);
        assert_eq!(host.players().connected, 1, "auto: everyone on player 0");
        let mut join = Join::begin(&mut host, 3);
        assert_eq!(host.player_setup().max, 3);
        host.frame(&[
            (DeviceKind::Keyboard, 0, press(Button::A)),
            (DeviceKind::Gamepad, 5, idle),
        ]);
        assert_eq!(host.players().connected, 0, "the screen starts empty");
        // The keyboard's A was held when the screen opened: no join yet.
        assert_eq!(join.update(&mut host), JoinEvent::None);
        host.frame(&[
            (DeviceKind::Keyboard, 0, idle),
            (DeviceKind::Gamepad, 5, press(Button::A)),
        ]);
        let pad = host.input_devices()[1].id;
        assert_eq!(
            join.update(&mut host),
            JoinEvent::Joined {
                device: pad,
                seat: 0
            }
        );
        host.frame(&[
            (DeviceKind::Keyboard, 0, press(Button::Start)),
            (DeviceKind::Gamepad, 5, idle),
        ]);
        assert_eq!(
            join.update(&mut host),
            JoinEvent::Joined {
                device: DeviceId(0),
                seat: 1
            }
        );
        host.frame(&[
            (DeviceKind::Keyboard, 0, press(Button::Start)),
            (DeviceKind::Gamepad, 5, press(Button::B)),
        ]);
        assert_eq!(host.players().connected, 0b11);
        assert_eq!(
            join.update(&mut host),
            JoinEvent::Left {
                device: pad,
                seat: 0
            }
        );
        host.frame(&[
            (DeviceKind::Keyboard, 0, idle),
            (DeviceKind::Gamepad, 5, idle),
        ]);
        assert_eq!(host.players().connected, 0b10);
        assert_eq!(join.update(&mut host), JoinEvent::None);
        host.frame(&[
            (DeviceKind::Keyboard, 0, press(Button::Start)),
            (DeviceKind::Gamepad, 5, idle),
        ]);
        assert_eq!(join.update(&mut host), JoinEvent::Start);
        let seated = join.seated(&host);
        assert!(seated[0].is_none() && seated[1].is_some());
    }

    #[test]
    fn requests_apply_to_the_copy_at_once_and_queue_for_the_host() {
        let mut h = HostedPlayers::new();
        let mut seats = Seats::new();
        seats.begin();
        seats.report(DeviceKind::Keyboard, 0, None, InputState::IDLE);
        seats.finish();
        h.publish(&seats);
        h.request(PlayersRequest::Setup(PlayerSetup::up_to(9)));
        assert_eq!(h.setup().max, 4);
        h.request(PlayersRequest::Assign(DeviceId(0), Some(2)));
        assert_eq!(h.devices()[0].seat, Some(2));
        assert_eq!(
            h.take_request(),
            Some(PlayersRequest::Setup(PlayerSetup::up_to(4)))
        );
        assert_eq!(
            h.take_request(),
            Some(PlayersRequest::Assign(DeviceId(0), Some(2)))
        );
        assert_eq!(h.take_request(), None);
    }
}
