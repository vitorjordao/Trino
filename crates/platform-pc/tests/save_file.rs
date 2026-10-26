//! PC saves through the public `Platform` API: round trip, the size limit,
//! the non-blocking `save_begin`/`save_step` pair (immediate on PC), and a
//! crash between the flushed temp file and the rename.

use trino_core::{Caps, Platform, SaveError};
use trino_platform_pc::PcPlatform;

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("trino-save-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn write_read_and_limits() {
    let d = dir("rw");
    let mut p = PcPlatform::new(&Caps::N64).with_save_file(d.join("game.sav"));
    let cap = p.save_capacity();
    assert!(cap > 0);
    let mut buf = vec![0u8; cap];
    assert_eq!(p.save_read(&mut buf), Ok(0), "no save yet");
    p.save_write(b"day=3").unwrap();
    assert_eq!(p.save_read(&mut buf), Ok(5));
    assert_eq!(&buf[..5], b"day=3");
    assert!(matches!(
        p.save_write(&vec![0u8; cap + 1]),
        Err(SaveError::TooLarge { .. })
    ));
    assert_eq!(
        p.save_read(&mut buf),
        Ok(5),
        "a refused save keeps the old one"
    );
    // Non-blocking API: done after begin on PC, and step reports done.
    p.save_begin(b"day=4").unwrap();
    assert_eq!(p.save_step(1000), Ok(true));
    assert_eq!(p.save_read(&mut buf), Ok(5));
    assert_eq!(&buf[..5], b"day=4");
    assert!(!d.join("game.tmp").exists(), "temp file renamed away");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_crash_before_the_rename_keeps_the_new_save() {
    let d = dir("crash");
    std::fs::create_dir_all(&d).unwrap();
    // What a crash between remove/rename leaves: only the temp file.
    std::fs::write(d.join("game.tmp"), b"day=9").unwrap();
    let mut p = PcPlatform::new(&Caps::N64).with_save_file(d.join("game.sav"));
    let mut buf = [0u8; 16];
    assert_eq!(p.save_read(&mut buf), Ok(5));
    assert_eq!(&buf[..5], b"day=9");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn no_save_file_configured_is_unsupported() {
    let mut p = PcPlatform::new(&Caps::N64);
    let mut buf = [0u8; 4];
    assert_eq!(p.save_read(&mut buf), Err(SaveError::Unsupported));
    assert_eq!(p.save_write(b"x"), Err(SaveError::Unsupported));
}

/// Issue #68: saves alternate between `<save>.a` and `<save>.b`, a damaged
/// newest falls back to the previous one (and says so), the save path from
/// before slot files still reads, and a crash between a slot's flushed
/// temp file and its rename keeps the new slot.
#[test]
fn slots_alternate_recover_and_read_the_old_file() {
    use trino_core::{SaveFault, SaveStatus};
    let d = dir("slots");
    std::fs::create_dir_all(&d).unwrap();
    // A save the PC wrote before slot files: raw bytes at the save path.
    std::fs::write(d.join("game.sav"), b"old=1").unwrap();
    let mut p = PcPlatform::new(&Caps::N64).with_save_file(d.join("game.sav"));
    let mut buf = [0u8; 32];
    assert_eq!(p.save_read(&mut buf), Ok(5));
    assert_eq!(
        (&buf[..5], p.save_status()),
        (&b"old=1"[..], SaveStatus::Ok)
    );
    p.save_write(b"day=1").unwrap();
    p.save_write(b"day=2").unwrap();
    assert!(d.join("game.sav.a").exists() && d.join("game.sav.b").exists());
    assert_eq!(
        std::fs::read(d.join("game.sav")).unwrap(),
        b"old=1",
        "left alone"
    );
    assert_eq!(p.save_read(&mut buf), Ok(5));
    assert_eq!(
        (&buf[..5], p.save_status()),
        (&b"day=2"[..], SaveStatus::Ok)
    );
    p.save_fault(SaveFault::FlipNewest).unwrap();
    assert_eq!(p.save_read(&mut buf), Ok(5));
    assert_eq!(
        (&buf[..5], p.save_status()),
        (&b"day=1"[..], SaveStatus::RecoveredFromBackup)
    );
    p.save_write(b"day=3").unwrap();
    assert_eq!(p.save_read(&mut buf), Ok(5));
    assert_eq!(
        (&buf[..5], p.save_status()),
        (&b"day=3"[..], SaveStatus::Ok)
    );
    // A crash after the temp file was flushed, before the rename.
    let newest = if std::fs::read(d.join("game.sav.a"))
        .unwrap()
        .ends_with(b"day=3")
    {
        "a"
    } else {
        "b"
    };
    let other = if newest == "a" { "b" } else { "a" };
    let slot = d.join(format!("game.sav.{other}"));
    let mut next = vec![0u8; 64];
    let n = trino_core::save::encode_slot(99, b"day=4", &mut next).unwrap();
    std::fs::remove_file(&slot).unwrap();
    std::fs::write(d.join(format!("game.sav.{other}.tmp")), &next[..n]).unwrap();
    assert_eq!(p.save_read(&mut buf), Ok(5));
    assert_eq!(&buf[..5], b"day=4");
    // Everything damaged: the old file is what is left.
    // (The fault rewrites each slot through its temp file and rename.)
    p.save_fault(SaveFault::FlipAll).unwrap();
    assert!(!d.join(format!("game.sav.{other}.tmp")).exists());
    assert_eq!(p.save_read(&mut buf), Ok(5));
    assert_eq!(
        (&buf[..5], p.save_status()),
        (&b"old=1"[..], SaveStatus::RecoveredFromBackup)
    );
    // No save file configured: no fault to inject.
    let mut none = PcPlatform::new(&Caps::N64);
    assert_eq!(
        none.save_fault(SaveFault::FlipNewest),
        Err(SaveError::Unsupported)
    );
    let _ = std::fs::remove_dir_all(&d);
}
