//! wav64 playback through libdragon's mixer.
//!
//! Channel plan (16 mixer channels): 0..=9 round-robin one-shots, 10..=13
//! steerable voices ([`VoiceId`] = channel + generation), 14 music, 15
//! stingers (the music ducks to 30% while one plays; [`N64Audio::poll`]
//! restores it). Call `poll` once per frame.
//!
//! libdragon streams a wav64 from one file cursor, so **a sound plays on
//! one channel at a time**: re-triggering a sound that is still playing
//! restarts it (two overlapping copies of the same wav64 corrupt the
//! stream — the mixer asserts).

use alloc::collections::BTreeMap;
use core::ffi::c_void;

use trino_core::{Audio, MusicId, SoundId, SoundParams, VoiceId};

use crate::ffi;

const ONESHOT_CHANNELS: u32 = 10;
const VOICE_FIRST: u32 = 10;
const VOICE_COUNT: usize = 4;
const MUSIC_CHANNEL: u32 = 14;
const STINGER_CHANNEL: u32 = 15;
const DUCK: f32 = 0.3;

/// Fixed point for the shim (65536 = 1.0; see `ffi::trino_ch_volume`).
fn q16(v: f32) -> u32 {
    (v.clamp(0.0, 2.0) * 65536.0) as u32
}

pub struct N64Audio {
    sounds: BTreeMap<u32, *mut c_void>,
    music: BTreeMap<u32, *mut c_void>,
    next_channel: u32,
    /// Generation per voice channel: stale handles are ignored.
    generations: [u8; VOICE_COUNT],
    music_volume: f32,
    ducked: bool,
    /// Sound handle last started on each channel (one channel per sound).
    channel_sound: [u32; 16],
}

impl N64Audio {
    pub fn new() -> Self {
        N64Audio {
            sounds: BTreeMap::new(),
            music: BTreeMap::new(),
            next_channel: 0,
            generations: [0; VOICE_COUNT],
            music_volume: 1.0,
            ducked: false,
            channel_sound: [0; 16],
        }
    }

    pub(crate) fn register_sound(&mut self, id: u32, wav: *mut c_void) {
        self.sounds.insert(id, wav);
    }

    pub(crate) fn register_music(&mut self, id: u32, wav: *mut c_void) {
        self.music.insert(id, wav);
    }

    /// Feed the mixer and restore ducked music; call once per frame.
    pub fn poll(&mut self) {
        unsafe { ffi::trino_audio_poll() }
        // Stinger over: bring the music back.
        if self.ducked && unsafe { ffi::trino_ch_playing(STINGER_CHANNEL) } == 0 {
            self.ducked = false;
            self.apply_music_volume();
        }
    }

    /// Stop `sound` wherever it still plays, then record it on `channel`.
    fn claim(&mut self, sound: u32, channel: u32) {
        for (ch, &s) in self.channel_sound.iter().enumerate() {
            if s == sound && unsafe { ffi::trino_ch_playing(ch as u32) } != 0 {
                unsafe { ffi::trino_channel_stop(ch as u32) }
            }
        }
        self.channel_sound[channel as usize] = sound;
    }

    fn apply_music_volume(&self) {
        let v = self.music_volume * if self.ducked { DUCK } else { 1.0 };
        unsafe { ffi::trino_ch_volume(MUSIC_CHANNEL, q16(v), q16(v)) }
    }

    fn voice(&self, voice: VoiceId) -> Option<u32> {
        let ch = voice.0 & 0xFF;
        let generation = (voice.0 >> 8) as u8;
        let slot = ch.checked_sub(VOICE_FIRST)? as usize;
        (slot < VOICE_COUNT && self.generations[slot] == generation).then_some(ch)
    }
}

impl Default for N64Audio {
    fn default() -> Self {
        Self::new()
    }
}

impl Audio for N64Audio {
    fn play_sound(&mut self, sound: SoundId) {
        self.play_sound_with(sound, &SoundParams::DEFAULT);
    }

    fn play_sound_with(&mut self, sound: SoundId, params: &SoundParams) {
        if let Some(&wav) = self.sounds.get(&sound.0) {
            let channel = self.next_channel;
            self.next_channel = (self.next_channel + 1) % ONESHOT_CHANNELS;
            self.claim(sound.0, channel);
            let (l, r) = params.gains();
            unsafe {
                ffi::trino_wav_play(wav, channel, 0);
                ffi::trino_ch_volume(channel, q16(l), q16(r));
            }
        }
    }

    fn play_voice(
        &mut self,
        sound: SoundId,
        params: &SoundParams,
        looped: bool,
    ) -> Option<VoiceId> {
        let &wav = self.sounds.get(&sound.0)?;
        let slot = (0..VOICE_COUNT)
            .find(|&i| unsafe { ffi::trino_ch_playing(VOICE_FIRST + i as u32) } == 0)?;
        let ch = VOICE_FIRST + slot as u32;
        self.claim(sound.0, ch);
        self.generations[slot] = self.generations[slot].wrapping_add(1);
        let (l, r) = params.gains();
        unsafe {
            ffi::trino_wav_play(wav, ch, looped as u32);
            ffi::trino_ch_volume(ch, q16(l), q16(r));
        }
        Some(VoiceId(ch | (self.generations[slot] as u32) << 8))
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        if let Some(ch) = self.voice(voice) {
            let (l, r) = params.gains();
            unsafe { ffi::trino_ch_volume(ch, q16(l), q16(r)) }
        }
    }

    fn stop_voice(&mut self, voice: VoiceId) {
        if let Some(ch) = self.voice(voice) {
            unsafe { ffi::trino_channel_stop(ch) }
            let slot = (ch - VOICE_FIRST) as usize;
            self.generations[slot] = self.generations[slot].wrapping_add(1);
        }
    }

    fn voice_playing(&self, voice: VoiceId) -> bool {
        self.voice(voice)
            .is_some_and(|ch| unsafe { ffi::trino_ch_playing(ch) } != 0)
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        // On the N64 looping comes from the wav64 file (audioconv64
        // --wav-loop at bake time); the flag is passed for API symmetry.
        if let Some(&wav) = self.music.get(&music.0) {
            unsafe {
                ffi::trino_channel_stop(MUSIC_CHANNEL);
                ffi::trino_wav_play(wav, MUSIC_CHANNEL, looped as u32);
            }
            self.apply_music_volume();
        }
    }

    fn stop_music(&mut self) {
        unsafe { ffi::trino_channel_stop(MUSIC_CHANNEL) }
    }

    fn play_stinger(&mut self, music: MusicId) {
        if let Some(&wav) = self.music.get(&music.0) {
            unsafe {
                ffi::trino_channel_stop(STINGER_CHANNEL);
                ffi::trino_wav_play(wav, STINGER_CHANNEL, 0);
                let v = self.music_volume;
                ffi::trino_ch_volume(STINGER_CHANNEL, q16(v), q16(v));
            }
            self.ducked = true;
            self.apply_music_volume();
        }
    }

    fn stinger_playing(&self) -> bool {
        unsafe { ffi::trino_ch_playing(STINGER_CHANNEL) != 0 }
    }

    fn set_music_volume(&mut self, volume: f32) {
        self.music_volume = volume.clamp(0.0, 1.0);
        self.apply_music_volume();
    }

    fn set_master_volume(&mut self, volume: f32) {
        unsafe { ffi::trino_mixer_set_vol(q16(volume.clamp(0.0, 1.0))) }
    }
}
