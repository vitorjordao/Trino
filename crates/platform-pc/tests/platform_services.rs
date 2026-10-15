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

    // A framed save, like the consoles keep: header ‖ data.
    let data = b"level=4;coins=17";
    let mut framed = save::header(data).to_vec();
    framed.extend_from_slice(data);
    p.save_write(&framed).unwrap();
    assert!(path.exists(), "parent directories are created");
    assert!(
        !path.with_extension("tmp").exists(),
        "atomic write cleans up"
    );
    let n = p.save_read(&mut buf).unwrap();
    assert_eq!(n, HEADER + data.len());
    let header: [u8; HEADER] = buf[..HEADER].try_into().unwrap();
    assert_eq!(save::data_len(&header), Some(data.len()));
    assert!(save::verify(&header, &buf[HEADER..n]));

    // A torn write (one flipped bit on disk) no longer verifies.
    let mut torn = std::fs::read(&path).unwrap();
    torn[HEADER + 2] ^= 0x10;
    std::fs::write(&path, &torn).unwrap();
    let n = p.save_read(&mut buf).unwrap();
    assert!(
        !save::verify(&header, &buf[HEADER..n]),
        "torn save = no save"
    );

    // A short buffer gets a prefix; the full length is reported.
    let mut small = [0u8; 4];
    assert_eq!(p.save_read(&mut small), Ok(n));
    assert_eq!(&small, &torn[..4]);

    // Over the N64's 16 Kbit EEPROM: rejected, the old save stays.
    assert_eq!(
        p.save_write(&[1u8; 2049]),
        Err(SaveError::TooLarge {
            len: 2049,
            capacity: 2048
        })
    );
    assert_eq!(std::fs::read(&path).unwrap(), torn);
    p.save_write(&[7u8; 2048])
        .expect("exactly the capacity fits");

    // A directory where the file should be: I/O errors, not panics.
    let blocked = dir.path().join("blocked");
    std::fs::create_dir_all(blocked.join("game.sav")).unwrap();
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
