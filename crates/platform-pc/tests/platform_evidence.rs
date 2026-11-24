//! Evidence for ADR 0045 (Bongo-Kart #180, #182, #183), ignored by default:
//! `cargo test -p trino-platform-pc --test platform_evidence -- --ignored`
//! writes `target/evidence/platform/`:
//!
//! - `rumble.csv`: four race events through `HostedRumble` at the player's
//!   100 %, 50 % and 0 %, 60 frames a second, with the N64 Rumble Pak's
//!   `DutyCycle` per frame (the `evidence/epic24-platform` branch's
//!   `rumble/rumble_chart.py` draws it);
//! - `save/images/result_card.png`: a picture kept by `Platform::save_image`;
//! - `transcript.txt`: the wall clock, a pinned daily-challenge day, and an
//!   `ActionMap` remapped by presses, swaps and its save block.

use std::fmt::Write as _;
use std::path::PathBuf;

use trino_core::actions::ActionBinding;
use trino_core::clock::{civil_from_unix, day_index};
use trino_core::{
    AccessSettings, ActionMap, Button, Caps, DutyCycle, HostedRumble, InputState, Key, Platform,
    RgbaImage, pressed_key,
};
use trino_platform_pc::PcPlatform;

fn out() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/evidence/platform");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// (second, low, high, ms, name): a race's jolts.
const EVENTS: [(f32, f32, f32, u32, &str); 4] = [
    (0.25, 0.8, 0.3, 150, "bump"),
    (0.9, 0.3, 0.9, 600, "boost"),
    (2.0, 1.0, 1.0, 400, "item hit"),
    (2.8, 0.5, 0.2, 800, "wall scrape"),
];

#[test]
#[ignore = "evidence: writes target/evidence/platform"]
fn rumble_timeline() {
    let mut csv = String::from("setting,frame,low,high,n64\n");
    for setting in [100u8, 50, 0] {
        let scale = AccessSettings {
            rumble: setting,
            ..AccessSettings::DEFAULT
        }
        .rumble_factor();
        let mut r = HostedRumble::new();
        let mut duty = DutyCycle::new();
        for frame in 0..240u32 {
            let now = frame as u64 * 16_667;
            for (at, low, high, ms, _) in EVENTS {
                if frame == (at * 60.0) as u32 {
                    r.request(0, low, high, ms, now, scale);
                }
            }
            let s = r.state(0, now);
            let on = duty.step(s.strongest());
            writeln!(
                csv,
                "{setting},{frame},{:.3},{:.3},{}",
                s.low, s.high, on as u8
            )
            .unwrap();
        }
    }
    let mut events = String::from("second,low,high,ms,name\n");
    for (at, low, high, ms, name) in EVENTS {
        writeln!(events, "{at},{low},{high},{ms},{name}").unwrap();
    }
    std::fs::write(out().join("rumble.csv"), csv).unwrap();
    std::fs::write(out().join("rumble_events.csv"), events).unwrap();
}

#[test]
#[ignore = "evidence: writes target/evidence/platform"]
fn result_card_and_transcript() {
    let dir = out();
    let _ = std::fs::remove_dir_all(dir.join("save"));
    let mut p = PcPlatform::new(&Caps::PC).with_save_file(dir.join("save/bongo.sav"));
    // A 320x180 result card: a sky-to-track gradient, a checkered finish
    // line, three podium blocks.
    let (w, h) = (320u32, 180u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let t = y as f32 / h as f32;
            let mut c = [
                (40.0 + 160.0 * t) as u8,
                (120.0 + 60.0 * t) as u8,
                (230.0 - 150.0 * t) as u8,
            ];
            if (140..150).contains(&y) && ((x / 10) + (y / 5)) % 2 == 0 {
                c = [250, 250, 250];
            } else if (140..150).contains(&y) {
                c = [20, 20, 20];
            }
            for (x0, top, col) in [
                (130u32, 70u32, [255u8, 200, 40]),
                (70, 95, [200, 200, 210]),
                (190, 110, [205, 127, 50]),
            ] {
                if (x0..x0 + 60).contains(&x) && (top..140).contains(&y) {
                    c = col;
                }
            }
            px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    let img = RgbaImage::new(w, h, &px).unwrap();
    p.save_image("result_card", &img).unwrap();
    assert!(dir.join("save/images/result_card.png").exists());

    let mut t = String::new();
    let now = p.unix_seconds().unwrap();
    let c = civil_from_unix(now);
    writeln!(
        t,
        "unix_seconds() = {now}  ({:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC, weekday {})",
        c.year, c.month, c.day, c.hour, c.min, c.sec, c.weekday
    )
    .unwrap();
    writeln!(
        t,
        "daily challenge day = day_index(now) = {}",
        day_index(now)
    )
    .unwrap();
    p.pin_clock(1_798_761_600);
    let pinned = p.unix_seconds().unwrap();
    let pc = civil_from_unix(pinned);
    writeln!(
        t,
        "pinned (TRINO_UNIX_TIME=1798761600): {pinned} = {:04}-{:02}-{:02}, day {}",
        pc.year,
        pc.month,
        pc.day,
        day_index(pinned)
    )
    .unwrap();

    // Bongo Kart's actions as an ActionMap, remapped by presses.
    const NAMES: [&str; 5] = ["accelerate", "brake", "drift", "item", "look back"];
    let mut map = ActionMap::new([
        ActionBinding::key(Key::W)
            .or_key(Key::ArrowUp)
            .or_button(Button::A),
        ActionBinding::key(Key::S)
            .or_key(Key::ArrowDown)
            .or_button(Button::B),
        ActionBinding::key(Key::Space)
            .or_key(Key::Z)
            .or_button(Button::R),
        ActionBinding::key(Key::X)
            .or_key(Key::E)
            .or_button(Button::X),
        ActionBinding::key(Key::B).or_button(Button::Select),
    ]);
    let show = |t: &mut String, map: &ActionMap<5>| {
        for (i, b) in map.bindings().iter().enumerate() {
            let keys: Vec<&str> = b.keys.iter().flatten().map(|k| k.label()).collect();
            let buttons: Vec<String> = b
                .buttons
                .iter()
                .flatten()
                .map(|b| format!("{b:?}"))
                .collect();
            writeln!(
                t,
                "  {:<11} keys {:<14} pad {}",
                NAMES[i],
                keys.join(" / "),
                buttons.join(" / ")
            )
            .unwrap();
        }
    };
    writeln!(t, "\nActionMap defaults:").unwrap();
    show(&mut t, &map);
    // The player presses J for "item" (slot 1): Escape is the menu's cancel.
    let mut now_in = InputState::default();
    now_in.set_key(Key::J, true);
    let key = pressed_key(&now_in, &InputState::default(), &[Key::Escape]).unwrap();
    let swapped = map.rebind_key(3, 1, key);
    writeln!(
        t,
        "\npress J for item, slot 2 -> rebind_key(item, 1, {}) swapped with {swapped:?}",
        key.label()
    )
    .unwrap();
    // Then Space for "item" slot 1: drift had it and gets X back.
    let swapped = map.rebind_key(3, 0, Key::Space);
    writeln!(
        t,
        "press SPACE for item, slot 1 -> swapped with {:?} ({})",
        swapped,
        swapped.map_or("-", |i| NAMES[i])
    )
    .unwrap();
    show(&mut t, &map);
    let mut block = [0u8; ActionMap::<5>::save_len()];
    let n = map.write_save(&mut block).unwrap();
    let hex: Vec<String> = block[..n].iter().map(|b| format!("{b:02x}")).collect();
    writeln!(t, "\nsave block ({n} bytes): {}", hex.join(" ")).unwrap();
    let mut fresh = ActionMap::new([ActionBinding::NONE; 5]);
    assert!(fresh.read_save(&block));
    writeln!(t, "read back equal: {}", fresh.bindings() == map.bindings()).unwrap();
    std::fs::write(dir.join("transcript.txt"), t).unwrap();
}
