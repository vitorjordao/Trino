//! The save power-cut soak (issue #121): a build that does nothing but save
//! a counter, frame after frame, so the power can be cut in the middle of a
//! write — a SummerCart64's EEPROM pulled during `save_step`, a 3DS's SD
//! card pulled (or the console switched off) during a write — and the next
//! boot says what survived.
//!
//! Each boot logs `TRINO_SOAK boot seq=<n> status=<s>`: the counter it read
//! and `Platform::save_status`. Each finished save logs `TRINO_SOAK wrote
//! seq=<n>`. After a cut, the boot must read the last `wrote` or the one
//! being written (`n` or `n + 1`), never less, never `corrupted`.
//! `cargo xtask save soak` checks that in Azahar by killing it mid-write
//! (`docs/HARDWARE.md` has the hardware procedure). The save is 96 bytes,
//! so an N64 write spans 14 EEPROM blocks (one per frame): a cut lands
//! inside one most of the time.

use trino_core::{Platform, SaveStatus};

/// The save's bytes: a tag, the counter, and padding to stretch the write.
pub const LEN: usize = 96;
const TAG: &[u8; 4] = b"SOAK";

/// The soak's state between frames.
pub struct SaveSoak {
    /// The counter the next save writes.
    next: u32,
    /// A save handed to `save_begin` and not finished yet.
    pending: Option<u32>,
}

/// The counter a soak save holds, if `data` is one.
pub fn counter(data: &[u8]) -> Option<u32> {
    let rest = data.strip_prefix(TAG)?;
    Some(u32::from_le_bytes(rest.get(..4)?.try_into().ok()?))
}

/// A soak save of `seq`: the tag, the counter, then bytes derived from it
/// (a torn write can't look whole).
pub fn encode(seq: u32) -> [u8; LEN] {
    let mut b = [0u8; LEN];
    b[..4].copy_from_slice(TAG);
    b[4..8].copy_from_slice(&seq.to_le_bytes());
    for (i, x) in b[8..].iter_mut().enumerate() {
        *x = (seq as u8).wrapping_mul(31).wrapping_add(i as u8);
    }
    b
}

impl SaveSoak {
    /// Read what survived and log it (`TRINO_SOAK boot …`).
    pub fn boot(platform: &mut dyn Platform) -> SaveSoak {
        let mut buf = [0u8; LEN];
        let n = platform.save_read(&mut buf).unwrap_or(0).min(LEN);
        let status = platform.save_status();
        let seq = counter(&buf[..n]);
        let digits = alloc_free_u32(seq.unwrap_or(0));
        let shown = if seq.is_some() {
            digits.as_str()
        } else {
            "none"
        };
        log_line(platform, "boot seq=", shown, status);
        SaveSoak {
            next: seq.map_or(1, |s| s.wrapping_add(1)),
            pending: None,
        }
    }

    /// One frame: start the next save, or move the pending one on (an
    /// EEPROM block a frame on the N64; the files finish at once).
    pub fn step(&mut self, platform: &mut dyn Platform) {
        if self.pending.is_none() {
            if platform.save_begin(&encode(self.next)).is_err() {
                return;
            }
            self.pending = Some(self.next);
        }
        if platform.save_step(2_000).unwrap_or(false)
            && let Some(seq) = self.pending.take()
        {
            log_line(
                platform,
                "wrote seq=",
                alloc_free_u32(seq).as_str(),
                SaveStatus::Unknown,
            );
            self.next = seq.wrapping_add(1);
        }
    }
}

/// Decimal digits without an allocator (`TextBuf`).
fn alloc_free_u32(v: u32) -> trino_core::TextBuf<12> {
    let mut t = trino_core::TextBuf::<12>::new();
    let _ = core::fmt::Write::write_fmt(&mut t, format_args!("{v}"));
    t
}

fn log_line(platform: &dyn Platform, what: &str, value: &str, status: SaveStatus) {
    let mut line = trino_core::TextBuf::<96>::new();
    let _ = core::fmt::Write::write_fmt(&mut line, format_args!("TRINO_SOAK {what}{value}"));
    if what.starts_with("boot") {
        let _ = core::fmt::Write::write_fmt(&mut line, format_args!(" status={}", status.label()));
    }
    let _ = core::fmt::Write::write_str(&mut line, "\n");
    platform.log(line.as_str());
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::string::String;
    use std::vec::Vec;

    use super::*;

    /// A medium that finishes a write after `steps` steps (an EEPROM).
    #[derive(Default)]
    struct Medium {
        save: Vec<u8>,
        pending: Option<(Vec<u8>, u32)>,
        log: core::cell::RefCell<String>,
    }

    impl Platform for Medium {
        fn now_micros(&self) -> u64 {
            0
        }
        fn log(&self, msg: &str) {
            self.log.borrow_mut().push_str(msg);
        }
        fn save_capacity(&self) -> usize {
            2000
        }
        fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, trino_core::SaveError> {
            buf[..self.save.len()].copy_from_slice(&self.save);
            Ok(self.save.len())
        }
        fn save_write(&mut self, data: &[u8]) -> Result<(), trino_core::SaveError> {
            self.save = data.to_vec();
            Ok(())
        }
        fn save_status(&self) -> SaveStatus {
            SaveStatus::Ok
        }
        fn save_begin(&mut self, data: &[u8]) -> Result<(), trino_core::SaveError> {
            self.pending = Some((data.to_vec(), 3));
            Ok(())
        }
        fn save_step(&mut self, _: u32) -> Result<bool, trino_core::SaveError> {
            let Some((d, left)) = self.pending.as_mut() else {
                return Ok(true);
            };
            *left -= 1;
            if *left == 0 {
                self.save = core::mem::take(d);
                self.pending = None;
                return Ok(true);
            }
            Ok(false)
        }
    }

    #[test]
    fn the_soak_counts_up_and_says_what_a_boot_read() {
        let mut m = Medium::default();
        let mut s = SaveSoak::boot(&mut m);
        for _ in 0..9 {
            s.step(&mut m);
        }
        let log = m.log.borrow().clone();
        assert!(
            log.starts_with("TRINO_SOAK boot seq=none status=ok\n"),
            "{log}"
        );
        assert!(log.contains("TRINO_SOAK wrote seq=3\n"), "{log}");
        assert!(!log.contains("seq=4"), "three steps per save: {log}");
        // A cut now (a save of 4 begun): the next boot reads 3.
        let mut again = Medium {
            save: m.save.clone(),
            ..Medium::default()
        };
        SaveSoak::boot(&mut again);
        assert_eq!(
            again.log.borrow().as_str(),
            "TRINO_SOAK boot seq=3 status=ok\n"
        );
        assert_eq!(counter(&encode(70_000)), Some(70_000));
        assert_eq!(counter(b"trino-save-v1"), None);
    }
}
