//! OS-level services a game may need, abstracted per platform: the clock,
//! debug logging, baked data blobs and save data.
//!
//! Apps hand a `&mut dyn Platform` to [`crate::Game::update_with`] every
//! frame; games that never need these services just implement
//! [`crate::Game::update`].

/// Handle to a baked data blob (`[blobs.*]` in the manifest, logical path
/// `blobs/<name>`; baked scenes are `scenes/<name>`). Bytes are delivered
/// as-is on every target — level data, dialogue tables, baked scenes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BlobId(pub u32);

impl BlobId {
    pub const fn from_path(logical_path: &str) -> Self {
        BlobId(crate::asset::asset_id(logical_path))
    }
}

/// Why a save operation failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SaveError {
    /// The platform has no save medium (or none was detected — e.g. an N64
    /// cartridge without EEPROM).
    Unsupported,
    /// Data larger than [`Platform::save_capacity`].
    TooLarge { len: usize, capacity: usize },
    /// The medium reported an error.
    Io,
}

/// How a PC window shows the game. Consoles have one fixed screen and
/// ignore it ([`Platform::display`] returns `None` there).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DisplaySettings {
    /// Borderless fullscreen on the current monitor (else a window).
    pub fullscreen: bool,
    /// Window client size in physical pixels (0 × 0 = fit the monitor).
    pub window_w: u16,
    pub window_h: u16,
    /// Pixel scale: the game renders at the output size divided by this
    /// (integer, so the upscale stays crisp). 0 = auto (about 300–600
    /// lines, see the backend).
    pub pixel_scale: u8,
    /// Interface scale in quarters: 4 is 100%, 6 is 150%, 8 is 200%; 0 =
    /// auto (100%). The UI system applies it on top of its whole-pixel fit
    /// ([`interface_scale`](Self::interface_scale),
    /// `UiEnv::with_interface_scale`).
    pub ui_scale: u8,
}

impl DisplaySettings {
    /// The first-run default: fullscreen at the monitor's size, auto
    /// scales.
    pub const DEFAULT: DisplaySettings = DisplaySettings {
        fullscreen: true,
        window_w: 0,
        window_h: 0,
        pixel_scale: 0,
        ui_scale: 0,
    };

    /// The interface scale as a factor (`ui_scale` / 4; auto is 1).
    pub fn interface_scale(&self) -> f32 {
        match self.ui_scale {
            0 => 1.0,
            n => n as f32 * 0.25,
        }
    }
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The display as the platform reports it (PC only).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DisplayInfo {
    /// What is in effect (after the last [`Platform::set_display`]).
    pub settings: DisplaySettings,
    /// The window's client area (or the fullscreen output), physical px.
    pub output: (u16, u16),
    /// The monitor the window is on, physical px.
    pub monitor: (u16, u16),
    /// The game's picture: the internal resolution it renders at.
    pub screen: (u16, u16),
    /// The pixel scale in effect (the auto value resolved).
    pub pixel_scale: u8,
}

/// Which bindings a remapping screen edits: the keyboard's or the
/// gamepads'.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BindDevice {
    Keyboard = 0,
    Gamepad = 1,
}

/// The gamepads a PC host can see.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GamepadStatus {
    /// Built without gamepad support, or the system's pad layer would not
    /// start (Linux without udev access, for one): keyboard and mouse only.
    Unsupported,
    /// Supported; none connected.
    Disconnected,
    /// At least one connected.
    Connected,
}

/// Input as a PC host reports it ([`Platform::input_info`]; `None` on
/// consoles, whose buttons are fixed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InputInfo {
    pub gamepad: GamepadStatus,
    /// A rebind waiting for its key or pad button ([`Platform::rebind`]).
    pub capturing: Option<(crate::Button, BindDevice)>,
}

pub trait Platform {
    /// Monotonic time in microseconds since an arbitrary epoch.
    fn now_micros(&self) -> u64;

    /// Debug logging. Routed to stdout on PC, ISViewer on N64, the debug
    /// channel on 3DS.
    fn log(&self, message: &str);

    /// Bytes of a baked blob, or `None` if the handle is unknown. Blobs are
    /// loaded at boot and never freed, hence `'static`: game state can keep
    /// borrowing them (a `BakedScene`, its node names and tags) across
    /// frames. On PC a live reload leaks the previous version (dev only).
    fn blob(&self, _id: BlobId) -> Option<&'static [u8]> {
        None
    }

    /// Bytes available for save data (0 = no save medium). The N64 budget
    /// is the cartridge EEPROM (512 B or 2 KB), so design saves for that.
    fn save_capacity(&self) -> usize {
        0
    }

    /// Read the save into `buf`; returns how many bytes were stored by the
    /// last [`save_write`](Self::save_write) (0 = no save yet). Reads at
    /// most `buf.len()` bytes.
    fn save_read(&mut self, _buf: &mut [u8]) -> Result<usize, SaveError> {
        Err(SaveError::Unsupported)
    }

    /// Replace the save with `data` (atomic where the medium allows).
    fn save_write(&mut self, _data: &[u8]) -> Result<(), SaveError> {
        Err(SaveError::Unsupported)
    }

    /// Start replacing the save with `data` without stalling the frame:
    /// then call [`save_step`](Self::save_step) once per frame until it
    /// returns `Ok(true)`. Until then [`save_read`](Self::save_read) still
    /// returns the previous save, and a power cut keeps it. A new
    /// `save_begin` replaces an unfinished one. The default writes it all
    /// now (files are fast); the N64 spreads its EEPROM blocks (~7.5 ms per
    /// 8 bytes) over frames.
    fn save_begin(&mut self, data: &[u8]) -> Result<(), SaveError> {
        self.save_write(data)
    }

    /// Continue a [`save_begin`](Self::save_begin): spend about `budget_us`
    /// (at least one unit of progress per call). `Ok(true)` once the save
    /// is complete — and also when nothing is pending.
    fn save_step(&mut self, _budget_us: u32) -> Result<bool, SaveError> {
        Ok(true)
    }

    /// The second screen's size (3DS touch screen, or the PC's 3DS profile),
    /// known at update time (the renderer's
    /// [`second_screen`](crate::Renderer::second_screen) is the same).
    fn second_screen(&self) -> Option<(u16, u16)> {
        None
    }

    /// Which variant of the console the game runs on ([`crate::HwTier`],
    /// ADR 0020): `Expanded` on an N64 with the Expansion Pak and on a New
    /// 3DS / New 2DS XL, detected at boot; `Base` everywhere else (and on
    /// the PC unless `TRINO_SIM=n64x|3dsnew` simulates a tier). The
    /// renderer's [`caps`](crate::Renderer::caps) is that tier's table.
    /// Branch on it at update time; content picks its tier variants as data.
    fn hw_tier(&self) -> crate::HwTier {
        crate::HwTier::Base
    }

    /// The language the system is set to ([`crate::i18n`], issue #59): the
    /// OS locale on the PC (`TRINO_LANG` overrides it), `navigator.language`
    /// on the Web, the system setting on the 3DS (a bare language, `pt`,
    /// with no region). `None` on the N64, which has no such setting: the
    /// game's default language, then the player's saved choice
    /// ([`Localizer::pick`](crate::Localizer::pick)).
    fn system_language(&self) -> Option<crate::LangTag> {
        None
    }

    /// The window/monitor/picture sizes and display settings, on platforms
    /// where they can change (PC). `None` on consoles.
    fn display(&self) -> Option<DisplayInfo> {
        None
    }

    /// Ask for new display settings (PC; a no-op elsewhere). The host
    /// applies them after the frame and calls [`crate::Game::resize`] when
    /// the picture's size changes; it also persists them.
    fn set_display(&mut self, _settings: DisplaySettings) {}

    /// The quality settings in effect ([`crate::quality`], issue #55): what
    /// the player chose or the first launch's calibration picked on the PC
    /// and the Web, a fixed preset on the consoles (the N64 and the 3DS
    /// `Low`, the New 3DS `Medium`, nothing scaled:
    /// [`QualitySettings::fixed`](crate::QualitySettings::fixed)). Games
    /// read it instead of branching per platform: `lod_bias` and
    /// `particle_scale` for content, the rest for an options page. The
    /// default is the consoles' `Low`.
    fn quality(&self) -> crate::QualitySettings {
        crate::QualitySettings::fixed(crate::QualityPreset::Low)
    }

    /// Ask for new quality settings (PC and Web; a documented no-op on the
    /// consoles, whose preset is fixed). The host applies them after the
    /// frame (render scale, dynamic resolution, MSAA, AO, anisotropy,
    /// sharpening, frame limit) and persists them.
    fn set_quality(&mut self, _settings: crate::QualitySettings) {}

    /// What a named preset means on this platform: the project's
    /// `assets/quality.ron` over the built-in values on the PC and the Web
    /// (an options page shows and applies these). `None` on the consoles
    /// and for `Custom`.
    fn quality_preset(&self, _preset: crate::QualityPreset) -> Option<crate::QualitySettings> {
        None
    }

    /// The quality as the host sees it (PC and Web): the live render scale,
    /// the calibration's pick, the GPU class, a handheld. `None` on the
    /// consoles.
    fn quality_info(&self) -> Option<crate::QualityInfo> {
        None
    }

    /// Run the calibration again ("Detect again" on an options page): the
    /// host renders its built-in scene at each preset after the frame and
    /// applies the highest that holds the frame rate (PC; a no-op
    /// elsewhere).
    fn detect_quality(&mut self) {}

    /// The player's accessibility settings ([`crate::access`], issue #60):
    /// color vision, text scale, high contrast, captions, reduced motion,
    /// the flash guard, hold-to-toggle. The PC and the Web keep them for the
    /// player (`<save>.access`, `localStorage`); the consoles keep them for
    /// the session (a game stores [`AccessSettings::pack`](crate::AccessSettings::pack)
    /// in its save to keep them longer). The default — a platform that keeps
    /// nothing — is [`AccessSettings::DEFAULT`](crate::AccessSettings::DEFAULT).
    fn access(&self) -> crate::AccessSettings {
        crate::AccessSettings::DEFAULT
    }

    /// Change the accessibility settings. They apply at once (the next
    /// `access()` returns them, clamped); the PC and the Web persist them
    /// after the frame. A documented no-op on a platform that keeps none
    /// (the default).
    fn set_access(&mut self, _settings: crate::AccessSettings) {}

    /// The input bindings' state, on platforms where they can be remapped
    /// (PC). `None` on consoles.
    fn input_info(&self) -> Option<InputInfo> {
        None
    }

    /// The first connected gamepad's name, as the system reports it.
    fn gamepad_name(&self) -> Option<&str> {
        None
    }

    /// The first connected gamepad's family ([`crate::PadFamily`], issue
    /// #86): Xbox, PlayStation, Nintendo, 8BitDo or generic, detected by the
    /// PC from the pad's USB ids and name, by the Web from `Gamepad.id`.
    /// A menu prints its buttons with it ([`crate::PadFamily::button_glyph`]).
    /// `None` without a pad, and on the consoles, whose buttons are fixed
    /// and known to the game (the N64's A/B/C, the 3DS's Nintendo layout).
    fn gamepad_family(&self) -> Option<crate::PadFamily> {
        None
    }

    /// What `button` is bound to on `device` ("Z", "Enter", "LB"…), for a
    /// remapping screen. `None` when unbound or not remappable.
    fn binding_label(&self, _button: crate::Button, _device: BindDevice) -> Option<&str> {
        None
    }

    /// Bind `button` on `device` to the next key or pad button pressed (PC;
    /// a no-op elsewhere). The host applies it after the frame, reports it
    /// in [`input_info`](Self::input_info) while it waits, and persists the
    /// bindings.
    fn rebind(&mut self, _button: crate::Button, _device: BindDevice) {}

    /// Drop a rebind still waiting for its press.
    fn cancel_rebind(&mut self) {}

    /// Back to the default bindings (persisted too).
    fn reset_bindings(&mut self) {}

    /// Start loading an asset group, or poll one ([`crate::group`], issue
    /// #56): never blocks. The first call on an unloaded group queues it and
    /// returns `Loading { progress: 0 }`; the backend then reads it a time
    /// slice per frame ([`crate::Caps::load_budget_us`]) and later calls
    /// return its progress, then `Ready`. `boot` is always ready; a group
    /// this build doesn't hold is `Failed`. The default (a backend without
    /// groups) knows only `boot`.
    fn load_group(&mut self, group: crate::GroupId) -> crate::LoadState {
        self.group_state(group)
    }

    /// Free an asset group's memory (a load in progress is cancelled). It
    /// reads as `Unloaded` at once and its assets stop drawing; the memory
    /// is back before the next frame's update. `boot` stays.
    fn unload_group(&mut self, _group: crate::GroupId) {}

    /// Where a group is, without asking for it.
    fn group_state(&self, group: crate::GroupId) -> crate::LoadState {
        if group == crate::GroupId::BOOT {
            crate::LoadState::Ready
        } else {
            crate::LoadState::Failed
        }
    }

    /// Bytes of a blob in any loaded group, boot included: borrowed from the
    /// platform, valid until its group unloads (copy what must outlive the
    /// frame, or look it up again). [`blob`](Self::blob) stays the `'static`
    /// boot-only path.
    fn group_blob(&self, id: BlobId) -> Option<&[u8]> {
        self.blob(id)
    }

    /// The services' state this backend holds ([`crate::services`], issue
    /// #61): every backend returns its [`HostedServices`](crate::HostedServices)
    /// and the methods below work through it. `None` (the default): a
    /// platform without services, where they are no-ops.
    fn services(&self) -> Option<&crate::HostedServices> {
        None
    }

    fn services_mut(&mut self) -> Option<&mut crate::HostedServices> {
        None
    }

    /// What this platform's services do: where achievements live, whether
    /// the store draws its own popups, syncs saves, shows presence.
    fn services_info(&self) -> crate::ServicesInfo {
        self.services()
            .map_or(crate::ServicesInfo::NONE, |s| s.info())
    }

    /// Unlock an achievement of `assets/achievements.ron` (once; unknown
    /// ids are ignored). The PC and the Web persist it after the frame; a
    /// Steam launch also unlocks it on Steam. The N64 and the 3DS keep it
    /// for the session: the game's save holds it
    /// ([`achievements`](Self::achievements)`().pack_for(..)`).
    fn unlock(&mut self, achievement: crate::AchievementId) {
        let table = crate::services::table_of(self);
        if let Some(s) = self.services_mut() {
            s.unlock(&table, achievement);
        }
    }

    /// Is `achievement` unlocked?
    fn is_unlocked(&self, achievement: crate::AchievementId) -> bool {
        let table = crate::services::table_of(self);
        table
            .index(achievement)
            .is_some_and(|i| self.achievements().is_unlocked(i))
    }

    /// Set a stat (clamped to its range; an increment-only stat keeps its
    /// highest value) and unlock the achievements whose threshold it
    /// reaches.
    fn set_stat(&mut self, stat: crate::StatId, value: i32) {
        let table = crate::services::table_of(self);
        if let Some(s) = self.services_mut() {
            s.set_stat(&table, stat, value);
        }
    }

    /// A stat's value (0 when unknown).
    fn stat(&self, stat: crate::StatId) -> i32 {
        let table = crate::services::table_of(self);
        table
            .stat_index(stat)
            .map_or(0, |i| self.achievements().stat(i))
    }

    /// Every achievement's bit and every stat, by table index: what an
    /// in-game list draws ([`crate::AchievementList`]) and what a console
    /// save stores ([`AchievementState::pack_for`](crate::AchievementState::pack_for)).
    fn achievements(&self) -> crate::AchievementState {
        self.services()
            .map_or(crate::AchievementState::EMPTY, |s| *s.state())
    }

    /// Give back a saved state (the game's console save after loading it):
    /// unlocked bits add up, increment-only stats keep the higher value,
    /// no popups. Harmless where the platform keeps its own copy (it
    /// merges).
    fn restore_achievements(&mut self, saved: &crate::AchievementState) {
        let table = crate::services::table_of(self);
        if let Some(s) = self.services_mut() {
            s.restore(&table, saved);
        }
    }

    /// Lock every achievement and zero the stats (tests, a debug menu, the
    /// editor's Play mode). On Steam it resets the account's too.
    fn reset_achievements(&mut self) {
        if let Some(s) = self.services_mut() {
            s.reset();
        }
    }

    /// The oldest unlock not shown yet, by table index
    /// ([`crate::AchievementToast::update`] takes them).
    fn next_unlock(&mut self) -> Option<usize> {
        self.services_mut().and_then(|s| s.next_unlock())
    }

    /// What friends see the player doing: a key of the table's `presence`
    /// list and its arguments (Steam rich presence; a documented no-op
    /// elsewhere, kept for [`services`](Self::services) readers).
    fn set_presence(&mut self, key: crate::StrKey, args: &[crate::Arg<'_>]) {
        if let Some(s) = self.services_mut() {
            s.set_presence(crate::Presence::new(key, args));
        }
    }

    /// The game just saved: a store with cloud saves may sync now (Steam
    /// Cloud uploads the save directory when the game exits; the hint
    /// commits the stats and the files sooner). A no-op elsewhere.
    fn cloud_sync_hint(&mut self) {
        if let Some(s) = self.services_mut() {
            s.cloud_sync_hint();
        }
    }

    /// Crash reports (issue #61): how many earlier runs left one, the
    /// player's consent to send them, whether the package names somewhere to
    /// send them. A game asks once ([`CrashReports::should_ask`](crate::CrashReports::should_ask))
    /// and answers with [`set_crash_consent`](Self::set_crash_consent).
    /// `None` on the consoles and the Web (no reports) — the default.
    fn crash_reports(&self) -> Option<crate::CrashReports> {
        None
    }

    /// The player's answer (the PC persists it; `Allow` sends the reports
    /// waiting and later ones). A no-op where there are no reports.
    fn set_crash_consent(&mut self, _consent: crate::CrashConsent) {}

    /// The memory this hardware has and what holds it ([`crate::MemoryReport`]):
    /// heap used/peak/total and the resident assets by category. Zeros where
    /// the backend doesn't count.
    fn memory(&self) -> crate::MemoryReport {
        crate::MemoryReport::default()
    }
}
