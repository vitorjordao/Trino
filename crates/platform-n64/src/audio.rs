//! wav64 playback through libdragon's mixer.
//!
//! Channel plan (16 mixer channels): 0..=9 round-robin one-shots, 10..=13
//! steerable voices ([`VoiceId`] = channel + generation), 14 music, 15
//! stingers (the music ducks to 30% while one plays, as on the PC). Call
//! [`N64Audio::poll`] once per frame.
//!
//! **Buses** (issue #53): every channel plays on a [`Bus`] (the asset's
//! from the bake's index, or the call's [`SoundParams::bus`]). Its volume
//! is the call's left/right gains × [`BusMixer::gain`] of that bus (master
//! × bus × ducking, mutes); `poll` advances the duck ramps by the time
//! since the last poll and sends the channels whose volume moved. The
//! mixer's own master volume stays 1.
//!
//! **Codecs**: VADPCM (decoded on the RSP), raw PCM, and Opus when the ROM
//! links the decoder (CPU + RSP; only when an asset needs it). Every wav64
//! streams from the ROM through its channel's sample buffer, so on the N64
//! every playing channel counts as a stream in [`Audio::audio_stats`].
//!
//! **Looping** follows the call (`play_voice`'s and `play_music`'s flag;
//! one-shots and stingers play once), as on the PC — the asset's `loop`
//! only trims its length for a seamless loop.
//!
//! libdragon streams a wav64 from one file cursor, so **a sound plays on
//! one channel at a time**: re-triggering a sound that is still playing
//! restarts it (two overlapping copies of the same wav64 corrupt the
//! stream — the mixer asserts).

use alloc::collections::BTreeMap;
use core::ffi::c_void;

use trino_core::{
    Audio, AudioStats, Bus, BusMixer, BusSet, DuckRule, MusicId, SoundId, SoundParams, VoiceId,
};

use crate::{ffi, runtime};

const CHANNELS: usize = 16;
const ONESHOT_CHANNELS: u32 = 10;
const VOICE_FIRST: u32 = 10;
const VOICE_COUNT: usize = 4;
const MUSIC_CHANNEL: u32 = 14;
const STINGER_CHANNEL: u32 = 15;
/// Music level while a stinger plays (the PC mixer's `DUCK`).
const DUCK: f32 = 0.3;

/// Fixed point for the shim (65536 = 1.0; see `ffi::trino_ch_volume`).
fn q16(v: f32) -> u32 {
    (v.clamp(0.0, 2.0) * 65536.0) as u32
}

#[derive(Clone, Copy)]
struct Clip {
    wav: *mut c_void,
    bus: Bus,
}

/// What a channel plays: the call's gains and the bus.
#[derive(Clone, Copy)]
struct Channel {
    gains: (f32, f32),
    bus: Bus,
}

/// The audio's frame cost and output so far, from the shim's counters
/// (`TRINO_AUDIO_COST` lines, the `N64_COST` audio terms).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioCost {
    /// Time spent in [`N64Audio::poll`]: mixing on the RSP (the CPU waits
    /// for it), VADPCM decoding, Opus decoding, ROM reads.
    pub poll_us: u32,
    pub polls: u32,
    /// The part the mixer spent waiting for its RSP mixing command.
    pub rsp_us: u32,
    /// Output buffers mixed and their length in samples.
    pub buffers: u32,
    pub buffer_samples: u32,
    /// Output rate (the AI's real one, ~44 100 Hz).
    pub rate: u32,
}

impl AudioCost {
    /// Output samples mixed.
    pub fn samples(&self) -> u64 {
        self.buffers as u64 * self.buffer_samples as u64
    }
}

pub struct N64Audio {
    sounds: BTreeMap<u32, Clip>,
    music: BTreeMap<u32, Clip>,
    next_channel: u32,
    /// Generation per voice channel: stale handles are ignored.
    generations: [u8; VOICE_COUNT],
    /// Sound handle last started on each channel (one channel per sound).
    channel_sound: [u32; CHANNELS],
    channels: [Channel; CHANNELS],
    /// Volumes last sent per channel (`u32::MAX`: none yet).
    sent: [(u32, u32); CHANNELS],
    buses: BusMixer,
    /// Channels playing at the last poll.
    playing: u32,
    last_poll_us: Option<u32>,
    opus: Option<bool>,
}

impl N64Audio {
    pub fn new() -> Self {
        N64Audio {
            sounds: BTreeMap::new(),
            music: BTreeMap::new(),
            next_channel: 0,
            generations: [0; VOICE_COUNT],
            channel_sound: [0; CHANNELS],
            channels: [Channel {
                gains: (1.0, 1.0),
                bus: Bus::Sfx,
            }; CHANNELS],
            sent: [(u32::MAX, u32::MAX); CHANNELS],
            buses: BusMixer::new(),
            playing: 0,
            last_poll_us: None,
            opus: None,
        }
    }

    pub(crate) fn register_sound(&mut self, id: u32, wav: *mut c_void, bus: Bus) {
        self.sounds.insert(id, Clip { wav, bus });
    }

    pub(crate) fn register_music(&mut self, id: u32, wav: *mut c_void, bus: Bus) {
        self.music.insert(id, Clip { wav, bus });
    }

    /// Free a sound (its asset group unloaded, issue #56): the channels
    /// playing it stop, then the wav64 closes. Later plays are silent.
    pub(crate) fn unregister_sound(&mut self, id: u32) -> bool {
        self.sounds.remove(&id).map(|c| self.free(c)).is_some()
    }

    /// Free a music track or stinger ([`N64Audio::unregister_sound`]).
    pub(crate) fn unregister_music(&mut self, id: u32) -> bool {
        self.music.remove(&id).map(|c| self.free(c)).is_some()
    }

    fn free(&mut self, clip: Clip) {
        unsafe { ffi::trino_wav_free(clip.wav) };
        // The shim stopped its channels: no voice of it is playing now.
        self.playing = unsafe { ffi::trino_audio_playing_mask() };
    }

    /// Enable Opus wav64s (before the first loads); false when the ROM was
    /// linked without the decoder.
    pub(crate) fn enable_opus(&mut self) -> bool {
        *self
            .opus
            .get_or_insert_with(|| unsafe { ffi::trino_audio_opus_init() } != 0)
    }

    /// Feed the mixer, advance the duck ramps and send the channel volumes
    /// that moved (bus changes, ducking, a stinger ending); once per frame.
    pub fn poll(&mut self) {
        unsafe { ffi::trino_audio_poll() }
        let now = runtime::now_us();
        let dt = self
            .last_poll_us
            .map_or(0.0, |t| now.wrapping_sub(t) as f32 / 1_000_000.0);
        self.last_poll_us = Some(now);
        self.playing = unsafe { ffi::trino_audio_playing_mask() };
        let mut sounding = BusSet::EMPTY;
        for ch in 0..CHANNELS {
            if self.playing & 1 << ch != 0 {
                sounding.insert(self.channels[ch].bus);
            }
        }
        self.buses.update(dt, sounding);
        self.refresh();
    }

    /// The audio's cost and output so far.
    pub fn cost(&self) -> AudioCost {
        let s = Self::counters();
        AudioCost {
            poll_us: s[4],
            polls: s[5],
            rsp_us: s[6],
            buffers: s[7],
            buffer_samples: s[8],
            rate: s[9],
        }
    }

    fn counters() -> [u32; 10] {
        let mut s = [0u32; 10];
        unsafe { ffi::trino_audio_stats(s.as_mut_ptr()) };
        s
    }

    /// Send the volume of every channel that plays.
    fn refresh(&mut self) {
        for ch in 0..CHANNELS as u32 {
            if self.playing & 1 << ch != 0 {
                self.send(ch);
            }
        }
    }

    /// Channel `ch`'s volume now: its gains × its bus's gain, the music
    /// ducked under a stinger.
    fn send(&mut self, ch: u32) {
        let c = self.channels[ch as usize];
        let mut g = self.buses.gain(c.bus);
        if ch == MUSIC_CHANNEL && self.playing & 1 << STINGER_CHANNEL != 0 {
            g *= DUCK;
        }
        let v = (q16(c.gains.0 * g), q16(c.gains.1 * g));
        if self.sent[ch as usize] != v {
            self.sent[ch as usize] = v;
            unsafe { ffi::trino_ch_volume(ch, v.0, v.1) }
        }
    }

    fn start(&mut self, wav: *mut c_void, ch: u32, looped: bool, gains: (f32, f32), bus: Bus) {
        self.channels[ch as usize] = Channel { gains, bus };
        unsafe { ffi::trino_wav_play(wav, ch, looped as u32) }
        self.playing |= 1 << ch;
        self.send(ch);
        if ch == STINGER_CHANNEL {
            self.send(MUSIC_CHANNEL);
        }
    }

    fn stop(&mut self, ch: u32) {
        unsafe { ffi::trino_channel_stop(ch) }
        self.playing &= !(1 << ch);
    }

    /// Stop `sound` wherever it still plays, then record it on `channel`.
    fn claim(&mut self, sound: u32, channel: u32) {
        for ch in 0..CHANNELS as u32 {
            if self.channel_sound[ch as usize] == sound && unsafe { ffi::trino_ch_playing(ch) } != 0
            {
                self.stop(ch);
            }
        }
        self.channel_sound[channel as usize] = sound;
    }

    fn voice(&self, voice: VoiceId) -> Option<u32> {
        let ch = voice.0 & 0xFF;
        let generation = (voice.0 >> 8) as u8;
        let slot = ch.checked_sub(VOICE_FIRST)? as usize;
        (slot < VOICE_COUNT && self.generations[slot] == generation).then_some(ch)
    }

    /// A bus setting changed: every playing channel gets its new volume.
    fn buses_changed(&mut self) {
        self.playing = unsafe { ffi::trino_audio_playing_mask() };
        self.refresh();
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
        if let Some(&clip) = self.sounds.get(&sound.0) {
            let channel = self.next_channel;
            self.next_channel = (self.next_channel + 1) % ONESHOT_CHANNELS;
            self.claim(sound.0, channel);
            let bus = params.bus.unwrap_or(clip.bus);
            self.start(clip.wav, channel, false, params.gains(), bus);
        }
    }

    fn play_voice(
        &mut self,
        sound: SoundId,
        params: &SoundParams,
        looped: bool,
    ) -> Option<VoiceId> {
        let &clip = self.sounds.get(&sound.0)?;
        let slot = (0..VOICE_COUNT)
            .find(|&i| unsafe { ffi::trino_ch_playing(VOICE_FIRST + i as u32) } == 0)?;
        let ch = VOICE_FIRST + slot as u32;
        self.claim(sound.0, ch);
        self.generations[slot] = self.generations[slot].wrapping_add(1);
        let bus = params.bus.unwrap_or(clip.bus);
        self.start(clip.wav, ch, looped, params.gains(), bus);
        Some(VoiceId(ch | (self.generations[slot] as u32) << 8))
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        if let Some(ch) = self.voice(voice) {
            let c = &mut self.channels[ch as usize];
            c.gains = params.gains();
            if let Some(bus) = params.bus {
                c.bus = bus;
            }
            self.send(ch);
        }
    }

    fn stop_voice(&mut self, voice: VoiceId) {
        if let Some(ch) = self.voice(voice) {
            self.stop(ch);
            let slot = (ch - VOICE_FIRST) as usize;
            self.generations[slot] = self.generations[slot].wrapping_add(1);
        }
    }

    fn voice_playing(&self, voice: VoiceId) -> bool {
        self.voice(voice)
            .is_some_and(|ch| unsafe { ffi::trino_ch_playing(ch) } != 0)
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        if let Some(&clip) = self.music.get(&music.0) {
            self.stop(MUSIC_CHANNEL);
            self.start(clip.wav, MUSIC_CHANNEL, looped, (1.0, 1.0), clip.bus);
        }
    }

    fn stop_music(&mut self) {
        self.stop(MUSIC_CHANNEL);
    }

    fn play_stinger(&mut self, music: MusicId) {
        if let Some(&clip) = self.music.get(&music.0) {
            self.stop(STINGER_CHANNEL);
            self.start(clip.wav, STINGER_CHANNEL, false, (1.0, 1.0), clip.bus);
        }
    }

    fn stinger_playing(&self) -> bool {
        unsafe { ffi::trino_ch_playing(STINGER_CHANNEL) != 0 }
    }

    fn set_music_volume(&mut self, volume: f32) {
        self.set_bus_volume(Bus::Music, volume);
    }

    fn set_master_volume(&mut self, volume: f32) {
        self.set_bus_volume(Bus::Master, volume);
    }

    fn set_bus_volume(&mut self, bus: Bus, volume: f32) {
        self.buses.set_volume(bus, volume);
        self.buses_changed();
    }

    fn bus_volume(&self, bus: Bus) -> f32 {
        self.buses.volume(bus)
    }

    fn set_bus_muted(&mut self, bus: Bus, muted: bool) {
        self.buses.set_muted(bus, muted);
        self.buses_changed();
    }

    fn bus_muted(&self, bus: Bus) -> bool {
        self.buses.muted(bus)
    }

    fn set_duck(&mut self, rule: DuckRule) {
        self.buses.set_duck(rule);
        self.buses_changed();
    }

    fn clear_ducks(&mut self) {
        self.buses.clear_ducks();
        self.buses_changed();
    }

    fn bus_duck(&self, bus: Bus) -> f32 {
        self.buses.duck(bus)
    }

    fn audio_stats(&self) -> AudioStats {
        let s = Self::counters();
        let playing = unsafe { ffi::trino_audio_playing_mask() };
        let voices = playing.count_ones() as u16;
        AudioStats {
            voices,
            // Every wav64 streams from the ROM.
            streams: voices,
            resident_bytes: s[0],
            stream_bytes: s[1],
            underruns: s[2],
            streamed_frames: s[3],
        }
    }
}
