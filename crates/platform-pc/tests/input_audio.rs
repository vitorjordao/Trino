//! The PC keyboard/mouse backend through `trino_core::Input`, and the cpal
//! audio backend through `trino_core::Audio` (silent without a device,
//! same bookkeeping) plus its pure stereo mixer. No GPU needed.

use std::sync::Arc;

use trino_core::{
    Audio, Bus, Button, Input, InputState, Key, MusicId, PointerButton, SoundId, SoundParams, Vec2,
};
use trino_platform_pc::audio::mixer::{DUCK, Kind, Mixer, Voice};
use trino_platform_pc::input::{BINDABLE, Bindings, keys_for};
use trino_platform_pc::{PcAudio, PcInput};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

/// The documented default mapping, key by key, with edge detection over
/// consecutive polls.
#[test]
fn every_console_button_maps_from_its_key_with_edges() {
    let map = [
        (KeyCode::KeyZ, Button::A),
        (KeyCode::KeyX, Button::B),
        (KeyCode::KeyC, Button::X),
        (KeyCode::KeyV, Button::Y),
        (KeyCode::KeyQ, Button::L),
        (KeyCode::KeyE, Button::R),
        (KeyCode::Enter, Button::Start),
        (KeyCode::ShiftRight, Button::Select),
        (KeyCode::ArrowUp, Button::DpadUp),
        (KeyCode::ArrowDown, Button::DpadDown),
        (KeyCode::ArrowLeft, Button::DpadLeft),
        (KeyCode::ArrowRight, Button::DpadRight),
    ];
    let mut input = PcInput::new();
    let mut prev = input.poll();
    for (code, button) in map {
        input.handle_key(code, true);
        let now = input.poll();
        assert!(now.is_down(button), "{code:?} → {button:?}");
        assert!(now.just_pressed(&prev, button));
        let held = input.poll();
        assert!(held.is_down(button) && !held.just_pressed(&now, button));
        for (_, other) in map.iter().filter(|(_, b)| *b != button) {
            assert!(!held.is_down(*other), "{code:?} must not press {other:?}");
        }
        input.handle_key(code, false);
        let up = input.poll();
        assert!(up.just_released(&held, button));
        prev = up;
    }
    // Keys outside the mapping press no button: P only sets its key bit,
    // a key without a `Key` (F13) changes nothing.
    input.handle_key(KeyCode::KeyP, true);
    let p = input.poll();
    assert!(!p.any_button() && p.key(Key::P) && p.keys.len() == 1);
    input.handle_key(KeyCode::KeyP, false);
    input.handle_key(KeyCode::F13, true);
    assert_eq!(input.poll(), InputState::default());
}

/// Every PC key sets its own `Key` bit (numpad aliases included, `=` also
/// sets `Plus`), without pressing console buttons except the ones its
/// default binding gives it (Z = A, Enter = Start, arrows = D-pad…).
#[test]
fn every_pc_key_sets_its_bit() {
    use KeyCode as C;
    let keys: &[(KeyCode, &[Key])] = &[
        (C::Escape, &[Key::Escape]),
        (C::Enter, &[Key::Enter]),
        (C::NumpadEnter, &[Key::Enter]),
        (C::Space, &[Key::Space]),
        (C::Tab, &[Key::Tab]),
        (C::Delete, &[Key::Delete]),
        (C::Backspace, &[Key::Backspace]),
        (C::Insert, &[Key::Insert]),
        (C::Home, &[Key::Home]),
        (C::End, &[Key::End]),
        (C::PageUp, &[Key::PageUp]),
        (C::PageDown, &[Key::PageDown]),
        (C::ArrowUp, &[Key::ArrowUp]),
        (C::ArrowDown, &[Key::ArrowDown]),
        (C::ArrowLeft, &[Key::ArrowLeft]),
        (C::ArrowRight, &[Key::ArrowRight]),
        (C::F1, &[Key::F1]),
        (C::F2, &[Key::F2]),
        (C::F3, &[Key::F3]),
        (C::F4, &[Key::F4]),
        (C::F5, &[Key::F5]),
        (C::F6, &[Key::F6]),
        (C::F7, &[Key::F7]),
        (C::F8, &[Key::F8]),
        (C::F9, &[Key::F9]),
        (C::F10, &[Key::F10]),
        (C::F11, &[Key::F11]),
        (C::F12, &[Key::F12]),
        (C::Digit0, &[Key::Digit0]),
        (C::Digit1, &[Key::Digit1]),
        (C::Numpad1, &[Key::Digit1]),
        (C::Digit2, &[Key::Digit2]),
        (C::Numpad2, &[Key::Digit2]),
        (C::Digit3, &[Key::Digit3]),
        (C::Numpad3, &[Key::Digit3]),
        (C::Digit4, &[Key::Digit4]),
        (C::Digit5, &[Key::Digit5]),
        (C::Digit6, &[Key::Digit6]),
        (C::Digit7, &[Key::Digit7]),
        (C::Digit8, &[Key::Digit8]),
        (C::Digit9, &[Key::Digit9]),
        (C::KeyA, &[Key::A]),
        (C::KeyB, &[Key::B]),
        (C::KeyC, &[Key::C]),
        (C::KeyD, &[Key::D]),
        (C::KeyE, &[Key::E]),
        (C::KeyF, &[Key::F]),
        (C::KeyG, &[Key::G]),
        (C::KeyH, &[Key::H]),
        (C::KeyI, &[Key::I]),
        (C::KeyJ, &[Key::J]),
        (C::KeyK, &[Key::K]),
        (C::KeyL, &[Key::L]),
        (C::KeyM, &[Key::M]),
        (C::KeyN, &[Key::N]),
        (C::KeyO, &[Key::O]),
        (C::KeyP, &[Key::P]),
        (C::KeyQ, &[Key::Q]),
        (C::KeyR, &[Key::R]),
        (C::KeyS, &[Key::S]),
        (C::KeyT, &[Key::T]),
        (C::KeyU, &[Key::U]),
        (C::KeyV, &[Key::V]),
        (C::KeyW, &[Key::W]),
        (C::KeyX, &[Key::X]),
        (C::KeyY, &[Key::Y]),
        (C::KeyZ, &[Key::Z]),
        (C::Equal, &[Key::Equal, Key::Plus]),
        (C::NumpadAdd, &[Key::Plus]),
        (C::Minus, &[Key::Minus]),
        (C::NumpadSubtract, &[Key::Minus]),
        (C::ShiftLeft, &[Key::Shift]),
        (C::ShiftRight, &[Key::Shift]),
        (C::ControlLeft, &[Key::Ctrl]),
        (C::ControlRight, &[Key::Ctrl]),
        (C::AltLeft, &[Key::Alt]),
        (C::AltRight, &[Key::Alt]),
        (C::SuperLeft, &[Key::Super]),
        (C::SuperRight, &[Key::Super]),
        (C::BracketLeft, &[Key::BracketLeft]),
        (C::BracketRight, &[Key::BracketRight]),
        (C::Semicolon, &[Key::Semicolon]),
        (C::Quote, &[Key::Quote]),
        (C::Comma, &[Key::Comma]),
        (C::Period, &[Key::Period]),
        (C::Slash, &[Key::Slash]),
        (C::Backslash, &[Key::Backslash]),
        (C::Backquote, &[Key::Backquote]),
    ];
    // Every Key is reachable from some key.
    for k in Key::ALL {
        assert!(
            keys.iter().any(|(_, ks)| ks.contains(&k)),
            "{k:?} has no key"
        );
    }
    let defaults = Bindings::default();
    for (code, expect) in keys {
        assert_eq!(keys_for(*code), *expect, "{code:?}");
        let mut input = PcInput::new();
        let prev = input.poll();
        input.handle_key(*code, true);
        let s = input.poll();
        for key in *expect {
            assert!(
                s.key(*key) && s.key_pressed(&prev, *key),
                "{code:?} → {key:?}"
            );
        }
        assert_eq!(s.keys.len(), expect.len(), "{code:?} sets only its bits");
        let bound: Vec<Button> = BINDABLE
            .iter()
            .enumerate()
            .filter(|(i, _)| defaults.keys[*i] == Some(*code))
            .map(|(_, b)| *b)
            .collect();
        let pressed: Vec<Button> = Button::ALL.into_iter().filter(|b| s.is_down(*b)).collect();
        assert_eq!(pressed, bound, "{code:?} presses only its default button");
        input.handle_key(*code, false);
        assert!(input.poll().keys.is_empty());
        assert_eq!(input.poll().keys, 0, "u32-era comparison still works");
    }
}

/// Gamepads without a device: a `PadReading` folds into the state through
/// the default bindings (L2/R2 = triggers, L3/R3 = stick clicks, both
/// sticks, analog triggers), and gilrs' trigger values become 0..=1.
#[test]
fn gamepad_readings_map_to_buttons_sticks_and_triggers() {
    use trino_platform_pc::input::{PadButton as P, PadReading, trigger_value};
    let input = PcInput::new();
    let fold = |r: PadReading| {
        let mut s = InputState::default();
        input.merge_pad(&mut s, &r);
        s
    };
    assert_eq!(fold(PadReading::default()), InputState::default());
    for (pad, buttons) in [
        (P::South, &[Button::A][..]),
        (P::East, &[Button::B]),
        (P::West, &[Button::X]),
        (P::North, &[Button::Y]),
        (P::LeftTrigger, &[Button::L]),
        (P::RightTrigger, &[Button::R]),
        (P::LeftTrigger2, &[Button::L, Button::L2]),
        (P::RightTrigger2, &[Button::R, Button::R2]),
        (P::LeftThumb, &[Button::Select, Button::L3]),
        (P::RightThumb, &[Button::R3]),
        (P::Start, &[Button::Start]),
        (P::Mode, &[Button::Start]),
        (P::Select, &[Button::Select]),
        (P::DPadUp, &[Button::DpadUp]),
        (P::DPadDown, &[Button::DpadDown]),
        (P::DPadLeft, &[Button::DpadLeft]),
        (P::DPadRight, &[Button::DpadRight]),
    ] {
        let mut r = PadReading::default();
        r.press(pad);
        let s = fold(r);
        let pressed: Vec<Button> = Button::ALL.into_iter().filter(|b| s.is_down(*b)).collect();
        let mut want = buttons.to_vec();
        want.sort_by_key(|b| *b as u16);
        assert_eq!(pressed, want, "{pad:?}");
        assert!(s.pad, "{pad:?} marks the pad as used");
    }
    // Sticks: Y-up, the stronger one wins; small motion is not "pad used".
    let s = fold(PadReading {
        left_stick: Vec2::new(0.5, -1.0),
        right_stick: Vec2::new(-1.0, 0.25),
        triggers: Vec2::new(0.75, 1.5),
        ..Default::default()
    });
    assert_eq!(s.stick, Vec2::new(0.5, -1.0));
    assert_eq!(s.right_stick, Vec2::new(-1.0, 0.25));
    assert_eq!(s.triggers, Vec2::new(0.75, 1.0), "clamped to 0..=1");
    assert!(s.pad);
    let drift = fold(PadReading {
        right_stick: Vec2::new(0.1, 0.1),
        triggers: Vec2::new(0.1, 0.0),
        ..Default::default()
    });
    assert!(!drift.pad, "drift under the deadzone");
    assert_eq!(drift.right_stick, Vec2::new(0.1, 0.1));
    // Two pads: the stronger right stick and the larger trigger win.
    let mut two = drift;
    input.merge_pad(
        &mut two,
        &PadReading {
            right_stick: Vec2::new(0.0, -0.9),
            triggers: Vec2::new(0.05, 0.6),
            ..Default::default()
        },
    );
    assert_eq!(two.right_stick, Vec2::new(0.0, -0.9));
    assert_eq!(two.triggers, Vec2::new(0.1, 0.6));
    // gilrs trigger values: button value, Z axis (negative at rest on some
    // pads), digital triggers.
    assert_eq!(trigger_value(false, 0.0, 0.0), 0.0);
    assert_eq!(trigger_value(true, 0.4, 0.0), 0.4);
    assert_eq!(trigger_value(false, 0.0, 0.7), 0.7);
    assert_eq!(trigger_value(false, 0.0, -1.0), 0.0);
    assert_eq!(trigger_value(true, 0.0, -1.0), 1.0, "digital trigger");
    assert_eq!(trigger_value(false, 2.0, 0.0), 1.0);
}

/// Keys bound to L2/R2 pull the triggers fully; the default keyboard has
/// none, so a fresh keyboard never reports triggers.
#[test]
fn keys_bound_to_l2_r2_are_full_triggers() {
    let mut input = PcInput::new();
    let mut b = Bindings::default();
    b.bind_key(Button::L2, KeyCode::KeyF);
    b.bind_key(Button::R3, KeyCode::KeyG);
    input.set_bindings(b);
    assert_eq!(input.poll().triggers, Vec2::ZERO);
    input.handle_key(KeyCode::KeyF, true);
    input.handle_key(KeyCode::KeyG, true);
    let s = input.poll();
    assert!(s.is_down(Button::L2) && s.is_down(Button::R3) && !s.is_down(Button::R2));
    assert_eq!(s.triggers, Vec2::new(1.0, 0.0));
    assert!(s.key(Key::F), "a bound key still sets its key bit");
    input.handle_key(KeyCode::KeyF, false);
    assert_eq!(input.poll().triggers, Vec2::ZERO);
}

/// WASD is the digital stick (Y-up), mixing with the arrows' D-pad.
#[test]
fn wasd_stick_and_pointer_state_across_polls() {
    let mut input = PcInput::new();
    for (code, stick) in [
        (KeyCode::KeyW, Vec2::new(0.0, 1.0)),
        (KeyCode::KeyS, Vec2::new(0.0, -1.0)),
        (KeyCode::KeyA, Vec2::new(-1.0, 0.0)),
        (KeyCode::KeyD, Vec2::new(1.0, 0.0)),
    ] {
        input.handle_key(code, true);
        assert_eq!(input.poll().stick, stick, "{code:?}");
        input.handle_key(code, false);
        assert_eq!(input.poll().stick, Vec2::ZERO);
    }

    // The pointer: position on either screen, three buttons, the wheel as a
    // per-poll delta; other mouse buttons are ignored.
    let prev = input.poll();
    input.handle_pointer_on(Some(Vec2::new(100.0, 50.0)), 1);
    input.handle_mouse_button(MouseButton::Right, true);
    input.handle_mouse_button(MouseButton::Back, true);
    input.handle_wheel(-1.0);
    let s = input.poll();
    assert_eq!(
        (s.pointer, s.pointer_screen),
        (Some(Vec2::new(100.0, 50.0)), 1)
    );
    assert!(s.pointer_pressed(&prev, PointerButton::Secondary));
    assert!(!s.pointer_down(PointerButton::Primary) && !s.pointer_down(PointerButton::Middle));
    assert_eq!(s.wheel, -1.0);
    input.handle_pointer(Some(Vec2::new(3.0, 4.0)));
    input.handle_mouse_button(MouseButton::Right, false);
    let later = input.poll();
    assert_eq!(later.pointer_screen, 0, "handle_pointer = main screen");
    assert!(later.pointer_released(&s, PointerButton::Secondary));
    assert_eq!(later.wheel, 0.0);
    assert!(!later.pad, "no gamepad was used");
}

fn tone(frames: usize, value: f32) -> Vec<f32> {
    vec![value; frames]
}

/// The `Audio` trait on the PC backend: voices, music, stingers and
/// volumes. Sounds are a minute long, so a real device draining the mixer
/// in the background cannot finish them mid-test.
#[test]
fn audio_voices_music_and_stingers_bookkeeping() {
    let mut audio = PcAudio::new();
    let rate = audio.sample_rate();
    assert!(rate >= 8_000, "a usable sample rate, even silent: {rate}");
    let (beep, song, sting) = (SoundId(1), MusicId(2), MusicId(3));
    let minute = rate as usize * 60;
    audio.upload_sound(beep, tone(minute, 0.25));
    audio.upload_music(song, tone(minute, 0.1));
    audio.upload_music(sting, tone(minute, 0.2));

    // Unknown handles do nothing.
    audio.play_sound(SoundId(99));
    audio.play_sound_with(SoundId(99), &SoundParams::DEFAULT);
    assert_eq!(
        audio.play_voice(SoundId(99), &SoundParams::DEFAULT, true),
        None
    );
    audio.play_music(MusicId(99), true);
    audio.play_stinger(MusicId(99));
    assert!(!audio.stinger_playing());

    // Steerable voices: start, steer, stop.
    let v = audio
        .play_voice(beep, &SoundParams::DEFAULT, true)
        .expect("a free voice");
    assert!(audio.voice_playing(v));
    audio.set_voice(
        v,
        &SoundParams {
            volume: 0.5,
            pan: -1.0,
            ..SoundParams::DEFAULT
        },
    );
    assert!(audio.voice_playing(v), "steering keeps it playing");
    audio.stop_voice(v);
    assert!(!audio.voice_playing(v));
    audio.set_voice(v, &SoundParams::DEFAULT); // stopped: ignored

    // One-shots share the voice pool: 24 voices at most.
    audio.play_sound(beep);
    audio.play_sound_with(
        beep,
        &SoundParams {
            volume: 0.3,
            pan: 0.5,
            ..SoundParams::DEFAULT
        },
    );
    let mut held = Vec::new();
    while let Some(v) = audio.play_voice(beep, &SoundParams::DEFAULT, true) {
        held.push(v);
        assert!(held.len() <= 24, "the pool is bounded");
    }
    assert_eq!(held.len(), 22, "24 voices minus the two one-shots");
    assert!(held.iter().all(|&v| audio.voice_playing(v)));
    let ids: std::collections::BTreeSet<_> = held.iter().map(|v| v.0).collect();
    assert_eq!(ids.len(), held.len(), "voice ids are unique");
    for v in held.drain(..) {
        audio.stop_voice(v);
    }
    assert!(
        audio
            .play_voice(beep, &SoundParams::DEFAULT, false)
            .is_some()
    );

    // Music and stingers: one of each at a time.
    audio.play_music(song, true);
    audio.play_music(song, false); // replaces, does not stack
    audio.play_stinger(sting);
    assert!(audio.stinger_playing());
    audio.play_stinger(sting);
    assert!(audio.stinger_playing());
    audio.stop_music();
    assert!(audio.stinger_playing(), "stop_music leaves the stinger");
    audio.set_music_volume(2.0);
    audio.set_master_volume(-1.0);
    audio.set_master_volume(0.8);

    let mut defaulted = PcAudio::default();
    assert_eq!(defaulted.sample_rate(), PcAudio::new().sample_rate());
    defaulted.stop_music();
}

fn voice(id: u32, data: &[f32], looped: bool, kind: Kind, gains: (f32, f32)) -> Voice {
    let bus = if kind == Kind::Sfx {
        Bus::Sfx
    } else {
        Bus::Music
    };
    Voice::pcm(id, Arc::new(data.to_vec()), looped, kind, bus, gains)
}

/// A mixer at 1 kHz with the master and music buses at `master`/`music`.
fn mixer(voices: Vec<Voice>, master: f32, music: f32) -> Mixer {
    let mut m = Mixer::new(1000);
    m.voices = voices;
    m.buses.set_volume(Bus::Master, master);
    m.buses.set_volume(Bus::Music, music);
    m
}

/// The stereo mixer behind the stream: per-voice gains and pan, master and
/// music levels, ducking under a stinger, looping, clamping and dropping
/// finished one-shots.
#[test]
fn mixer_gains_ducking_looping_and_clamping() {
    let left_half = SoundParams {
        volume: 0.5,
        pan: -1.0,
        ..SoundParams::DEFAULT
    }
    .gains();
    assert!(left_half.0 > 0.4 && left_half.1 < 0.01, "{left_half:?}");

    let mut out = [9.0f32; 8];
    let mut m = mixer(
        vec![
            voice(1, &[0.4, 0.4], false, Kind::Sfx, (1.0, 0.5)),
            voice(2, &[0.2], true, Kind::Music, (1.0, 1.0)),
        ],
        0.5,
        1.0,
    );
    m.mix(&mut out);
    // Frame 0: sfx 0.4×(1, .5)×.5 + music .2×.5.
    assert!(
        (out[0] - 0.3).abs() < 1e-6 && (out[1] - 0.2).abs() < 1e-6,
        "{out:?}"
    );
    // Frame 2: the one-shot ended, the looped music goes on.
    assert!((out[4] - 0.1).abs() < 1e-6 && (out[5] - 0.1).abs() < 1e-6);
    assert_eq!(m.voices.len(), 1, "finished one-shots are dropped");
    assert_eq!(m.voices[0].id, 2);

    // A stinger ducks the music (DUCK) and plays at the music level.
    m.voices
        .push(voice(3, &[0.5; 8], false, Kind::Stinger, (1.0, 1.0)));
    m.buses.set_volume(Bus::Master, 1.0);
    m.buses.set_volume(Bus::Music, 0.5);
    m.mix(&mut out);
    let want = 0.2 * 0.5 * DUCK + 0.5 * 0.5;
    assert!((out[0] - want).abs() < 1e-6, "{} vs {want}", out[0]);

    // Loud voices clamp to the -1..1 range; an empty looped voice is safe.
    let mut loud = mixer(
        vec![
            voice(4, &[0.9; 4], false, Kind::Sfx, (1.0, 1.0)),
            voice(5, &[-0.9; 4], false, Kind::Sfx, (0.0, 0.0)),
            voice(6, &[0.9; 4], false, Kind::Sfx, (1.0, -3.0)),
            voice(7, &[], true, Kind::Sfx, (1.0, 1.0)),
        ],
        1.0,
        1.0,
    );
    loud.mix(&mut out);
    assert_eq!((out[0], out[1]), (1.0, -1.0));
    assert_eq!(loud.voices.len(), 1, "only the looped (empty) voice stays");
}

#[test]
fn newly_supported_keys_are_bindable_with_labels() {
    use trino_platform_pc::input::key_label;
    use winit::keyboard::KeyCode;

    assert_eq!(key_label(KeyCode::Space), Some("SPACE"));
    assert_eq!(key_label(KeyCode::Tab), Some("TAB"));
    assert_eq!(key_label(KeyCode::Backspace), Some("BACKSPACE"));
    assert_eq!(key_label(KeyCode::Digit1), Some("1"));
    assert_eq!(key_label(KeyCode::Digit2), Some("2"));
    assert_eq!(key_label(KeyCode::Digit3), Some("3"));
    assert_eq!(key_label(KeyCode::KeyR), Some("R"));
    assert_eq!(key_label(KeyCode::KeyT), Some("T"));
}
