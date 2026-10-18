//! ndsp playback of baked `T3AU` sounds (issue #53): DSP-ADPCM (the DSP
//! decodes it for free) or PCM16. Whole sounds sit in linear memory; a
//! streamed one (music, by default) is read from the RomFS in chunks by the
//! shim's audio thread, so a track costs three small buffers while it plays.
//!
//! Channel plan (16 mixer channels): 0..=9 round-robin one-shots, 10..=13
//! steerable voices ([`VoiceId`] = channel + generation), 14 music, 15
//! stingers (the music ducks to 30% while one plays, like the PC).
//!
//! **Buses:** every channel remembers its bus and its own gains (volume,
//! pan); a [`BusMixer`] gives each bus its gain (master × bus × ducking,
//! mutes). [`N3dsAudio::poll`] — once per frame — advances the duck ramps
//! by the real time elapsed and pushes the gains that changed.

use alloc::collections::BTreeMap;
use core::ffi::c_void;

use trino_core::{
    Audio, AudioStats, Bus, BusMixer, BusSet, DuckRule, MixerConfig, MusicId, SoundId, SoundParams,
    VoiceId,
};

use crate::{ffi, runtime};

const CHANNELS: usize = 16;
const ONESHOT_CHANNELS: u32 = 10;
const VOICE_FIRST: u32 = 10;
const VOICE_COUNT: usize = 4;
const MUSIC_CHANNEL: u32 = 14;
const STINGER_CHANNEL: u32 = 15;
/// Music level while a stinger plays (the PC's `mixer::DUCK`).
const DUCK: f32 = 0.3;

/// Fixed point for the shim (65536 = 1.0; see `ffi::trino_ch_volume`).
fn q16(v: f32) -> u32 {
    (v.clamp(0.0, 2.0) * 65536.0) as u32
}

/// A loaded sound and the bus it was baked for.
#[derive(Clone, Copy)]
struct Clip {
    wav: *mut c_void,
    bus: Bus,
}

/// What a channel last started: its bus and its own gains.
#[derive(Clone, Copy)]
struct Playing {
    bus: Bus,
    gains: (f32, f32),
}

pub struct N3dsAudio {
    sounds: BTreeMap<u32, Clip>,
    music: BTreeMap<u32, Clip>,
    next_channel: u32,
    /// Generation per voice channel: stale handles are ignored.
    generations: [u8; VOICE_COUNT],
    channels: [Option<Playing>; CHANNELS],
    /// The q16 gains last sent per channel.
    sent: [(u32, u32); CHANNELS],
    buses: BusMixer,
    last_us: Option<u32>,
}

impl N3dsAudio {
    pub fn new() -> Self {
        N3dsAudio {
            sounds: BTreeMap::new(),
            music: BTreeMap::new(),
            next_channel: 0,
            generations: [0; VOICE_COUNT],
            channels: [None; CHANNELS],
            sent: [(u32::MAX, u32::MAX); CHANNELS],
            buses: BusMixer::new(),
            last_us: None,
        }
    }

    fn clip(wav: *mut c_void) -> Clip {
        let info = unsafe { ffi::trino_wav_info(wav) };
        Clip {
            wav,
            bus: Bus::from_index((info & 0xFF) as usize).unwrap_or(Bus::Sfx),
        }
    }

    pub(crate) fn register_sound(&mut self, id: u32, wav: *mut c_void) {
        self.sounds.insert(id, Self::clip(wav));
    }

    pub(crate) fn register_music(&mut self, id: u32, wav: *mut c_void) {
        self.music.insert(id, Self::clip(wav));
    }

    /// The project's mix (`audio/mixer`, baked from `assets/audio.toml`).
    pub(crate) fn apply_mixer(&mut self, bytes: &[u8]) {
        match MixerConfig::parse(bytes) {
            Ok(cfg) => cfg.apply(self),
            Err(e) => {
                runtime::log("TRINO_AUDIO: bad audio/mixer: ");
                runtime::log(e);
                runtime::log("\n");
            }
        }
    }

    /// Once per frame: release finished streams, advance the duck ramps by
    /// the time since the last call and push the gains that moved.
    pub fn poll(&mut self) {
        unsafe { ffi::trino_audio_poll() }
        let now = runtime::now_us();
        let dt = self
            .last_us
            .map_or(0.0, |l| now.wrapping_sub(l) as f32 / 1_000_000.0)
            .min(0.25);
        self.last_us = Some(now);
        let mut sounding = BusSet::EMPTY;
        for ch in 0..CHANNELS {
            match self.channels[ch] {
                Some(p) if playing(ch as u32) => sounding.insert(p.bus),
                Some(_) => self.channels[ch] = None,
                None => {}
            }
        }
        self.buses.update(dt, sounding);
        self.refresh();
    }

    /// Send every playing channel its gain (only the ones that changed).
    fn refresh(&mut self) {
        let stinger = self.channels[STINGER_CHANNEL as usize].is_some() && playing(STINGER_CHANNEL);
        for ch in 0..CHANNELS {
            if let Some(p) = self.channels[ch] {
                let duck = if ch as u32 == MUSIC_CHANNEL && stinger {
                    DUCK
                } else {
                    1.0
                };
                let g = self.buses.gain(p.bus) * duck;
                let q = (q16(p.gains.0 * g), q16(p.gains.1 * g));
                if q != self.sent[ch] {
                    self.sent[ch] = q;
                    unsafe { ffi::trino_ch_volume(ch as u32, q.0, q.1) }
                }
            }
        }
    }

    /// Start `clip` on `ch` with `gains` on `bus`.
    fn start(&mut self, ch: u32, clip: Clip, looped: bool, bus: Bus, gains: (f32, f32)) {
        unsafe { ffi::trino_wav_play(clip.wav, ch, looped as u32) }
        self.channels[ch as usize] = Some(Playing { bus, gains });
        // ndspChnReset put the mix back to full: send ours again.
        self.sent[ch as usize] = (u32::MAX, u32::MAX);
        self.refresh();
    }

    fn stop(&mut self, ch: u32) {
        unsafe { ffi::trino_channel_stop(ch) }
        self.channels[ch as usize] = None;
        // The music comes back from under a stopped stinger.
        self.refresh();
    }

    fn voice(&self, voice: VoiceId) -> Option<u32> {
        let ch = voice.0 & 0xFF;
        let generation = (voice.0 >> 8) as u8;
        let slot = ch.checked_sub(VOICE_FIRST)? as usize;
        (slot < VOICE_COUNT && self.generations[slot] == generation).then_some(ch)
    }

    /// The core the shim's streaming thread runs on (2 = the New 3DS's
    /// third core, 0 = the app core).
    pub fn stream_core(&self) -> u32 {
        unsafe { ffi::trino_audio_stat(5) }
    }
}

fn playing(ch: u32) -> bool {
    unsafe { ffi::trino_ch_playing(ch) != 0 }
}

impl Default for N3dsAudio {
    fn default() -> Self {
        Self::new()
    }
}

impl Audio for N3dsAudio {
    fn play_sound(&mut self, sound: SoundId) {
        self.play_sound_with(sound, &SoundParams::DEFAULT);
    }

    fn play_sound_with(&mut self, sound: SoundId, params: &SoundParams) {
        if let Some(&clip) = self.sounds.get(&sound.0) {
            let channel = self.next_channel;
            self.next_channel = (self.next_channel + 1) % ONESHOT_CHANNELS;
            let bus = params.bus.unwrap_or(clip.bus);
            self.start(channel, clip, false, bus, params.gains());
        }
    }

    fn play_voice(
        &mut self,
        sound: SoundId,
        params: &SoundParams,
        looped: bool,
    ) -> Option<VoiceId> {
        let &clip = self.sounds.get(&sound.0)?;
        let slot = (0..VOICE_COUNT).find(|&i| !playing(VOICE_FIRST + i as u32))?;
        let ch = VOICE_FIRST + slot as u32;
        self.generations[slot] = self.generations[slot].wrapping_add(1);
        let bus = params.bus.unwrap_or(clip.bus);
        self.start(ch, clip, looped, bus, params.gains());
        Some(VoiceId(ch | (self.generations[slot] as u32) << 8))
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        if let Some(ch) = self.voice(voice)
            && let Some(p) = &mut self.channels[ch as usize]
        {
            p.gains = params.gains();
            self.refresh();
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
        self.voice(voice).is_some_and(playing)
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        if let Some(&clip) = self.music.get(&music.0) {
            self.start(MUSIC_CHANNEL, clip, looped, clip.bus, (1.0, 1.0));
        }
    }

    fn stop_music(&mut self) {
        self.stop(MUSIC_CHANNEL);
    }

    fn play_stinger(&mut self, music: MusicId) {
        if let Some(&clip) = self.music.get(&music.0) {
            self.start(STINGER_CHANNEL, clip, false, clip.bus, (1.0, 1.0));
        }
    }

    fn stinger_playing(&self) -> bool {
        playing(STINGER_CHANNEL)
    }

    fn set_music_volume(&mut self, volume: f32) {
        self.set_bus_volume(Bus::Music, volume);
    }

    fn set_master_volume(&mut self, volume: f32) {
        self.set_bus_volume(Bus::Master, volume);
    }

    fn set_bus_volume(&mut self, bus: Bus, volume: f32) {
        self.buses.set_volume(bus, volume);
        self.refresh();
    }

    fn bus_volume(&self, bus: Bus) -> f32 {
        self.buses.volume(bus)
    }

    fn set_bus_muted(&mut self, bus: Bus, muted: bool) {
        self.buses.set_muted(bus, muted);
        self.refresh();
    }

    fn bus_muted(&self, bus: Bus) -> bool {
        self.buses.muted(bus)
    }

    fn set_duck(&mut self, rule: DuckRule) {
        self.buses.set_duck(rule);
    }

    fn clear_ducks(&mut self) {
        self.buses.clear_ducks();
        self.refresh();
    }

    fn bus_duck(&self, bus: Bus) -> f32 {
        self.buses.duck(bus)
    }

    fn audio_stats(&self) -> AudioStats {
        let stat = |which| unsafe { ffi::trino_audio_stat(which) };
        AudioStats {
            voices: (0..CHANNELS as u32).filter(|&ch| playing(ch)).count() as u16,
            streams: stat(0) as u16,
            resident_bytes: stat(1),
            stream_bytes: stat(2),
            underruns: stat(3),
            streamed_frames: stat(4),
        }
    }
}
