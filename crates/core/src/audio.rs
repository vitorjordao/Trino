//! The audio contract. Backends: cpal (PC), libdragon mixer + xm64 (N64),
//! ndsp (3DS).

/// Handle to a baked sound effect (`.wav64` on N64, PCM elsewhere).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SoundId(pub u32);

impl SoundId {
    /// Handle from a logical asset path, e.g. `SoundId::from_path("sounds/beep")`.
    pub const fn from_path(logical_path: &str) -> Self {
        SoundId(crate::asset::asset_id(logical_path))
    }
}

/// Handle to a baked music track (`.xm64` on N64, streamed elsewhere).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MusicId(pub u32);

impl MusicId {
    pub const fn from_path(logical_path: &str) -> Self {
        MusicId(crate::asset::asset_id(logical_path))
    }
}

pub trait Audio {
    /// Fire-and-forget playback of a sound effect.
    fn play_sound(&mut self, sound: SoundId);

    /// Start a music track, replacing the current one.
    fn play_music(&mut self, music: MusicId, looped: bool);

    fn stop_music(&mut self);

    /// Master volume, 0.0..=1.0.
    fn set_master_volume(&mut self, volume: f32);
}
