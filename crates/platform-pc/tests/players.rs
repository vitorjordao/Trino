//! The PC's local players (issue #67): the keyboard and pads as seats
//! through `PcInput::poll_players` (fed pads, no device), one player
//! merging every device exactly as before, player 2's own bindings, and
//! the bindings file with every player's lines (old files still read).

use trino_core::{BindDevice, Button, Input, Platform, PlayerSetup, Vec2};
use trino_platform_pc::input::{
    Bindings, FedPad, InputRequest, PadButton, PadReading, players_bindings_from_text,
    players_bindings_to_text,
};
use trino_platform_pc::{PcInput, PcPlatform};
use winit::keyboard::KeyCode;

fn pad(key: u32, b: PadButton, stick: Vec2) -> FedPad {
    let mut reading = PadReading::default();
    reading.press(b);
    reading.left_stick = stick;
    FedPad {
        key,
        reading,
        family: None,
    }
}

#[test]
fn one_player_merges_every_device_as_before() {
    let mut input = PcInput::new();
    input.handle_key(KeyCode::KeyZ, true);
    input.feed_pads(&[
        pad(1, PadButton::East, Vec2::new(0.5, 0.0)),
        pad(2, PadButton::North, Vec2::new(0.0, -1.0)),
    ]);
    let p = input.poll_players();
    assert_eq!(p.connected, 1);
    let s = p.players[0];
    assert!(s.is_down(Button::A) && s.is_down(Button::B) && s.is_down(Button::Y));
    assert_eq!(s.stick, Vec2::new(0.0, -1.0), "the stronger stick wins");
    assert!(s.pad);
    // `poll` is player 1 of the same fold.
    input.feed_pads(&[pad(1, PadButton::East, Vec2::ZERO)]);
    let s = input.poll();
    assert!(s.is_down(Button::A) && s.is_down(Button::B));
}

#[test]
fn more_players_seat_pads_and_each_plays_with_its_bindings() {
    let mut input = PcInput::new();
    input.seats_mut().set_setup(PlayerSetup::up_to(2));
    // Player 2 moved A to the pad's north button.
    let mut all = [Bindings::default(); 4];
    all[1].bind_pad(Button::A, PadButton::North);
    input.set_all_bindings(all);
    input.handle_key(KeyCode::KeyZ, true);
    input.feed_pads(&[
        pad(10, PadButton::South, Vec2::ZERO),
        pad(11, PadButton::North, Vec2::new(1.0, 0.0)),
    ]);
    let p = input.poll_players();
    assert_eq!(p.connected, 0b11);
    assert!(
        p.players[0].is_down(Button::A),
        "keyboard Z and pad 1's South"
    );
    assert!(p.players[1].is_down(Button::A), "player 2's North is A");
    assert!(!p.players[1].is_down(Button::Y));
    assert_eq!(p.players[1].stick, Vec2::new(1.0, 0.0));
    // Published to the platform as the game sees it.
    let mut platform = PcPlatform::new(&trino_core::Caps::PC);
    platform.players.publish(input.seats());
    assert_eq!(platform.players().connected, 0b11);
    assert_eq!(platform.input_devices().len(), 3);
}

#[test]
fn rebinding_a_player_and_the_file_with_every_player() {
    let mut input = PcInput::new();
    let mut platform = PcPlatform::new(&trino_core::Caps::PC);
    platform.publish_input(&input);
    platform.rebind_player(2, Button::B, BindDevice::Keyboard);
    let req = platform.take_input_request().unwrap();
    assert_eq!(
        req,
        InputRequest::Rebind(2, Button::B, BindDevice::Keyboard)
    );
    input.apply(req);
    assert_eq!(input.capturing_player(), Some(2));
    input.handle_key(KeyCode::KeyM, true);
    assert_eq!(
        input
            .bindings_for(2)
            .label(Button::B, BindDevice::Keyboard)
            .as_deref(),
        Some("M")
    );
    assert_eq!(
        input
            .bindings()
            .label(Button::B, BindDevice::Keyboard)
            .as_deref(),
        Some("X")
    );
    platform.publish_input(&input);
    assert_eq!(
        platform.binding_label_for(2, Button::B, BindDevice::Keyboard),
        Some("M")
    );
    assert_eq!(
        platform.binding_label(Button::B, BindDevice::Keyboard),
        Some("X")
    );
    // The file: player 1 as before, player 3's lines prefixed.
    let text = players_bindings_to_text(input.all_bindings());
    assert!(text.starts_with("# trino-input 2\nA = Z |"), "{text}");
    assert!(text.contains("P3 B = M |"), "{text}");
    assert_eq!(&players_bindings_from_text(&text), input.all_bindings());
    // A file from before #67 is player 1's.
    let old = trino_platform_pc::input::bindings_to_text(&Bindings::default());
    assert_eq!(players_bindings_from_text(&old), [Bindings::default(); 4]);
}
