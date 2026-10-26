//! Local players through the public API (issue #67): a platform that hosts
//! no players reports nobody; a host's `Seats` publish through
//! `HostedPlayers`, the game's requests come back to the seats; the `Join`
//! screen seats devices by their presses; the `NetPad` round trip.

use trino_core::{
    Audio, Button, DeviceKind, Game, HostedPlayers, InputState, Join, JoinEvent, MusicId, NetPad,
    Platform, PlayerSetup, PlayersInput, Renderer, Seats, SoundId, Vec2,
};

struct Bare;
impl Platform for Bare {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
}

/// A host: devices into seats, published to the platform.
struct Host {
    seats: Seats,
    hosted: HostedPlayers,
}

impl Platform for Host {
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

fn press(b: Button) -> InputState {
    let mut s = InputState::IDLE;
    s.set(b, true);
    s
}

/// A game that moves one dot per connected player.
struct Dots {
    pos: [Vec2; 4],
}

impl Game for Dots {
    fn update(&mut self, _: &InputState, _: &mut dyn Audio, _: f32) {}
    fn update_with(
        &mut self,
        p: &mut dyn Platform,
        input: &InputState,
        _: &mut dyn Audio,
        dt: f32,
    ) {
        let players = *p.players();
        assert_eq!(players.players[0], *input, "player 1 is the update's input");
        for i in 0..4 {
            if players.is_connected(i) {
                self.pos[i] += players.players[i].stick * dt;
            }
        }
    }
    fn render(&mut self, _: &mut dyn Renderer) {}
}

struct Mute;
impl Audio for Mute {
    fn play_sound(&mut self, _: SoundId) {}
    fn play_music(&mut self, _: MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

#[test]
fn a_platform_without_players_reports_nobody_and_ignores_requests() {
    let mut p = Bare;
    assert_eq!(*p.players(), PlayersInput::NONE);
    assert!(p.input_devices().is_empty());
    assert_eq!(p.player_setup(), PlayerSetup::SINGLE);
    p.set_player_setup(PlayerSetup::up_to(4));
    p.assign_player(trino_core::DeviceId(0), Some(1));
    p.clear_players();
    p.rebind_player(1, Button::A, trino_core::BindDevice::Gamepad);
    assert_eq!(
        p.binding_label_for(1, Button::A, trino_core::BindDevice::Gamepad),
        None
    );
    assert_eq!(p.player_setup(), PlayerSetup::SINGLE);
}

#[test]
fn hosts_publish_seats_and_games_move_every_player() {
    let mut host = Host {
        seats: Seats::new(),
        hosted: HostedPlayers::new(),
    };
    let mut stick = InputState::IDLE;
    stick.stick = Vec2::new(1.0, 0.0);
    let devices = [
        (DeviceKind::Keyboard, 0, InputState::IDLE),
        (DeviceKind::Gamepad, 3, stick),
        (DeviceKind::Gamepad, 4, stick),
    ];
    host.frame(&devices);
    assert_eq!(
        host.players().connected,
        1,
        "one player: every device is player 1"
    );
    // The game asks for two players: the host applies it after the frame.
    host.set_player_setup(PlayerSetup::up_to(2));
    assert_eq!(host.player_setup().max, 2, "the copy shows it at once");
    host.frame(&devices);
    assert_eq!(host.players().connected, 0b11);
    let mut g = Dots {
        pos: [Vec2::ZERO; 4],
    };
    let p0 = host.players().players[0];
    g.update_with(&mut host, &p0, &mut Mute, 1.0);
    assert_eq!(g.pos[0], Vec2::new(1.0, 0.0));
    assert_eq!(g.pos[1], Vec2::new(1.0, 0.0));
    assert_eq!(g.pos[2], Vec2::ZERO, "a third player past the setup's max");
    // Move the second pad to nobody.
    let pad2 = host.input_devices()[2].id;
    host.assign_player(pad2, None);
    host.frame(&devices);
    assert_eq!(host.players().connected, 0b01);
}

#[test]
fn the_join_screen_takes_seats_by_presses() {
    let mut host = Host {
        seats: Seats::new(),
        hosted: HostedPlayers::new(),
    };
    let idle = InputState::IDLE;
    host.frame(&[
        (DeviceKind::Keyboard, 0, idle),
        (DeviceKind::Gamepad, 1, idle),
    ]);
    let mut join = Join::begin(&mut host, 2);
    host.frame(&[
        (DeviceKind::Keyboard, 0, idle),
        (DeviceKind::Gamepad, 1, idle),
    ]);
    assert_eq!(join.update(&mut host), JoinEvent::None);
    host.frame(&[
        (DeviceKind::Keyboard, 0, idle),
        (DeviceKind::Gamepad, 1, press(Button::A)),
    ]);
    assert!(matches!(
        join.update(&mut host),
        JoinEvent::Joined { seat: 0, .. }
    ));
    host.frame(&[
        (DeviceKind::Keyboard, 0, press(Button::A)),
        (DeviceKind::Gamepad, 1, idle),
    ]);
    assert!(matches!(
        join.update(&mut host),
        JoinEvent::Joined { seat: 1, .. }
    ));
    host.frame(&[
        (DeviceKind::Keyboard, 0, idle),
        (DeviceKind::Gamepad, 1, press(Button::Start)),
    ]);
    assert_eq!(join.update(&mut host), JoinEvent::Start);
    assert_eq!(host.players().connected, 0b11);
    let seated = join.seated(&host);
    assert_eq!(seated[0].map(|d| d.kind), Some(DeviceKind::Gamepad));
    assert_eq!(seated[1].map(|d| d.kind), Some(DeviceKind::Keyboard));
}

#[test]
fn a_net_pad_is_what_every_peer_sees() {
    let mut s = press(Button::B);
    s.stick = Vec2::new(0.3, -0.7);
    s.triggers = Vec2::new(1.0, 0.2);
    let q = NetPad::quantize(&s);
    assert_eq!(NetPad::quantize(&q), q);
    assert!(q.is_down(Button::B));
    assert!((q.stick.y + 0.7).abs() < 0.01);
    assert_eq!(
        NetPad::from_bytes(NetPad::from_input(&s).to_bytes()),
        NetPad::from_input(&s)
    );
}
