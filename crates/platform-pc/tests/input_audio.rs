//! The PC keyboard/mouse backend through `trino_core::Input`, and the cpal
//! audio backend through `trino_core::Audio` (silent without a device,
//! same bookkeeping) plus its pure stereo mixer. No GPU needed.

use std::sync::Arc;

use trino_core::{
    Audio, Button, Input, InputState, Key, MusicId, PointerButton, SoundId, SoundParams, Vec2,
};
use trino_platform_pc::audio::mixer::{DUCK, Kind, Voice, mix_into};
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
    // Keys outside the mapping change nothing.
    input.handle_key(KeyCode::KeyP, true);
    assert_eq!(input.poll(), InputState::default());
}

/// Every PC key sets its own `Key` bit (numpad aliases included), without
/// pressing console buttons except where documented (Enter = Start,
/// right Shift = Select).
#[test]
fn every_pc_key_sets_its_bit() {
    let keys = [
        (KeyCode::Escape, Key::Escape),
        (KeyCode::Enter, Key::Enter),
        (KeyCode::NumpadEnter, Key::Enter),
        (KeyCode::Space, Key::Space),
        (KeyCode::Tab, Key::Tab),
        (KeyCode::Delete, Key::Delete),
        (KeyCode::Backspace, Key::Backspace),
        (KeyCode::F1, Key::F1),
        (KeyCode::F2, Key::F2),
        (KeyCode::F3, Key::F3),
        (KeyCode::F4, Key::F4),
        (KeyCode::F5, Key::F5),
        (KeyCode::F6, Key::F6),
        (KeyCode::F7, Key::F7),
        (KeyCode::F8, Key::F8),
        (KeyCode::Digit1, Key::Digit1),
        (KeyCode::Numpad1, Key::Digit1),
        (KeyCode::Digit2, Key::Digit2),
        (KeyCode::Numpad2, Key::Digit2),
        (KeyCode::Digit3, Key::Digit3),
        (KeyCode::Numpad3, Key::Digit3),
        (KeyCode::PageUp, Key::PageUp),
        (KeyCode::PageDown, Key::PageDown),
        (KeyCode::Home, Key::Home),
        (KeyCode::KeyR, Key::R),
        (KeyCode::KeyW, Key::W),
        (KeyCode::KeyA, Key::A),
        (KeyCode::KeyS, Key::S),
        (KeyCode::KeyD, Key::D),
        (KeyCode::Equal, Key::Plus),
        (KeyCode::NumpadAdd, Key::Plus),
        (KeyCode::Minus, Key::Minus),
        (KeyCode::NumpadSubtract, Key::Minus),
        (KeyCode::ShiftLeft, Key::Shift),
        (KeyCode::ShiftRight, Key::Shift),
        (KeyCode::ControlLeft, Key::Ctrl),
        (KeyCode::ControlRight, Key::Ctrl),
        (KeyCode::KeyT, Key::T),
    ];
    for (code, key) in keys {
        let mut input = PcInput::new();
        let prev = input.poll();
        input.handle_key(code, true);
        let s = input.poll();
        assert!(
            s.key(key) && s.key_pressed(&prev, key),
            "{code:?} → {key:?}"
        );
        assert_eq!(s.keys.count_ones(), 1, "{code:?} sets one key bit");
        let buttons = [
            Button::A,
            Button::B,
            Button::X,
            Button::Y,
            Button::L,
            Button::R,
            Button::Start,
            Button::Select,
        ];
        let pressed: Vec<_> = buttons.iter().filter(|b| s.is_down(**b)).collect();
        match code {
            KeyCode::Enter => assert_eq!(pressed, [&Button::Start]),
            KeyCode::ShiftRight => assert_eq!(pressed, [&Button::Select]),
            _ => assert!(pressed.is_empty(), "{code:?} pressed {pressed:?}"),
        }
        input.handle_key(code, false);
        assert_eq!(input.poll().keys, 0);
    }
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
    Voice {
        id,
        data: Arc::new(data.to_vec()),
        cursor: 0,
        looped,
        kind,
        gains,
    }
}

/// The stereo mixer behind the stream: per-voice gains and pan, master and
/// music levels, ducking under a stinger, looping, clamping and dropping
/// finished one-shots.
#[test]
fn mixer_gains_ducking_looping_and_clamping() {
    let left_half = SoundParams {
        volume: 0.5,
        pan: -1.0,
    }
    .gains();
    assert!(left_half.0 > 0.4 && left_half.1 < 0.01, "{left_half:?}");

    let mut out = [9.0f32; 8];
    let mut voices = vec![
        voice(1, &[0.4, 0.4], false, Kind::Sfx, (1.0, 0.5)),
        voice(2, &[0.2], true, Kind::Music, (1.0, 1.0)),
    ];
    mix_into(&mut out, &mut voices, 0.5, 1.0);
    // Frame 0: sfx 0.4×(1, .5)×.5 + music .2×.5.
    assert!(
        (out[0] - 0.3).abs() < 1e-6 && (out[1] - 0.2).abs() < 1e-6,
        "{out:?}"
    );
    // Frame 2: the one-shot ended, the looped music goes on.
    assert!((out[4] - 0.1).abs() < 1e-6 && (out[5] - 0.1).abs() < 1e-6);
    assert_eq!(voices.len(), 1, "finished one-shots are dropped");
    assert_eq!(voices[0].id, 2);

    // A stinger ducks the music (DUCK) and plays at the music level.
    voices.push(voice(3, &[0.5; 8], false, Kind::Stinger, (1.0, 1.0)));
    mix_into(&mut out, &mut voices, 1.0, 0.5);
    let want = 0.2 * 0.5 * DUCK + 0.5 * 0.5;
    assert!((out[0] - want).abs() < 1e-6, "{} vs {want}", out[0]);

    // Loud voices clamp to the -1..1 range; an empty looped voice is safe.
    let mut loud = vec![
        voice(4, &[0.9; 4], false, Kind::Sfx, (1.0, 1.0)),
        voice(5, &[-0.9; 4], false, Kind::Sfx, (0.0, 0.0)),
        voice(6, &[0.9; 4], false, Kind::Sfx, (1.0, -3.0)),
        voice(7, &[], true, Kind::Sfx, (1.0, 1.0)),
    ];
    mix_into(&mut out, &mut loud, 1.0, 1.0);
    assert_eq!((out[0], out[1]), (1.0, -1.0));
    assert_eq!(loud.len(), 1, "only the looped (empty) voice stays");
}
