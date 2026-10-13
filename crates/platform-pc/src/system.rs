//! PC platform services ([`trino_core::Platform`]): clock, stdout log,
//! baked blobs and a save file.
//!
//! The save lives in one file written atomically (temp file + rename) and
//! is capped at the simulated console's [`Caps::save_bytes`] — an N64
//! profile game that saves 3 KB fails on PC exactly like it would on the
//! cartridge EEPROM.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use trino_core::{BlobId, Caps, Platform, SaveError};

pub struct PcPlatform {
    start: Instant,
    /// Leaked on insert: blobs are `'static` (see `Platform::blob`). A live
    /// reload leaks the previous version — dev-only, bounded by edits.
    blobs: HashMap<u32, &'static [u8]>,
    save_path: Option<PathBuf>,
    save_capacity: usize,
}

impl PcPlatform {
    /// No save file (saves report `Unsupported`) until
    /// [`with_save_file`](Self::with_save_file).
    pub fn new(caps: &Caps) -> Self {
        PcPlatform {
            start: Instant::now(),
            blobs: HashMap::new(),
            save_path: None,
            save_capacity: caps.save_bytes as usize,
        }
    }

    /// Persist saves at `path` (created on first write).
    pub fn with_save_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.save_path = Some(path.into());
        self
    }

    /// Register (or replace — live reload) a blob.
    pub fn insert_blob(&mut self, id: BlobId, bytes: Vec<u8>) {
        self.blobs.insert(id.0, Box::leak(bytes.into_boxed_slice()));
    }
}

impl Platform for PcPlatform {
    fn now_micros(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }

    fn log(&self, message: &str) {
        println!("{message}");
    }

    fn blob(&self, id: BlobId) -> Option<&'static [u8]> {
        self.blobs.get(&id.0).copied()
    }

    fn save_capacity(&self) -> usize {
        if self.save_path.is_some() {
            self.save_capacity
        } else {
            0
        }
    }

    fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, SaveError> {
        let path = self.save_path.as_ref().ok_or(SaveError::Unsupported)?;
        match std::fs::read(path) {
            Ok(bytes) => {
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                Ok(bytes.len())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(_) => Err(SaveError::Io),
        }
    }

    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        let path = self.save_path.as_ref().ok_or(SaveError::Unsupported)?;
        if data.len() > self.save_capacity {
            return Err(SaveError::TooLarge {
                len: data.len(),
                capacity: self.save_capacity,
            });
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|_| SaveError::Io)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, data).map_err(|_| SaveError::Io)?;
        std::fs::rename(&tmp, path).map_err(|_| SaveError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_by_handle() {
        let mut p = PcPlatform::new(&Caps::N64);
        let id = BlobId::from_path("blobs/level");
        assert!(p.blob(id).is_none());
        p.insert_blob(id, b"abc".to_vec());
        assert_eq!(p.blob(id), Some(&b"abc"[..]));
    }

    #[test]
    fn save_round_trip_and_budget() {
        let dir = std::env::temp_dir().join(format!("trino-save-{}", std::process::id()));
        let path = dir.join("game.sav");
        let mut p = PcPlatform::new(&Caps::N64).with_save_file(&path);
        assert_eq!(p.save_capacity(), 2048);
        let mut buf = [0u8; 16];
        assert_eq!(p.save_read(&mut buf), Ok(0), "no save yet");
        p.save_write(b"hello").unwrap();
        assert_eq!(p.save_read(&mut buf), Ok(5));
        assert_eq!(&buf[..5], b"hello");
        assert_eq!(
            p.save_write(&[0u8; 4096]),
            Err(SaveError::TooLarge {
                len: 4096,
                capacity: 2048
            })
        );
        let mut none = PcPlatform::new(&Caps::N64);
        assert_eq!(none.save_write(b"x"), Err(SaveError::Unsupported));
        assert_eq!(none.save_capacity(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
