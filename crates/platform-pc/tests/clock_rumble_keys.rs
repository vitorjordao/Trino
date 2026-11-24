//! The PC's answers to Bongo-Kart #180, #182 and #183 through the public
//! API: the wall clock and its pin, rumble scaled by the player's setting and
//! forwarded to the pads, `rebind_key`, the sub-frame key tap, and
//! `save_image`'s PNG.

use trino_core::{
    AccessSettings, BindDevice, Button, Caps, Input, Key, Platform, RgbaImage, Rumble,
    RumbleSupport, SaveError,
};
use trino_platform_pc::input::InputRequest;
use trino_platform_pc::{PcInput, PcPlatform};
use winit::keyboard::KeyCode;

#[test]
fn the_wall_clock_is_utc_and_a_pin_keeps_running() {
    let mut p = PcPlatform::new(&Caps::PC);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let t = p.unix_seconds().expect("the PC has a clock");
    assert!(t.abs_diff(now) <= 1, "{t} vs {now}");
    assert!(t > 1_700_000_000);
    // A pinned clock starts where it was pinned (the 2026-01-01 daily
    // challenge in a test) and runs with the monotonic clock.
    p.pin_clock(1_767_225_600);
    let a = p.unix_seconds().unwrap();
    assert!((1_767_225_600..1_767_225_602).contains(&a));
    assert_eq!(trino_core::clock::day_index(a), 20_454);
}

#[test]
fn rumble_is_scaled_by_the_setting_and_reaches_the_pads() {
    let mut p = PcPlatform::new(&Caps::PC);
    let mut input = PcInput::new();
    // No motors at all: nothing is kept, nothing is sent.
    p.set_rumble_support(0, RumbleSupport::Unsupported);
    p.rumble(0, 1.0, 1.0, 500);
    assert_eq!(p.rumble_state(0), Rumble::OFF);
    assert_eq!(p.take_rumble(), None);

    // A pad that rumbles, the setting at 50 %.
    p.set_rumble_support(1, RumbleSupport::Ready);
    p.set_access(AccessSettings {
        rumble: 50,
        ..AccessSettings::DEFAULT
    });
    p.rumble(1, 1.0, 0.6, 400);
    let half = Rumble {
        low: 0.5,
        high: 0.3,
    };
    assert_eq!(p.rumble_state(1), half);
    assert_eq!(p.take_rumble(), Some((1, half, 400)));
    // Through the host's call: the pad layer gets the scaled request.
    p.rumble(1, 1.0, 0.6, 400);
    p.publish_rumble(&mut input);
    assert_eq!(input.rumble_sent(1), Some((half, 400)));
    assert_eq!(
        p.rumble_support(1),
        input.rumble_support(1),
        "publish_rumble reports what the pads can do"
    );

    // 0 % is off: the request stops the motors.
    p.set_rumble_support(1, RumbleSupport::Ready);
    p.set_access(AccessSettings {
        rumble: 0,
        ..AccessSettings::DEFAULT
    });
    p.rumble(1, 1.0, 1.0, 400);
    assert_eq!(p.rumble_state(1), Rumble::OFF);
    assert_eq!(p.take_rumble(), Some((1, Rumble::OFF, 0)));
    // Past the players: ignored.
    p.rumble(9, 1.0, 1.0, 400);
    assert_eq!(p.rumble_state(9), Rumble::OFF);
}

#[test]
fn rebind_key_binds_at_once_and_refuses_fixed_keys() {
    let mut p = PcPlatform::new(&Caps::PC);
    let mut input = PcInput::new();
    assert!(
        !p.rebind_key(0, Button::A, Key::J),
        "no input published: nothing to bind"
    );
    p.publish_input(&input);
    assert!(p.rebind_key(0, Button::A, Key::J));
    assert!(p.rebind_key(0, Button::B, Key::Z), "B takes A's old key");
    assert!(!p.rebind_key(0, Button::Y, Key::W), "W moves the stick");
    assert!(!p.rebind_key(0, Button::Y, Key::Escape));
    assert!(!p.rebind_key(9, Button::Y, Key::K));
    assert_eq!(
        p.take_input_request(),
        Some(InputRequest::BindKey(0, Button::A, KeyCode::KeyJ))
    );
    input.apply(InputRequest::BindKey(0, Button::A, KeyCode::KeyJ));
    input.apply(p.take_input_request().unwrap());
    assert_eq!(p.take_input_request(), None);
    assert!(input.take_changed(), "the host persists it");
    p.publish_input(&input);
    assert_eq!(p.binding_label(Button::A, BindDevice::Keyboard), Some("J"));
    assert_eq!(p.binding_label(Button::B, BindDevice::Keyboard), Some("Z"));
    input.handle_key(KeyCode::KeyJ, true);
    assert!(input.poll().is_down(Button::A));
    input.handle_key(KeyCode::KeyJ, false);
    // Shift binds the left one.
    assert_eq!(
        trino_platform_pc::input::key_code_for(Key::Shift),
        Some(KeyCode::ShiftLeft)
    );
}

#[test]
fn a_tap_shorter_than_a_frame_still_reads_down_once() {
    let mut input = PcInput::new();
    // Enter pressed and released between two polls (a 60 ms tap at 8 fps).
    input.handle_key(KeyCode::Enter, true);
    input.handle_key(KeyCode::Enter, false);
    input.handle_key(KeyCode::KeyD, true);
    input.handle_key(KeyCode::KeyD, false);
    let s = input.poll();
    assert!(s.is_down(Button::Start) && s.key(Key::Enter));
    assert!(s.key(Key::D) && s.stick.x > 0.0);
    let s = input.poll();
    assert!(!s.is_down(Button::Start) && !s.key(Key::Enter));
    assert_eq!(s.stick.x, 0.0);
    // A key still held is unaffected.
    input.handle_key(KeyCode::KeyZ, true);
    assert!(input.poll().is_down(Button::A));
    assert!(input.poll().is_down(Button::A));
}

#[test]
fn save_image_writes_a_png_beside_the_save() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = PcPlatform::new(&Caps::PC);
    let px: Vec<u8> = (0..4 * 3 * 2).map(|i| (i * 11) as u8).collect();
    let img = RgbaImage::new(3, 2, &px).unwrap();
    assert_eq!(p.save_image("shot", &img), Err(SaveError::Unsupported));

    let mut p = p.with_save_file(dir.path().join("save/game.sav"));
    p.save_image("race-1_result", &img).unwrap();
    let file = dir.path().join("save/images/race-1_result.png");
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&file).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut back = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut back).unwrap();
    assert_eq!((info.width, info.height), (3, 2));
    assert_eq!(&back[..info.buffer_size()], &px[..]);
    assert!(
        !dir.path()
            .join("save/images/race-1_result.png.tmp")
            .exists()
    );

    // Names that would leave the folder, and images whose sizes disagree.
    assert_eq!(p.save_image("../x", &img), Err(SaveError::Io));
    let wrong = RgbaImage {
        width: 4,
        height: 2,
        pixels: &px,
    };
    assert_eq!(p.save_image("wrong", &wrong), Err(SaveError::Io));
    assert!(!dir.path().join("save/images/wrong.png").exists());
}
