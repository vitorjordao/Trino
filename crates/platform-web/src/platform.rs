//! [`trino_core::Platform`] in a browser: `performance.now()`, the console,
//! blobs from the asset pack and a save in `localStorage`.

use std::collections::HashMap;

use trino_core::{
    BindDevice, BlobId, Button, Caps, DisplayInfo, DisplaySettings, GamepadStatus, InputInfo,
    Platform, QualityInfo, QualityPreset, QualitySettings, SaveError,
};
use trino_render_wgpu::quality::HostedQuality;

pub struct WebPlatform {
    performance: Option<web_sys::Performance>,
    /// Leaked on insert: blobs are `'static` (see `Platform::blob`).
    blobs: HashMap<u32, &'static [u8]>,
    /// The `localStorage` key of the save (`trino-save:<name>`).
    save_key: String,
    save_capacity: usize,
    pub(crate) display: Option<DisplayInfo>,
    pub(crate) display_request: Option<DisplaySettings>,
    pub(crate) input: Option<InputInfo>,
    pub(crate) pad_name: Option<String>,
    /// The quality ([`Platform::quality`] and its siblings), published by
    /// the host (`crate::quality`).
    pub(crate) quality: HostedQuality,
}

impl WebPlatform {
    /// Saves under `save_name` in this origin's `localStorage`, capped at
    /// the profile's save budget.
    pub fn new(caps: &Caps, save_name: &str) -> Self {
        WebPlatform {
            performance: web_sys::window().and_then(|w| w.performance()),
            blobs: HashMap::new(),
            save_key: format!("trino-save:{save_name}"),
            save_capacity: caps.save_bytes as usize,
            display: None,
            display_request: None,
            input: None,
            pad_name: None,
            quality: HostedQuality::new(caps),
        }
    }

    /// Register (or replace) a blob.
    pub fn insert_blob(&mut self, id: BlobId, bytes: Vec<u8>) {
        self.blobs.insert(id.0, Box::leak(bytes.into_boxed_slice()));
    }

    /// The fullscreen or window change the game asked for since the last
    /// call ([`Platform::set_display`]).
    pub fn take_display_request(&mut self) -> Option<DisplaySettings> {
        self.display_request.take()
    }

    fn storage() -> Result<web_sys::Storage, SaveError> {
        // Private windows and blocked site data have no storage (or a
        // throwing one): saves are unsupported there, never a crash.
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .ok_or(SaveError::Unsupported)
    }
}

impl Platform for WebPlatform {
    fn now_micros(&self) -> u64 {
        self.performance
            .as_ref()
            .map_or(0, |p| (p.now() * 1000.0) as u64)
    }

    fn log(&self, message: &str) {
        crate::js::log(message);
    }

    fn blob(&self, id: BlobId) -> Option<&'static [u8]> {
        self.blobs.get(&id.0).copied()
    }

    fn save_capacity(&self) -> usize {
        match Self::storage() {
            Ok(_) => self.save_capacity,
            Err(_) => 0,
        }
    }

    fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, SaveError> {
        let storage = Self::storage()?;
        let Some(text) = storage
            .get_item(&self.save_key)
            .map_err(|_| SaveError::Io)?
        else {
            return Ok(0);
        };
        // A damaged value reads as no save (the game's own framing would
        // reject it anyway).
        let Some(bytes) = crate::b64::decode(&text) else {
            return Ok(0);
        };
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(bytes.len())
    }

    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        if data.len() > self.save_capacity {
            return Err(SaveError::TooLarge {
                len: data.len(),
                capacity: self.save_capacity,
            });
        }
        let storage = Self::storage()?;
        // One `setItem`: the browser replaces the value atomically (a
        // quota error leaves the old save).
        storage
            .set_item(&self.save_key, &crate::b64::encode(data))
            .map_err(|_| SaveError::Io)
    }

    fn display(&self) -> Option<DisplayInfo> {
        self.display
    }

    fn set_display(&mut self, settings: DisplaySettings) {
        self.display_request = Some(settings);
    }

    fn quality(&self) -> QualitySettings {
        self.quality.settings()
    }

    fn set_quality(&mut self, settings: QualitySettings) {
        self.quality.request(settings);
    }

    fn quality_preset(&self, preset: QualityPreset) -> Option<QualitySettings> {
        self.quality.preset(preset)
    }

    fn quality_info(&self) -> Option<QualityInfo> {
        self.quality.info()
    }

    fn detect_quality(&mut self) {
        self.quality.detect();
    }

    fn input_info(&self) -> Option<InputInfo> {
        self.input.or(Some(InputInfo {
            gamepad: GamepadStatus::Disconnected,
            capturing: None,
        }))
    }

    fn gamepad_name(&self) -> Option<&str> {
        self.pad_name.as_deref()
    }

    fn binding_label(&self, button: Button, device: BindDevice) -> Option<&str> {
        // The PC's default bindings (`crate::keys`); the Web has no
        // rebinding screen storage yet, so these are fixed.
        let key = match button {
            Button::A => "Z",
            Button::B => "X",
            Button::X => "C",
            Button::Y => "V",
            Button::L => "Q",
            Button::R => "E",
            Button::Start => "ENTER",
            Button::Select => "RSHIFT",
            Button::DpadUp => "UP",
            Button::DpadDown => "DOWN",
            Button::DpadLeft => "LEFT",
            Button::DpadRight => "RIGHT",
            _ => return None,
        };
        let pad = match button {
            Button::A => "A",
            Button::B => "B",
            Button::X => "X",
            Button::Y => "Y",
            Button::L => "LB/LT",
            Button::R => "RB/RT",
            Button::Start => "Start",
            Button::Select => "Back",
            Button::DpadUp => "D-Up",
            Button::DpadDown => "D-Down",
            Button::DpadLeft => "D-Left",
            Button::DpadRight => "D-Right",
            _ => return None,
        };
        Some(match device {
            BindDevice::Keyboard => key,
            BindDevice::Gamepad => pad,
        })
    }
}
