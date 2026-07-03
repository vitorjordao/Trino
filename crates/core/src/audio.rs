//! The audio contract. Backends: cpal (PC), libdragon mixer + xm64 (N64),
//! ndsp (3DS).

/// Handle to a baked sound effect (`.wav64` on N64, PCM elsewhere).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SoundId(pub u32);

/// Handle to a baked music track (`.xm64` on N64, streamed elsewhere).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MusicId(pub u32);

pub trait Audio {
    /// Fire-and-forget playback of a sound effect.
    fn play_sound(&mut self, sound: SoundId);

    /// Start a music track, replacing the current one.
    fn play_music(&mut self, music: MusicId, looped: bool);

    fn stop_music(&mut self);

    /// Master volume, 0.0..=1.0.
    fn set_master_volume(&mut self, volume: f32);
}
