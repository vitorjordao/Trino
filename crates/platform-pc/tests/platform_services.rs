//! `PcPlatform` through the `Platform` trait (blobs from a real bake, the
//! save file, display settings, the second screen) and the console
//! simulation profiles. No GPU needed.

mod common;

use trino_core::save::{self, HEADER};
use trino_core::{BlobId, Caps, DisplayInfo, DisplaySettings, Platform, SaveError};
use trino_platform_pc::sim::{
    MAX_ASPECT, MIN_ASPECT, MIN_LINES, auto_pixel_scale, native_internal,
};
use trino_platform_pc::system::{display_settings_from_text, display_settings_to_text};
use trino_platform_pc::{PcPlatform, SimProfile};

/// `[blobs.*]` baked by the pipeline reach the game byte for byte through
/// `Platform::blob`; a live reload replaces them under the same handle.
#[test]
fn baked_blobs_are_served_by_handle() {
    let dir = tempfile::tempdir().unwrap();
    let assets = common::bake_project(dir.path());
    let mut p = PcPlatform::new(&Caps::N64);
    for b in &assets.blobs {
        p.insert_blob(BlobId(b.id), b.bytes.clone());
    }
    let id = BlobId::from_path(common::BLOB);
    assert_eq!(p.blob(id), Some(common::BLOB_BYTES));
    assert_eq!(p.blob(BlobId::from_path("blobs/missing")), None);
    p.insert_blob(id, b"v2".to_vec());
    assert_eq!(p.blob(id), Some(&b"v2"[..]), "reload swaps the bytes");
}

/// Saves: capacity per profile, round trip through a file, framing with
/// `core::save` (what the console backends store), torn data, the size
/// cap, I/O failures and no save file at all.
#[test]
fn save_file_round_trip_framing_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/dir/game.sav");
    for (caps, cap) in [(Caps::N64, 2048), (Caps::N3DS, 65_536), (Caps::PC, 1 << 20)] {
        assert_eq!(
            PcPlatform::new(&caps).with_save_file(&path).save_capacity(),
            cap
        );
    }
    let mut p = PcPlatform::new(&Caps::N64).with_save_file(&path);
    let mut buf = [0u8; 64];
    assert_eq!(p.save_read(&mut buf), Ok(0), "no save yet");

    // A framed save, like the consoles keep: header ‖ data. The PC keeps
    // it in A/B slot files beside the save path (issue #68).
    let data = b"level=4;coins=17";
    let mut framed = save::header(data).to_vec();
    framed.extend_from_slice(data);
    p.save_write(&framed).unwrap();
    let slot_a = dir.path().join("nested/dir/game.sav.a");
    assert!(slot_a.exists(), "parent directories are created");
    assert!(
        !dir.path().join("nested/dir/game.sav.a.tmp").exists(),
        "atomic write cleans up"
    );
    let n = p.save_read(&mut buf).unwrap();
    assert_eq!(n, HEADER + data.len());
    let header: [u8; HEADER] = buf[..HEADER].try_into().unwrap();
    assert_eq!(save::data_len(&header), Some(data.len()));
    assert!(save::verify(&header, &buf[HEADER..n]));
    assert_eq!(p.save_status(), trino_core::SaveStatus::Ok);

    // A torn write (one flipped bit on disk) with no backup: no save, said.
    let mut torn = std::fs::read(&slot_a).unwrap();
    torn[save::SLOT_HEADER + HEADER + 2] ^= 0x10;
    std::fs::write(&slot_a, &torn).unwrap();
    assert_eq!(p.save_read(&mut buf), Ok(0), "torn save = no save");
    assert_eq!(p.save_status(), trino_core::SaveStatus::Corrupted);
    p.save_write(&framed).unwrap();

    // A short buffer gets a prefix; the full length is reported.
    let mut small = [0u8; 4];
    assert_eq!(p.save_read(&mut small), Ok(n));
    assert_eq!(&small, &framed[..4]);

    // Over the N64's 16 Kbit EEPROM: rejected, the old save stays.
    let before = (
        std::fs::read(&slot_a).ok(),
        std::fs::read(dir.path().join("nested/dir/game.sav.b")).ok(),
    );
    assert_eq!(
        p.save_write(&[1u8; 2049]),
        Err(SaveError::TooLarge {
            len: 2049,
            capacity: 2048
        })
    );
    let after = (
        std::fs::read(&slot_a).ok(),
        std::fs::read(dir.path().join("nested/dir/game.sav.b")).ok(),
    );
    assert_eq!(before, after);
    p.save_write(&[7u8; 2048])
        .expect("exactly the capacity fits");

    // A directory where the file should be: I/O errors, not panics.
    let blocked = dir.path().join("blocked");
    std::fs::create_dir_all(blocked.join("game.sav")).unwrap();
    std::fs::create_dir_all(blocked.join("game.sav.a.tmp")).unwrap();
    let mut q = PcPlatform::new(&Caps::N64).with_save_file(blocked.join("game.sav"));
    assert_eq!(q.save_read(&mut buf), Err(SaveError::Io));
    assert_eq!(q.save_write(b"x"), Err(SaveError::Io));
    // A file where the parent directory should be.
    std::fs::write(dir.path().join("file"), b"").unwrap();
    let mut r = PcPlatform::new(&Caps::N64).with_save_file(dir.path().join("file/game.sav"));
    assert_eq!(r.save_write(b"x"), Err(SaveError::Io));

    // No save file configured: unsupported, zero capacity.
    let mut none = PcPlatform::new(&Caps::N64);
    assert_eq!(none.save_capacity(), 0);
    assert_eq!(none.save_read(&mut buf), Err(SaveError::Unsupported));
    assert_eq!(none.save_write(b"x"), Err(SaveError::Unsupported));
}

/// The clock, the log, the second screen and the display request flow a
/// host drives (publish info → game asks → host takes the request).
#[test]
fn clock_log_second_screen_and_display_requests() {
    let mut p = PcPlatform::new(&Caps::PC);
    let t0 = p.now_micros();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let t1 = p.now_micros();
    assert!(t1 >= t0 + 1_000, "monotonic microseconds ({t0} → {t1})");
    p.log("platform_services: log line");

    assert_eq!(p.second_screen(), None);
    p.set_second_screen(Some((320, 240)));
    assert_eq!(p.second_screen(), Some((320, 240)));
    p.set_second_screen(None);
    assert_eq!(p.second_screen(), None);

    let wanted = DisplaySettings {
        fullscreen: false,
        window_w: 1280,
        window_h: 720,
        pixel_scale: 2,
        ui_scale: 1,
    };
    // No display published (consoles, tests): requests are dropped.
    assert_eq!(p.display(), None);
    p.set_display(wanted);
    assert_eq!(p.take_display_request(), None);
    let info = DisplayInfo {
        settings: DisplaySettings::DEFAULT,
        output: (1920, 1080),
        monitor: (1920, 1080),
        screen: (640, 360),
        pixel_scale: 3,
    };
    p.set_display_info(Some(info));
    assert_eq!(p.display(), Some(info));
    p.set_display(DisplaySettings::DEFAULT);
    p.set_display(wanted);
    assert_eq!(p.take_display_request(), Some(wanted), "the last one wins");
    assert_eq!(p.take_display_request(), None, "taken once");
}

/// The display settings file: round trip, hand-edited variants, broken
/// lines and out-of-range values.
#[test]
fn display_settings_text_round_trip_and_repair() {
    for s in [
        DisplaySettings::DEFAULT,
        DisplaySettings {
            fullscreen: false,
            window_w: 1920,
            window_h: 1200,
            pixel_scale: 4,
            ui_scale: 2,
        },
    ] {
        let text = display_settings_to_text(&s);
        assert!(text.ends_with('\n') && text.lines().count() == 4, "{text}");
        assert_eq!(display_settings_from_text(&text), s);
    }
    let edited = display_settings_from_text(
        " fullscreen = true \nwindow= 800x600\npixel_scale=99\nui_scale=50\nnoise\nextra=1\n",
    );
    assert_eq!(
        edited,
        DisplaySettings {
            fullscreen: true,
            window_w: 800,
            window_h: 600,
            pixel_scale: 16,
            ui_scale: 8,
        },
        "values trimmed and clamped"
    );
    let broken =
        display_settings_from_text("fullscreen=no\nwindow=800\npixel_scale=x\nui_scale=-1");
    assert_eq!(
        broken,
        DisplaySettings {
            fullscreen: false,
            ..DisplaySettings::DEFAULT
        },
        "broken values fall back to the default"
    );
    assert_eq!(display_settings_from_text(""), DisplaySettings::DEFAULT);
}

/// Every simulation profile: name aliases, budgets and resolutions, and the
/// native profile's picture-size rules.
#[test]
fn sim_profiles_and_native_picture_sizes() {
    for (names, profile, res, caps) in [
        (
            &["native", "PC"][..],
            SimProfile::Native,
            (640, 480),
            Caps::PC,
        ),
        (&["n64", "N64"][..], SimProfile::N64, (320, 240), Caps::N64),
        (
            &["3ds", "N3DS"][..],
            SimProfile::N3ds,
            (400, 240),
            Caps::N3DS,
        ),
    ] {
        for n in names {
            assert_eq!(SimProfile::parse(n), Some(profile), "{n}");
        }
        assert_eq!(profile.internal_resolution(), res);
        assert_eq!(profile.caps(), caps);
    }
    assert_eq!(SimProfile::parse(""), None);
    assert_eq!(SimProfile::default(), SimProfile::Native);

    assert_eq!(auto_pixel_scale(0), 1);
    assert_eq!(auto_pixel_scale(720), 2);
    assert_eq!(auto_pixel_scale(1080), 3);
    assert_eq!(auto_pixel_scale(2160), 7);
    for out in [(1280, 720), (1920, 1080), (3440, 1440), (800, 1280), (1, 1)] {
        for scale in 0..=5 {
            let (w, h, k) = native_internal(out, scale);
            assert!(k >= 1 && w % 2 == 0 && h % 2 == 0, "{out:?} {scale}");
            assert!(
                w * k <= out.0.max(2) && h * k <= out.1.max(2),
                "fits {out:?}"
            );
            let aspect = w as f32 / h as f32;
            if h >= MIN_LINES {
                assert!(
                    (MIN_ASPECT - 0.01..=MAX_ASPECT + 0.01).contains(&aspect),
                    "{out:?} {scale}: {w}x{h}"
                );
            }
        }
    }
}

/// Renderer setup errors read well (hosts print them before exiting).
#[test]
fn renderer_errors_are_readable() {
    use trino_platform_pc::RendererError;
    let cases = [
        (
            RendererError::NoAdapter("none".into()),
            "no compatible GPU adapter: none",
        ),
        (
            RendererError::Device("lost".into()),
            "failed to open GPU device: lost",
        ),
        (RendererError::Surface("gone".into()), "surface error: gone"),
    ];
    for (e, text) in cases {
        assert_eq!(e.to_string(), text);
        let boxed: Box<dyn std::error::Error> = Box::new(e);
        assert_eq!(boxed.to_string(), text);
    }
}

/// Rebinding through the platform, as a host drives it: the game asks
/// (`Platform::rebind`), the host hands the request to the input backend,
/// the next key pressed becomes the binding (and only that button keeps
/// it), the labels follow, the bindings survive their text file, and a
/// reset brings the defaults back. Without a published input state the
/// requests are dropped (consoles, tests).
#[test]
fn rebinding_keys_through_the_platform() {
    use trino_core::{BindDevice, Button, GamepadStatus, Input, Platform};
    use trino_platform_pc::input::{InputRequest, bindings_from_text, bindings_to_text};
    use winit::keyboard::KeyCode;

    let mut p = PcPlatform::new(&Caps::PC);
    let mut input = trino_platform_pc::PcInput::new();
    p.rebind(Button::A, BindDevice::Keyboard);
    assert_eq!(p.take_input_request(), None, "no input published yet");

    p.publish_input(&input);
    let info = p.input_info().expect("published");
    assert_eq!(info.capturing, None);
    assert_ne!(
        info.gamepad,
        GamepadStatus::Connected,
        "no pad on a test runner"
    );
    assert_eq!(p.binding_label(Button::A, BindDevice::Keyboard), Some("Z"));
    assert_eq!(
        p.binding_label(Button::L, BindDevice::Gamepad),
        Some("LB/LT")
    );

    p.rebind(Button::A, BindDevice::Keyboard);
    let r = p.take_input_request().expect("a request");
    assert_eq!(r, InputRequest::Rebind(0, Button::A, BindDevice::Keyboard));
    input.apply(r);
    p.publish_input(&input);
    assert_eq!(
        p.input_info().unwrap().capturing,
        Some((Button::A, BindDevice::Keyboard))
    );
    // B's key: it moves to A, B is left without one.
    input.handle_key(KeyCode::KeyX, true);
    assert!(
        !input.poll().is_down(Button::A),
        "the capturing press does nothing"
    );
    input.handle_key(KeyCode::KeyX, false);
    assert!(input.take_changed());
    p.publish_input(&input);
    assert_eq!(p.input_info().unwrap().capturing, None);
    assert_eq!(p.binding_label(Button::A, BindDevice::Keyboard), Some("X"));
    assert_eq!(p.binding_label(Button::B, BindDevice::Keyboard), None);
    input.handle_key(KeyCode::KeyX, true);
    let s = input.poll();
    assert!(s.is_down(Button::A) && !s.is_down(Button::B));
    input.handle_key(KeyCode::KeyX, false);
    input.handle_key(KeyCode::KeyZ, true);
    assert!(!input.poll().is_down(Button::A), "Z no longer presses A");
    input.handle_key(KeyCode::KeyZ, false);

    // The text file round-trips; a broken file keeps the defaults.
    let text = bindings_to_text(input.bindings());
    assert_eq!(&bindings_from_text(&text), input.bindings());
    assert_eq!(
        bindings_from_text("garbage\nA = ??? | Nothing\n"),
        Default::default()
    );

    // A key with a fixed role (W moves the stick) is not taken: the
    // rebind keeps waiting, and the next bindable key gets it.
    input.apply(InputRequest::Rebind(0, Button::Y, BindDevice::Keyboard));
    input.handle_key(KeyCode::KeyW, true);
    input.handle_key(KeyCode::KeyW, false);
    assert_eq!(
        input.info().capturing,
        Some((Button::Y, BindDevice::Keyboard))
    );
    input.handle_key(KeyCode::KeyK, true);
    input.handle_key(KeyCode::KeyK, false);
    assert_eq!(input.info().capturing, None);
    assert_eq!(
        input
            .bindings()
            .label(Button::Y, BindDevice::Keyboard)
            .as_deref(),
        Some("K")
    );
    assert!(input.take_changed());

    // Escape cancels a pending rebind.
    input.apply(InputRequest::Rebind(0, Button::Y, BindDevice::Keyboard));
    input.handle_key(KeyCode::Escape, true);
    assert_eq!(input.info().capturing, None);
    assert!(!input.take_changed());

    p.reset_bindings();
    input.apply(p.take_input_request().unwrap());
    assert!(input.take_changed());
    p.publish_input(&input);
    assert_eq!(p.binding_label(Button::A, BindDevice::Keyboard), Some("Z"));
    assert_eq!(p.binding_label(Button::B, BindDevice::Keyboard), Some("X"));
}

/// `.input` files written before L2/R2/L3/R3 existed (twelve lines) still
/// parse to the same twelve bindings; the four new buttons get their
/// defaults, minus any key or pad button the file moved to another button.
/// New files carry sixteen lines and round-trip.
#[test]
fn old_bindings_files_still_parse() {
    use trino_core::{BindDevice, Button};
    use trino_platform_pc::input::{
        BINDABLE, Bindings, PadButton as P, bindings_from_text, bindings_to_text,
    };
    use winit::keyboard::KeyCode;

    // Exactly what the twelve-button engine wrote for the defaults.
    const OLD_DEFAULT: &str = "A = Z | South
B = X | East
X = C | West
Y = V | North
L = Q | LeftTrigger LeftTrigger2
R = E | RightTrigger RightTrigger2
Start = ENTER | Start Mode
Select = RSHIFT | Select LeftThumb
DpadUp = UP | DPadUp
DpadDown = DOWN | DPadDown
DpadLeft = LEFT | DPadLeft
DpadRight = RIGHT | DPadRight
";
    let d = Bindings::default();
    assert_eq!(bindings_from_text(OLD_DEFAULT), d);
    // The defaults' first twelve lines are still the old text, byte for byte.
    let new_text = bindings_to_text(&d);
    assert!(new_text.starts_with(OLD_DEFAULT), "{new_text}");
    assert_eq!(new_text.lines().count(), BINDABLE.len());
    assert!(new_text.ends_with(
        "L2 = - | LeftTrigger2\nR2 = - | RightTrigger2\nL3 = - | LeftThumb\nR3 = - | RightThumb\n"
    ));

    // A rebound old file: A on K and on LT, Y unbound, Select on the right
    // stick click.
    const OLD_REBOUND: &str = "A = K | LeftTrigger2
B = X | East
X = C | West
Y = - | -
L = Q | LeftTrigger
R = E | RightTrigger RightTrigger2
Start = ENTER | Start Mode
Select = RSHIFT | RightThumb
DpadUp = UP | DPadUp
DpadDown = DOWN | DPadDown
DpadLeft = LEFT | DPadLeft
DpadRight = RIGHT | DPadRight
";
    let b = bindings_from_text(OLD_REBOUND);
    let slot = |button: Button| BINDABLE.iter().position(|x| *x == button).unwrap();
    assert_eq!(b.keys[slot(Button::A)], Some(KeyCode::KeyK));
    assert_eq!(b.pad[slot(Button::A)], [Some(P::LeftTrigger2), None]);
    assert_eq!(b.keys[slot(Button::Y)], None);
    assert_eq!(b.pad[slot(Button::Y)], [None, None]);
    assert_eq!(b.pad[slot(Button::L)], [Some(P::LeftTrigger), None]);
    assert_eq!(b.pad[slot(Button::Select)], [Some(P::RightThumb), None]);
    let changed = [Button::A, Button::Y, Button::L, Button::Select];
    for (i, button) in BINDABLE[..12].iter().enumerate() {
        if !changed.contains(button) {
            assert_eq!((b.keys[i], b.pad[i]), (d.keys[i], d.pad[i]), "{button:?}");
        }
    }
    // LT went to A and RS to Select: L2 and R3 don't get them back. R2
    // keeps RT (R still shares it, as by default); L3 keeps LS (Select
    // gave it up, nobody took it).
    assert_eq!(b.pad[slot(Button::L2)], [None, None]);
    assert_eq!(b.pad[slot(Button::R2)], [Some(P::RightTrigger2), None]);
    assert_eq!(b.pad[slot(Button::L3)], [Some(P::LeftThumb), None]);
    assert_eq!(b.pad[slot(Button::R3)], [None, None]);
    assert_eq!(b.label(Button::L2, BindDevice::Keyboard), None);

    // New buttons and new keys bind and persist.
    let mut b = d;
    b.bind_key(Button::L2, KeyCode::Backquote);
    b.bind_key(Button::R3, KeyCode::KeyF);
    b.bind_key(Button::Start, KeyCode::F12);
    b.bind_pad(Button::L3, P::North);
    let text = bindings_to_text(&b);
    assert!(text.contains("L2 = ` | LeftTrigger2\n"), "{text}");
    assert!(text.contains("R3 = F | RightThumb\n"), "{text}");
    assert!(text.contains("Start = F12 | Start Mode\n"), "{text}");
    assert!(text.contains("L3 = - | North\n"), "{text}");
    assert!(text.contains("Y = V | -\n"), "North left Y: {text}");
    assert_eq!(bindings_from_text(&text), b);
    assert_eq!(
        b.label(Button::L2, BindDevice::Keyboard).as_deref(),
        Some("`")
    );
    assert_eq!(
        b.label(Button::L3, BindDevice::Gamepad).as_deref(),
        Some("Y")
    );
}

/// A pad rebind takes the next pad button (here fed directly: no pad on a
/// test runner); the pad button leaves the button that had it.
#[test]
fn rebinding_a_pad_button() {
    use trino_core::{BindDevice, Button};
    use trino_platform_pc::input::{Bindings, PadButton};

    let mut b = Bindings::default();
    b.bind_pad(Button::Y, PadButton::South);
    assert_eq!(
        b.label(Button::Y, BindDevice::Gamepad).as_deref(),
        Some("A")
    );
    assert_eq!(b.label(Button::A, BindDevice::Gamepad), None);
    b.bind_pad(Button::L, PadButton::RightThumb);
    assert_eq!(
        b.label(Button::L, BindDevice::Gamepad).as_deref(),
        Some("RS")
    );

    let mut input = trino_platform_pc::PcInput::new();
    assert!(!input.pad_pressed(PadButton::North), "nothing waits");
}

/// A simulated tier (`TRINO_SIM=n64x|3dsnew`, issue #54): the platform
/// answers `hw_tier` from the caps it was made with, or the host's choice.
#[test]
fn hw_tier_follows_the_simulated_caps() {
    use trino_core::HwTier;
    for (caps, tier) in [
        (Caps::N64, HwTier::Base),
        (Caps::N64_EXPANDED, HwTier::Expanded),
        (Caps::N3DS_NEW, HwTier::Expanded),
        (Caps::PC, HwTier::Base),
    ] {
        assert_eq!(PcPlatform::new(&caps).hw_tier(), tier);
    }
    let (profile, tier) = SimProfile::parse_tier("3dsnew").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut p =
        PcPlatform::new(&profile.caps().with_tier(tier)).with_save_file(dir.path().join("g.sav"));
    assert_eq!(p.hw_tier(), HwTier::Expanded);
    // The tier changes RAM and cores, not the cartridge's or the SD's save.
    assert_eq!(p.save_capacity(), Caps::N3DS.save_bytes as usize);
    p.set_hw_tier(HwTier::Base);
    assert_eq!(p.hw_tier(), HwTier::Base);
}

/// Accessibility (issue #60): the PC platform keeps the player's settings,
/// clamps them, hands a change to the host once to persist, and the text
/// file round-trips and repairs.
#[test]
fn accessibility_settings_through_the_platform_and_the_file() {
    use trino_core::{AccessSettings, CaptionMode, CvdMode, Platform};
    use trino_platform_pc::system::{access_settings_from_text, access_settings_to_text};
    let mut p = trino_platform_pc::PcPlatform::new(&trino_core::Caps::PC);
    assert_eq!(p.access(), AccessSettings::DEFAULT);
    assert_eq!(p.take_access_change(), None);
    // The host's saved file: no change to persist.
    let saved = AccessSettings {
        cvd: CvdMode::Deuteranopia,
        ..AccessSettings::DEFAULT
    };
    p.load_access(saved);
    assert_eq!(p.access(), saved);
    assert_eq!(p.take_access_change(), None);
    // The game's change: applied at once, persisted once, clamped.
    let wanted = AccessSettings {
        text_scale: 240,
        captions: CaptionMode::Important,
        ..saved
    };
    p.set_access(wanted);
    let got = p.access();
    assert_eq!(got.text_scale, 200);
    assert_eq!(got.captions, CaptionMode::Important);
    assert_eq!(p.take_access_change(), Some(got));
    assert_eq!(p.take_access_change(), None);
    p.set_access(got);
    assert_eq!(
        p.take_access_change(),
        None,
        "the same settings change nothing"
    );
    // The file.
    let text = access_settings_to_text(&got);
    assert!(
        text.starts_with('#') && text.contains("cvd=deuteranopia"),
        "{text}"
    );
    assert_eq!(access_settings_from_text(&text), got);
    assert_eq!(
        access_settings_from_text("cvd=?\ntext_scale=150\nnoise"),
        AccessSettings {
            text_scale: 150,
            ..AccessSettings::DEFAULT
        }
    );
}

/// The connected pad's family reaches the game through the platform
/// (issue #86): the host copies the pad layer's name and USB ids into the
/// input backend (`note_pad`, what the gilrs glue does every poll), then
/// publishes it with the rest of the input state.
#[test]
fn the_pads_family_reaches_the_game() {
    use trino_core::{Button, PadFamily, PadSlot, Platform};
    use trino_platform_pc::input::PadIdent;

    let mut p = PcPlatform::new(&Caps::PC);
    let mut input = trino_platform_pc::PcInput::new();
    assert_eq!(p.gamepad_family(), None, "nothing published");
    p.publish_input(&input);
    assert_eq!(p.gamepad_family(), None, "no pad");
    for (name, usb, family) in [
        (
            "DualSense Wireless Controller",
            Some((0x054c, 0x0ce6)),
            PadFamily::PlayStation,
        ),
        (
            "Nintendo Switch Pro Controller",
            Some((0x057e, 0x2009)),
            PadFamily::Nintendo,
        ),
        ("Xbox Wireless Controller", None, PadFamily::Xbox),
        (
            "Generic USB Joystick",
            Some((0x0079, 0x0006)),
            PadFamily::Generic,
        ),
    ] {
        input.note_pad(Some(PadIdent {
            name: name.into(),
            vendor: usb.map(|u: (u16, u16)| u.0),
            product: usb.map(|u: (u16, u16)| u.1),
        }));
        p.publish_input(&input);
        assert_eq!(p.gamepad_family(), Some(family), "{name}");
        assert_eq!(p.gamepad_name(), Some(name));
    }
    // What a menu prints for A on the last pad family it saw: a Switch
    // Pro's bottom button is labelled B.
    input.note_pad(Some(PadIdent {
        name: "Pro Controller".into(),
        vendor: Some(0x057e),
        product: Some(0x2009),
    }));
    p.publish_input(&input);
    let f = p.gamepad_family().unwrap();
    assert_eq!(f.button_glyph(Button::A).short, "B");
    assert_eq!(f.glyph(PadSlot::standard(Button::Start)).short, "+");
    input.note_pad(None);
    p.publish_input(&input);
    assert_eq!(p.gamepad_family(), None, "unplugged");
}

/// The PC names itself in a trace (issue #62), whatever console look it
/// simulates: a trace of the N64 profile is still a PC trace.
#[test]
fn the_trace_names_the_pc_whatever_profile_it_simulates() {
    for caps in [Caps::PC, Caps::N64, Caps::N3DS] {
        let p = PcPlatform::new(&caps);
        assert_eq!(
            trino_core::Platform::trace_platform(&p),
            trino_core::trace::TracePlatform::Pc
        );
    }
}
