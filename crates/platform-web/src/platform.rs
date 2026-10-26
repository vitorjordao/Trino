//! [`trino_core::Platform`] in a browser: `performance.now()`, the console,
//! blobs from the asset pack, asset groups downloaded beside it
//! (`crate::groups`, issue #56) and a save in `localStorage`.

use std::collections::HashMap;

use trino_core::mem::HeapStats;
use trino_core::{
    BindDevice, BlobId, Button, Caps, DisplayInfo, DisplaySettings, GamepadStatus, GroupId,
    InputInfo, LoadState, MemoryReport, Platform, QualityInfo, QualityPreset, QualitySettings,
    SaveError,
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
    pub(crate) pad_family: Option<trino_core::PadFamily>,
    /// The quality ([`Platform::quality`] and its siblings), published by
    /// the host (`crate::quality`).
    pub(crate) quality: HostedQuality,
    pub(crate) groups: crate::groups::Groups,
    /// The player's accessibility settings (`localStorage`
    /// `trino-access:<name>`, loaded and stored by the runtime).
    pub(crate) access: trino_core::HostedAccess,
    /// Achievements and stats (`localStorage` `trino-services:<name>`,
    /// loaded and stored by the runtime; issue #61).
    pub(crate) services: trino_core::HostedServices,
    /// The local players (issue #67), published by the host each frame.
    pub players: trino_core::HostedPlayers,
    /// `?lang=`, else `navigator.language` (`crate::language`).
    language: Option<trino_core::LangTag>,
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
            pad_family: None,
            quality: HostedQuality::new(caps),
            groups: crate::groups::Groups::new(caps.ram_bytes),
            access: trino_core::HostedAccess::new(),
            services: trino_core::HostedServices::new(trino_core::ServiceBackend::Local),
            players: trino_core::HostedPlayers::new(),
            language: crate::language::detect(),
        }
    }

    /// Replace the system language (a host that knows better).
    pub fn set_system_language(&mut self, language: Option<trino_core::LangTag>) {
        self.language = language;
    }

    /// Count the page's heap in `Platform::memory` (the app's
    /// `#[global_allocator]`, a `trino_core::mem::Counting`).
    pub fn set_heap(&mut self, heap: Option<&'static dyn HeapStats>) {
        self.groups.heap = heap;
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

    fn trace_platform(&self) -> trino_core::trace::TracePlatform {
        trino_core::trace::TracePlatform::Web
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

    fn load_group(&mut self, group: GroupId) -> LoadState {
        let state = self.groups.table.request_load(group);
        if matches!(state, LoadState::Loading { .. }) {
            self.groups.fetch(group);
        }
        state
    }

    fn unload_group(&mut self, group: GroupId) {
        self.groups.table.request_unload(group);
    }

    fn group_state(&self, group: GroupId) -> LoadState {
        self.groups.table.state(group)
    }

    fn group_blob(&self, id: BlobId) -> Option<&[u8]> {
        match self.groups.blobs.get(&id.0) {
            Some(b) => Some(b),
            None => self.blob(id),
        }
    }

    fn memory(&self) -> MemoryReport {
        self.groups.report()
    }

    fn system_language(&self) -> Option<trino_core::LangTag> {
        self.language
    }

    fn display(&self) -> Option<DisplayInfo> {
        self.display
    }

    fn set_display(&mut self, settings: DisplaySettings) {
        self.display_request = Some(settings);
    }

    fn access(&self) -> trino_core::AccessSettings {
        self.access.get()
    }

    fn services(&self) -> Option<&trino_core::HostedServices> {
        Some(&self.services)
    }

    fn services_mut(&mut self) -> Option<&mut trino_core::HostedServices> {
        Some(&mut self.services)
    }

    fn hosted_players(&self) -> Option<&trino_core::HostedPlayers> {
        Some(&self.players)
    }

    fn hosted_players_mut(&mut self) -> Option<&mut trino_core::HostedPlayers> {
        Some(&mut self.players)
    }

    fn set_access(&mut self, settings: trino_core::AccessSettings) {
        self.access.set(settings);
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

    fn gamepad_family(&self) -> Option<trino_core::PadFamily> {
        self.pad_family
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
