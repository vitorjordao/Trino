//! A game's actions through `ActionMap` (Bongo-Kart #183): reading them from
//! keys and buttons, remapping with swaps, catching the next press, and the
//! save block.

use trino_core::actions::{ACTIONS_SAVE_VERSION, ActionBinding};
use trino_core::{ActionMap, Button, InputState, Key, pressed_button, pressed_key};

const ACCEL: usize = 0;
const BRAKE: usize = 1;
const DRIFT: usize = 2;
const ITEM: usize = 3;

fn kart() -> ActionMap<4> {
    ActionMap::new([
        ActionBinding::key(Key::W)
            .or_key(Key::ArrowUp)
            .or_button(Button::A),
        ActionBinding::key(Key::S)
            .or_key(Key::ArrowDown)
            .or_button(Button::B),
        ActionBinding::key(Key::Space).or_button(Button::R),
        ActionBinding::key(Key::X).or_button(Button::X),
    ])
}

fn keys(ks: &[Key]) -> InputState {
    let mut s = InputState::default();
    for k in ks {
        s.set_key(*k, true);
    }
    s
}

#[test]
fn actions_read_any_of_their_keys_and_buttons() {
    let m = kart();
    assert!(m.down(ACCEL, &keys(&[Key::W])));
    assert!(m.down(ACCEL, &keys(&[Key::ArrowUp])));
    assert!(!m.down(BRAKE, &keys(&[Key::W])));
    let mut pad = InputState::default();
    pad.set(Button::A, true);
    assert!(m.down(ACCEL, &pad));
    assert!(m.binding(ACCEL).buttons_down(&pad) && !m.binding(ACCEL).keys_down(&pad));
    let (prev, now) = (InputState::default(), keys(&[Key::Space]));
    assert!(m.pressed(DRIFT, &now, &prev) && !m.pressed(DRIFT, &now, &now));
    assert!(m.released(DRIFT, &prev, &now));
    // Past the end: nothing, never a panic.
    assert!(!m.down(99, &now));
}

#[test]
fn taking_a_key_from_another_action_swaps_and_reports_it() {
    let mut m = kart();
    assert!(m.is_default());
    // Accelerate's first key becomes Space: drift had it and gets W back.
    assert_eq!(m.rebind_key(ACCEL, 0, Key::Space), Some(DRIFT));
    assert_eq!(
        m.binding(ACCEL).keys,
        [Some(Key::Space), Some(Key::ArrowUp)]
    );
    assert_eq!(m.binding(DRIFT).keys, [Some(Key::W), None]);
    assert!(m.down(DRIFT, &keys(&[Key::W])) && !m.down(ACCEL, &keys(&[Key::W])));
    // The action's own other slot just lets the key go.
    assert_eq!(m.rebind_key(ACCEL, 0, Key::ArrowUp), None);
    assert_eq!(m.binding(ACCEL).keys, [Some(Key::ArrowUp), None]);
    // A key nobody has: no swap.
    assert_eq!(m.rebind_key(ITEM, 1, Key::E), None);
    assert_eq!(m.binding(ITEM).keys, [Some(Key::X), Some(Key::E)]);
    // Buttons swap the same way.
    assert_eq!(m.rebind_button(BRAKE, 0, Button::A), Some(ACCEL));
    assert_eq!(m.binding(ACCEL).buttons, [Some(Button::B), None]);
    // Bad indexes change nothing.
    let before = m;
    assert_eq!(m.rebind_key(9, 0, Key::Q), None);
    assert_eq!(m.rebind_key(ACCEL, 2, Key::Q), None);
    m.clear_key(9, 0);
    assert_eq!(m, before);
    m.clear_key(ITEM, 1);
    m.clear_button(ITEM, 0);
    assert_eq!(m.binding(ITEM), ActionBinding::key(Key::X));
    assert!(!m.is_default());
    m.reset();
    assert_eq!(m, kart());
}

#[test]
fn the_next_press_skips_held_and_reserved_keys() {
    let prev = keys(&[Key::W]);
    let now = keys(&[Key::W, Key::Escape, Key::J]);
    assert_eq!(pressed_key(&now, &prev, &[Key::Escape]), Some(Key::J));
    assert_eq!(pressed_key(&now, &prev, &[]), Some(Key::Escape));
    assert_eq!(pressed_key(&prev, &prev, &[]), None);
    let mut pad = InputState::default();
    pad.set(Button::Start, true);
    pad.set(Button::Y, true);
    assert_eq!(
        pressed_button(&pad, &InputState::default(), &[Button::Start]),
        Some(Button::Y)
    );
    assert_eq!(Key::J.label(), "J");
    assert_eq!(Key::ArrowLeft.label(), "LEFT");
    // Every key has a non-empty ASCII label.
    for k in Key::ALL {
        assert!(!k.label().is_empty() && k.label().is_ascii(), "{k:?}");
    }
}

#[test]
fn the_save_block_round_trips_and_refuses_foreign_bytes() {
    let mut m = kart();
    m.rebind_key(ITEM, 1, Key::Slash);
    m.rebind_button(DRIFT, 1, Button::L2);
    m.clear_key(BRAKE, 1);
    let mut buf = [0u8; ActionMap::<4>::save_len()];
    assert_eq!(m.write_save(&mut buf), Some(18));
    assert_eq!(buf[0], ACTIONS_SAVE_VERSION);
    assert_eq!(buf[1], 4);
    let mut back = kart();
    assert!(back.read_save(&buf));
    assert_eq!(back, m);
    // Short output, other versions, other counts, unknown keys: refused,
    // and the map keeps what it had.
    assert_eq!(m.write_save(&mut [0u8; 17]), None);
    for bad in [
        &buf[..17],
        &[2, 4][..],
        &{
            let mut b = buf;
            b[1] = 3;
            b
        }[..],
        &{
            let mut b = buf;
            b[2] = 200;
            b
        }[..],
        &{
            let mut b = buf;
            b[5] = 16;
            b
        }[..],
    ] {
        let mut fresh = kart();
        assert!(!fresh.read_save(bad));
        assert_eq!(fresh, kart());
    }
}
