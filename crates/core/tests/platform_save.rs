//! Platform services end to end: the trait defaults, baked blobs, and save
//! data framed like the console backends (`core::save`: `"TS" len fnv1a`,
//! data first and header last) — round trip, torn writes, blank and
//! foreign media, capacity.

mod common;

use common::MemoryPlatform;
use trino_core::save::{self, HEADER};
use trino_core::{BlobId, DisplayInfo, DisplaySettings, Platform, SaveError};

/// A platform with no services at all: clock and log only.
struct Bare;

impl Platform for Bare {
    fn now_micros(&self) -> u64 {
        42
    }
    fn log(&self, _message: &str) {}
}

#[test]
fn platform_defaults_report_no_services() {
    let mut p = Bare;
    let p: &mut dyn Platform = &mut p;
    assert_eq!(p.now_micros(), 42);
    p.log("ignored");
    assert_eq!(p.blob(BlobId::from_path("blobs/level")), None);
    assert_eq!(p.save_capacity(), 0);
    let mut buf = [0u8; 4];
    assert_eq!(p.save_read(&mut buf), Err(SaveError::Unsupported));
    assert_eq!(p.save_write(b"x"), Err(SaveError::Unsupported));
    assert_eq!(p.second_screen(), None);
    // Base hardware unless a backend detects an expanded tier (ADR 0020).
    assert_eq!(p.hw_tier(), trino_core::HwTier::Base);
    assert_eq!(p.display(), None);
    p.set_display(DisplaySettings {
        fullscreen: false,
        ..DisplaySettings::DEFAULT
    });
    assert_eq!(
        p.display(),
        None,
        "set_display is a no-op without a display"
    );
    // No remapping on a platform that does not offer it.
    assert_eq!(p.input_info(), None);
    assert_eq!(p.gamepad_name(), None);
    p.rebind(trino_core::Button::A, trino_core::BindDevice::Keyboard);
    p.cancel_rebind();
    p.reset_bindings();
    assert_eq!(
        p.binding_label(trino_core::Button::A, trino_core::BindDevice::Keyboard),
        None
    );
    assert_eq!(p.input_info(), None);
    // A fixed Low quality, nothing scaled, and no way to change it (#55).
    use trino_core::{QualityPreset, QualitySettings};
    assert_eq!(p.quality(), QualitySettings::fixed(QualityPreset::Low));
    p.set_quality(QualitySettings::ULTRA);
    p.detect_quality();
    assert_eq!(p.quality(), QualitySettings::fixed(QualityPreset::Low));
    assert_eq!(p.quality_preset(QualityPreset::High), None);
    assert_eq!(p.quality_info(), None);
}

#[test]
fn display_settings_default_is_auto_fullscreen() {
    let d = DisplaySettings::default();
    assert_eq!(d, DisplaySettings::DEFAULT);
    assert!(d.fullscreen);
    assert_eq!(
        (d.window_w, d.window_h, d.pixel_scale, d.ui_scale),
        (0, 0, 0, 0)
    );
    let info = DisplayInfo {
        settings: d,
        output: (1920, 1080),
        monitor: (1920, 1080),
        screen: (640, 360),
        pixel_scale: 3,
    };
    assert_eq!(info.settings, d);
    assert_eq!(info.screen.0 * info.pixel_scale as u16, info.output.0);
}

#[test]
fn blobs_are_served_by_logical_handle() {
    let mut p = MemoryPlatform::new(64);
    let level = BlobId::from_path("blobs/level1");
    p.insert_blob(level, b"#..#\n#..#\n".to_vec());
    let dynp: &dyn Platform = &p;
    assert_eq!(dynp.blob(level), Some(&b"#..#\n#..#\n"[..]));
    assert_eq!(dynp.blob(BlobId::from_path("blobs/level2")), None);
    assert_eq!(level.0, trino_core::asset_id("blobs/level1"));
    dynp.log("loaded level1");
    assert_eq!(p.log.borrow().as_slice(), ["loaded level1"]);
}

#[test]
fn save_round_trips_through_the_framed_medium() {
    // A 512-byte EEPROM (the smaller N64 cartridge size).
    let mut p = MemoryPlatform::new(512);
    let dynp: &mut dyn Platform = &mut p;
    assert_eq!(dynp.save_capacity(), 512 - HEADER);
    let mut buf = [0u8; 64];
    assert_eq!(dynp.save_read(&mut buf), Ok(0), "blank medium: no save yet");

    dynp.save_write(b"stars=3;coins=42").unwrap();
    assert_eq!(dynp.save_read(&mut buf), Ok(16));
    assert_eq!(&buf[..16], b"stars=3;coins=42");

    // The medium holds header ‖ data, as documented.
    let header: [u8; HEADER] = p.medium[..HEADER].try_into().unwrap();
    assert_eq!(&header[..2], b"TS");
    assert_eq!(save::data_len(&header), Some(16));
    assert!(save::verify(&header, &p.medium[HEADER..HEADER + 16]));
    assert_eq!(header, save::header(b"stars=3;coins=42"));

    // A shorter buffer gets a prefix but learns the full length.
    let mut small = [0u8; 5];
    assert_eq!(p.save_read(&mut small), Ok(16));
    assert_eq!(&small, b"stars");

    // Overwrite with a shorter save: only the new data reads back.
    p.save_write(b"v2").unwrap();
    assert_eq!(p.save_read(&mut buf), Ok(2));
    assert_eq!(&buf[..2], b"v2");

    // An empty save is a valid save of length 0.
    p.save_write(&[]).unwrap();
    assert_eq!(p.save_read(&mut buf), Ok(0));
    assert!(save::verify(&save::header(&[]), &[]));
}

#[test]
fn torn_and_foreign_media_read_back_as_no_save() {
    let mut p = MemoryPlatform::new(256);
    let mut buf = [0u8; 64];
    p.save_write(b"good save").unwrap();

    // Power cut in the middle of the next write: the data half-landed, the
    // header still describes the old save -> checksum mismatch -> no save.
    p.cut_after = Some(4);
    assert_eq!(p.save_write(b"newer save"), Err(SaveError::Io));
    assert_eq!(p.save_read(&mut buf), Ok(0), "torn write reads as no save");

    // A good write afterwards recovers.
    p.save_write(b"recovered").unwrap();
    assert_eq!(p.save_read(&mut buf), Ok(9));

    // A flipped bit in the data.
    p.medium[HEADER + 2] ^= 0x01;
    assert_eq!(p.save_read(&mut buf), Ok(0));

    // A foreign medium (another game's data, no "TS" magic).
    p.medium[..HEADER].copy_from_slice(b"XYZW\0\0\0\0");
    assert_eq!(p.save_read(&mut buf), Ok(0));

    // A header announcing more data than the medium can hold.
    let mut h = save::header(b"x");
    h[2..4].copy_from_slice(&u16::MAX.to_le_bytes());
    p.medium[..HEADER].copy_from_slice(&h);
    assert_eq!(save::data_len(&h), Some(u16::MAX as usize));
    assert_eq!(p.save_read(&mut buf), Ok(0));
}

#[test]
fn saves_past_capacity_or_without_medium_fail() {
    let mut p = MemoryPlatform::new(64);
    let cap = p.save_capacity();
    assert_eq!(cap, 56);
    let big = vec![7u8; cap + 1];
    assert_eq!(
        p.save_write(&big),
        Err(SaveError::TooLarge {
            len: cap + 1,
            capacity: cap
        })
    );
    // Exactly the capacity fits.
    p.save_write(&big[..cap]).unwrap();
    let mut buf = vec![0u8; cap];
    assert_eq!(p.save_read(&mut buf), Ok(cap));
    assert!(buf.iter().all(|&b| b == 7));

    // A medium too small for the header is "no save support".
    let mut none = MemoryPlatform::new(HEADER);
    assert_eq!(none.save_capacity(), 0);
    assert_eq!(none.save_write(b"a"), Err(SaveError::Unsupported));
    assert_eq!(none.save_read(&mut buf), Err(SaveError::Unsupported));
}

#[test]
fn checksum_is_fnv1a_like_asset_handles() {
    assert_eq!(save::checksum(b""), 0x811c_9dc5);
    assert_eq!(
        save::checksum(b"sprites/hero"),
        trino_core::asset_id("sprites/hero")
    );
    assert_ne!(save::checksum(b"ab"), save::checksum(b"ba"));
    assert_eq!(save::data_len(&[0xFF; HEADER]), None, "blank EEPROM");
}
