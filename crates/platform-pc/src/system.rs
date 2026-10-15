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

use trino_core::{BlobId, Caps, DisplayInfo, DisplaySettings, Platform, SaveError};

pub struct PcPlatform {
    start: Instant,
    /// Leaked on insert: blobs are `'static` (see `Platform::blob`). A live
    /// reload leaks the previous version — dev-only, bounded by edits.
    blobs: HashMap<u32, &'static [u8]>,
    save_path: Option<PathBuf>,
    save_capacity: usize,
    /// What the host reports ([`Platform::display`]); `None` = fixed screen.
    display: Option<DisplayInfo>,
    /// The game's last [`Platform::set_display`], for the host to apply.
    display_request: Option<DisplaySettings>,
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
            display: None,
            display_request: None,
        }
    }

    /// The host publishes the display state (window, monitor, picture).
    pub fn set_display_info(&mut self, info: Option<DisplayInfo>) {
        self.display = info;
    }

    /// The settings the game asked for since the last call, if any.
    pub fn take_display_request(&mut self) -> Option<DisplaySettings> {
        self.display_request.take()
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

    fn display(&self) -> Option<DisplayInfo> {
        self.display
    }

    fn set_display(&mut self, settings: DisplaySettings) {
        if self.display.is_some() {
            self.display_request = Some(settings);
        }
    }
}

/// Display settings as a small text file (`key=value` lines), so players
/// can read and fix it by hand.
pub fn display_settings_to_text(s: &DisplaySettings) -> String {
    format!(
        "fullscreen={}\nwindow={}x{}\npixel_scale={}\nui_scale={}\n",
        s.fullscreen as u8, s.window_w, s.window_h, s.pixel_scale, s.ui_scale
    )
}

/// Parse [`display_settings_to_text`]; unknown or broken lines keep the
/// default.
pub fn display_settings_from_text(text: &str) -> DisplaySettings {
    let mut s = DisplaySettings::DEFAULT;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "fullscreen" => s.fullscreen = v == "1" || v == "true",
            "window" => {
                if let Some((w, h)) = v.split_once('x')
                    && let (Ok(w), Ok(h)) = (w.parse(), h.parse())
                {
                    s.window_w = w;
                    s.window_h = h;
                }
            }
            "pixel_scale" => s.pixel_scale = v.parse().unwrap_or(0).min(16),
            "ui_scale" => s.ui_scale = v.parse().unwrap_or(0).min(8),
            _ => {}
        }
    }
    s
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
    fn display_settings_round_trip_and_requests() {
        let s = DisplaySettings {
            fullscreen: false,
            window_w: 1600,
            window_h: 900,
            pixel_scale: 2,
            ui_scale: 3,
        };
        assert_eq!(display_settings_from_text(&display_settings_to_text(&s)), s);
        assert_eq!(
            display_settings_from_text("garbage\nwindow=axb\n"),
            DisplaySettings::DEFAULT
        );
        // Requests only exist where the host reports a display.
        let mut p = PcPlatform::new(&Caps::PC);
        p.set_display(s);
        assert_eq!(p.take_display_request(), None);
        p.set_display_info(Some(DisplayInfo {
            settings: DisplaySettings::DEFAULT,
            output: (1920, 1080),
            monitor: (1920, 1080),
            screen: (640, 360),
            pixel_scale: 3,
        }));
        p.set_display(s);
        assert_eq!(p.take_display_request(), Some(s));
        assert_eq!(p.take_display_request(), None);
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
