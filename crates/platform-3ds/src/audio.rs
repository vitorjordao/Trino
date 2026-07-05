//! Raw-PCM16 playback through ndsp.
//!
//! Channels 0..=13 round-robin for SFX; 14 is reserved for music. ndsp is
//! callback-driven, so [`N3dsAudio::poll`] is a no-op kept for loop symmetry
//! with the other consoles.

use alloc::collections::BTreeMap;
use core::ffi::c_void;

use trino_core::{Audio, MusicId, SoundId};

use crate::ffi;

const SFX_CHANNELS: u32 = 14;
const MUSIC_CHANNEL: u32 = 14;

pub struct N3dsAudio {
    sounds: BTreeMap<u32, *mut c_void>,
    music: BTreeMap<u32, *mut c_void>,
    next_channel: u32,
}

impl N3dsAudio {
    pub fn new() -> Self {
        N3dsAudio {
            sounds: BTreeMap::new(),
            music: BTreeMap::new(),
            next_channel: 0,
        }
    }

    pub(crate) fn register_sound(&mut self, id: u32, wav: *mut c_void) {
        self.sounds.insert(id, wav);
    }

    pub(crate) fn register_music(&mut self, id: u32, wav: *mut c_void) {
        self.music.insert(id, wav);
    }

    /// ndsp needs no per-frame pump; kept so game loops stay identical
    /// across consoles.
    pub fn poll(&mut self) {
        unsafe { ffi::trino_audio_poll() }
    }
}

impl Default for N3dsAudio {
    fn default() -> Self {
        Self::new()
    }
}

impl Audio for N3dsAudio {
    fn play_sound(&mut self, sound: SoundId) {
        if let Some(&wav) = self.sounds.get(&sound.0) {
            let channel = self.next_channel;
            self.next_channel = (self.next_channel + 1) % SFX_CHANNELS;
            unsafe { ffi::trino_wav_play(wav, channel, 0) }
        }
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        if let Some(&wav) = self.music.get(&music.0) {
            unsafe {
                ffi::trino_channel_stop(MUSIC_CHANNEL);
                ffi::trino_wav_play(wav, MUSIC_CHANNEL, looped as u32);
            }
        }
    }

    fn stop_music(&mut self) {
        unsafe { ffi::trino_channel_stop(MUSIC_CHANNEL) }
    }

    fn set_master_volume(&mut self, _volume: f32) {
        // Not exposed by the shim yet (kept as a no-op for ABI stability).
    }
}
