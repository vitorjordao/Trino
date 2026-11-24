//! [`trino_core::Platform`] in a browser: `performance.now()`, the console,
//! blobs from the asset pack, asset groups downloaded beside it
//! (`crate::groups`, issue #56) and a save in `localStorage`.

use std::collections::HashMap;
use trino_core::save;

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
    save_status: trino_core::SaveStatus,
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
    /// `?unix=<seconds>`: a wall clock that starts where a test says.
    pub(crate) clock_pin: Option<trino_core::PinnedClock>,
    /// The game's key rebinds, applied by the runtime before the next poll
    /// (Bongo-Kart #183).
    pub(crate) key_requests: Vec<KeyRequest>,
    /// The keyboard's bindings as the runtime last published them, and the
    /// button a rebind waits for.
    pub(crate) key_labels: [Option<&'static str>; 16],
    pub(crate) capturing: Option<Button>,
    /// What each player's motors were asked to play (Bongo-Kart #180) and
    /// what the pads can do, published by the runtime.
    pub(crate) rumble: trino_core::HostedRumble,
    pub(crate) rumble_support: [trino_core::RumbleSupport; trino_core::MAX_PLAYERS],
}

/// A rebind the game asked for ([`Platform::rebind`], [`Platform::rebind_key`],
/// …), for the runtime to apply to the keyboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyRequest {
    /// `button` takes the next bindable key.
    Capture(Button),
    /// `button` is this key `code` from now on.
    Bind(Button, &'static str),
    Cancel,
    Reset,
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
            save_status: trino_core::SaveStatus::Unknown,
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
            clock_pin: None,
            key_requests: Vec::new(),
            key_labels: crate::keys::WebBindings::default().labels(),
            capturing: None,
            rumble: trino_core::HostedRumble::new(),
            rumble_support: [trino_core::RumbleSupport::Unsupported; trino_core::MAX_PLAYERS],
        }
    }

    /// The wall clock reads `secs` now and runs from there (`?unix=`).
    pub fn pin_clock(&mut self, secs: u64) {
        self.clock_pin = Some(trino_core::PinnedClock::new(secs, self.now_micros()));
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
        let mut kv = LocalStorage(Self::storage()?);
        let mut scratch = vec![0u8; save::slot_file_len(self.save_capacity)];
        let mut slots = crate::saves::Slots {
            kv: &mut kv,
            key: &self.save_key,
        };
        let r = save::read_slot_files(&mut slots, buf, &mut scratch)?;
        self.save_status = r.status;
        Ok(r.len)
    }

    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        if data.len() > self.save_capacity {
            return Err(SaveError::TooLarge {
                len: data.len(),
                capacity: self.save_capacity,
            });
        }
        let mut kv = LocalStorage(Self::storage()?);
        let mut scratch = vec![0u8; save::slot_file_len(self.save_capacity)];
        let mut file = vec![0u8; save::slot_file_len(data.len())];
        let mut slots = crate::saves::Slots {
            kv: &mut kv,
            key: &self.save_key,
        };
        save::write_slot_files(&mut slots, data, &mut scratch, &mut file)?;
        Ok(())
    }

    fn save_status(&self) -> trino_core::SaveStatus {
        self.save_status
    }

    fn save_fault(&mut self, fault: trino_core::SaveFault) -> Result<(), SaveError> {
        let mut kv = LocalStorage(Self::storage()?);
        let mut scratch = vec![0u8; save::slot_file_len(self.save_capacity)];
        let mut slots = crate::saves::Slots {
            kv: &mut kv,
            key: &self.save_key,
        };
        match save::slot_files_fault(&mut slots, fault, &mut scratch)? {
            true => Ok(()),
            false => Err(SaveError::Io),
        }
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
        let info = self.input.unwrap_or(InputInfo {
            gamepad: GamepadStatus::Disconnected,
            capturing: None,
        });
        Some(InputInfo {
            capturing: self.capturing.map(|b| (b, BindDevice::Keyboard)),
            ..info
        })
    }

    fn unix_seconds(&self) -> Option<u64> {
        if let Some(pin) = &self.clock_pin {
            return Some(pin.read(self.now_micros()));
        }
        let ms = js_sys::Date::now();
        (ms.is_finite() && ms > 0.0).then(|| (ms / 1000.0) as u64)
    }

    fn rumble(&mut self, player: u8, low: f32, high: f32, ms: u32) {
        if self.rumble_support(player) == trino_core::RumbleSupport::Unsupported {
            return;
        }
        let scale = self.access.get().rumble_factor();
        let now = self.now_micros();
        self.rumble.request(player, low, high, ms, now, scale);
    }

    fn rumble_state(&self, player: u8) -> trino_core::Rumble {
        self.rumble.state(player, self.now_micros())
    }

    fn rumble_support(&self, player: u8) -> trino_core::RumbleSupport {
        self.rumble_support
            .get(player as usize)
            .copied()
            .unwrap_or_default()
    }

    fn rebind_player(&mut self, _player: u8, button: Button, device: BindDevice) {
        // One keyboard (whoever's seat it is); pads keep the standard
        // mapping in a browser.
        if device == BindDevice::Keyboard {
            self.key_requests.push(KeyRequest::Capture(button));
        }
    }

    fn rebind_key(&mut self, player: u8, button: Button, key: trino_core::Key) -> bool {
        match crate::keys::code_for(key) {
            Some(code) if (player as usize) < trino_core::MAX_PLAYERS => {
                self.key_requests.push(KeyRequest::Bind(button, code));
                true
            }
            _ => false,
        }
    }

    fn cancel_rebind(&mut self) {
        self.key_requests.push(KeyRequest::Cancel);
    }

    fn reset_bindings(&mut self) {
        self.key_requests.push(KeyRequest::Reset);
    }

    fn save_image(&mut self, name: &str, image: &trino_core::RgbaImage) -> Result<(), SaveError> {
        if !trino_core::image::image_name_ok(name) {
            return Err(SaveError::Io);
        }
        let png = crate::image::encode_png(image).ok_or(SaveError::Io)?;
        crate::image::download(&format!("{name}.png"), &png)
    }

    fn gamepad_name(&self) -> Option<&str> {
        self.pad_name.as_deref()
    }

    fn gamepad_family(&self) -> Option<trino_core::PadFamily> {
        self.pad_family
    }

    fn binding_label_for(&self, _player: u8, button: Button, device: BindDevice) -> Option<&str> {
        // The keyboard's bindings as the runtime published them (one
        // keyboard: every player's label is the same); pads keep the
        // standard mapping's names.
        if device == BindDevice::Keyboard {
            return self.key_labels[button as usize];
        }
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
        Some(pad)
    }
}

/// `localStorage` as the save slots' string store.
struct LocalStorage(web_sys::Storage);

impl crate::saves::KeyValue for LocalStorage {
    fn get(&self, key: &str) -> Result<Option<String>, SaveError> {
        self.0.get_item(key).map_err(|_| SaveError::Io)
    }
    fn set(&mut self, key: &str, value: &str) -> Result<(), SaveError> {
        // One `setItem`: the browser replaces the value atomically (a
        // quota error leaves the old slot).
        self.0.set_item(key, value).map_err(|_| SaveError::Io)
    }
}
