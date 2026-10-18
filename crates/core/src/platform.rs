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
    /// Interface scale for the game to apply (0 = auto). The engine only
    /// stores and persists it.
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

    /// The window/monitor/picture sizes and display settings, on platforms
    /// where they can change (PC). `None` on consoles.
    fn display(&self) -> Option<DisplayInfo> {
        None
    }

    /// Ask for new display settings (PC; a no-op elsewhere). The host
    /// applies them after the frame and calls [`crate::Game::resize`] when
    /// the picture's size changes; it also persists them.
    fn set_display(&mut self, _settings: DisplaySettings) {}

    /// The input bindings' state, on platforms where they can be remapped
    /// (PC). `None` on consoles.
    fn input_info(&self) -> Option<InputInfo> {
        None
    }

    /// The first connected gamepad's name, as the system reports it.
    fn gamepad_name(&self) -> Option<&str> {
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

    /// The memory this hardware has and what holds it ([`crate::MemoryReport`]):
    /// heap used/peak/total and the resident assets by category. Zeros where
    /// the backend doesn't count.
    fn memory(&self) -> crate::MemoryReport {
        crate::MemoryReport::default()
    }
}
