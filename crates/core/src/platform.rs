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

pub trait Platform {
    /// Monotonic time in microseconds since an arbitrary epoch.
    fn now_micros(&self) -> u64;

    /// Debug logging. Routed to stdout on PC, ISViewer on N64, the debug
    /// channel on 3DS.
    fn log(&self, message: &str);

    /// Bytes of a baked blob, or `None` if the handle is unknown. Blobs are
    /// loaded at boot and live as long as the platform.
    fn blob(&self, _id: BlobId) -> Option<&[u8]> {
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
}
