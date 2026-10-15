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

    /// The window/monitor/picture sizes and display settings, on platforms
    /// where they can change (PC). `None` on consoles.
    fn display(&self) -> Option<DisplayInfo> {
        None
    }

    /// Ask for new display settings (PC; a no-op elsewhere). The host
    /// applies them after the frame and calls [`crate::Game::resize`] when
    /// the picture's size changes; it also persists them.
    fn set_display(&mut self, _settings: DisplaySettings) {}
}
