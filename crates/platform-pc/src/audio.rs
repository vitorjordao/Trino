//! cpal audio backend with a small software mixer.
//!
//! Sounds are mono `f32` sample buffers at the device sample rate (resampled
//! at load time). The mixer ([`mixer`]) is pure and unit-tested; cpal only
//! drains it. It mixes in **stereo**: every voice carries left/right gains
//! (volume + pan, [`trino_core::SoundParams::gains`]), music ducks while a
//! stinger plays, and voices have ids so games can steer them.
//!
//! If no output device exists (common on CI runners) the backend degrades to
//! a silent no-op instead of failing — audio must never break a smoke test.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait};
use trino_core::{Audio, MusicId, SoundId, SoundParams, VoiceId};

pub mod mixer {
    //! Pure stereo mixer (unit-tested without an audio device).

    use std::sync::Arc;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Kind {
        Sfx,
        Music,
        Stinger,
    }

    pub struct Voice {
        pub id: u32,
        pub data: Arc<Vec<f32>>,
        pub cursor: usize,
        pub looped: bool,
        pub kind: Kind,
        /// Left / right gains.
        pub gains: (f32, f32),
    }

    /// Music level while a stinger plays.
    pub const DUCK: f32 = 0.3;

    /// Mix every voice into interleaved stereo `out` (`[l, r, l, r, ...]`),
    /// advance cursors, drop finished one-shots, clamp to -1..1.
    pub fn mix_into(out: &mut [f32], voices: &mut Vec<Voice>, master: f32, music: f32) {
        out.fill(0.0);
        let ducked = voices.iter().any(|v| v.kind == Kind::Stinger);
        let music_gain = music * if ducked { DUCK } else { 1.0 };
        for voice in voices.iter_mut() {
            let k = master
                * match voice.kind {
                    Kind::Music => music_gain,
                    Kind::Stinger => music,
                    Kind::Sfx => 1.0,
                };
            let (gl, gr) = (voice.gains.0 * k, voice.gains.1 * k);
            for frame in out.as_chunks_mut::<2>().0 {
                if voice.cursor >= voice.data.len() {
                    if voice.looped && !voice.data.is_empty() {
                        voice.cursor = 0;
                    } else {
                        break;
                    }
                }
                let s = voice.data[voice.cursor];
                frame[0] += s * gl;
                frame[1] += s * gr;
                voice.cursor += 1;
            }
        }
        voices.retain(|v| v.looped || v.cursor < v.data.len());
        for slot in out.iter_mut() {
            *slot = slot.clamp(-1.0, 1.0);
        }
    }
}

use mixer::{Kind, Voice};

struct Shared {
    voices: Vec<Voice>,
    master: f32,
    music: f32,
    next_id: u32,
}

impl Shared {
    fn push(&mut self, data: Arc<Vec<f32>>, looped: bool, kind: Kind, gains: (f32, f32)) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        self.voices.push(Voice {
            id,
            data,
            cursor: 0,
            looped,
            kind,
            gains,
        });
        id
    }
}

/// Most simultaneous steerable voices (like a console mixer's channels).
const MAX_VOICES: usize = 24;

pub struct PcAudio {
    shared: Arc<Mutex<Shared>>,
    sounds: HashMap<u32, Arc<Vec<f32>>>,
    music: HashMap<u32, Arc<Vec<f32>>>,
    sample_rate: u32,
    _stream: Option<cpal::Stream>,
}

impl PcAudio {
    pub fn new() -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            voices: Vec::new(),
            master: 1.0,
            music: 1.0,
            next_id: 0,
        }));

        let (stream, sample_rate) = match Self::open_stream(shared.clone()) {
            Some((stream, rate)) => (Some(stream), rate),
            None => {
                eprintln!("trino-audio: no output device, running silent");
                (None, 44_100)
            }
        };

        PcAudio {
            shared,
            sounds: HashMap::new(),
            music: HashMap::new(),
            sample_rate,
            _stream: stream,
        }
    }

    fn open_stream(shared: Arc<Mutex<Shared>>) -> Option<(cpal::Stream, u32)> {
        use cpal::traits::StreamTrait;

        let device = cpal::default_host().default_output_device()?;
        let config = device.default_output_config().ok()?;
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;

        let mut stereo = Vec::new();
        let stream = device
            .build_output_stream(
                config.into(),
                move |data: &mut [f32], _| {
                    let frames = data.len() / channels;
                    stereo.resize(frames * 2, 0.0);
                    if let Ok(mut s) = shared.lock() {
                        let (master, music) = (s.master, s.music);
                        mixer::mix_into(&mut stereo, &mut s.voices, master, music);
                    } else {
                        stereo.fill(0.0);
                    }
                    for frame in 0..frames {
                        let (l, r) = (stereo[frame * 2], stereo[frame * 2 + 1]);
                        let out = &mut data[frame * channels..(frame + 1) * channels];
                        match channels {
                            1 => out[0] = (l + r) * 0.5,
                            _ => {
                                out[0] = l;
                                out[1] = r;
                                for extra in &mut out[2..] {
                                    *extra = 0.0;
                                }
                            }
                        }
                    }
                },
                |e| eprintln!("trino-audio: stream error: {e}"),
                None,
            )
            .ok()?;
        stream.play().ok()?;
        Some((stream, sample_rate))
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn upload_sound(&mut self, id: SoundId, samples: Vec<f32>) {
        self.sounds.insert(id.0, Arc::new(samples));
    }

    pub fn upload_music(&mut self, id: MusicId, samples: Vec<f32>) {
        self.music.insert(id.0, Arc::new(samples));
    }
}

impl Default for PcAudio {
    fn default() -> Self {
        Self::new()
    }
}

impl Audio for PcAudio {
    fn play_sound(&mut self, sound: SoundId) {
        self.play_sound_with(sound, &SoundParams::DEFAULT);
    }

    fn play_sound_with(&mut self, sound: SoundId, params: &SoundParams) {
        let Some(data) = self.sounds.get(&sound.0) else {
            return;
        };
        if let Ok(mut s) = self.shared.lock() {
            s.push(data.clone(), false, Kind::Sfx, params.gains());
        }
    }

    fn play_voice(
        &mut self,
        sound: SoundId,
        params: &SoundParams,
        looped: bool,
    ) -> Option<VoiceId> {
        let data = self.sounds.get(&sound.0)?.clone();
        let mut s = self.shared.lock().ok()?;
        if s.voices.len() >= MAX_VOICES {
            return None;
        }
        Some(VoiceId(s.push(data, looped, Kind::Sfx, params.gains())))
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        if let Ok(mut s) = self.shared.lock()
            && let Some(v) = s.voices.iter_mut().find(|v| v.id == voice.0)
        {
            v.gains = params.gains();
        }
    }

    fn stop_voice(&mut self, voice: VoiceId) {
        if let Ok(mut s) = self.shared.lock() {
            s.voices.retain(|v| v.id != voice.0);
        }
    }

    fn voice_playing(&self, voice: VoiceId) -> bool {
        self.shared
            .lock()
            .is_ok_and(|s| s.voices.iter().any(|v| v.id == voice.0))
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        let Some(data) = self.music.get(&music.0) else {
            return;
        };
        if let Ok(mut s) = self.shared.lock() {
            s.voices.retain(|v| v.kind != Kind::Music);
            s.push(data.clone(), looped, Kind::Music, (1.0, 1.0));
        }
    }

    fn stop_music(&mut self) {
        if let Ok(mut s) = self.shared.lock() {
            s.voices.retain(|v| v.kind != Kind::Music);
        }
    }

    fn play_stinger(&mut self, music: MusicId) {
        let Some(data) = self.music.get(&music.0) else {
            return;
        };
        if let Ok(mut s) = self.shared.lock() {
            s.voices.retain(|v| v.kind != Kind::Stinger);
            s.push(data.clone(), false, Kind::Stinger, (1.0, 1.0));
        }
    }

    fn stinger_playing(&self) -> bool {
        self.shared
            .lock()
            .is_ok_and(|s| s.voices.iter().any(|v| v.kind == Kind::Stinger))
    }

    fn set_music_volume(&mut self, volume: f32) {
        if let Ok(mut s) = self.shared.lock() {
            s.music = volume.clamp(0.0, 1.0);
        }
    }

    fn set_master_volume(&mut self, volume: f32) {
        if let Ok(mut s) = self.shared.lock() {
            s.master = volume.clamp(0.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mixer::{DUCK, Kind, Voice, mix_into};
    use std::sync::Arc;

    fn voice(data: Vec<f32>, looped: bool) -> Voice {
        kind_voice(data, looped, Kind::Sfx)
    }

    fn kind_voice(data: Vec<f32>, looped: bool, kind: Kind) -> Voice {
        Voice {
            id: 1,
            data: Arc::new(data),
            cursor: 0,
            looped,
            kind,
            gains: (1.0, 1.0),
        }
    }

    /// Left channel of an interleaved buffer.
    fn left(out: &[f32]) -> Vec<f32> {
        out.as_chunks::<2>().0.iter().map(|f| f[0]).collect()
    }

    #[test]
    fn mixes_and_advances() {
        let mut voices = vec![voice(vec![0.5; 4], false), voice(vec![0.25; 2], false)];
        let mut out = [0.0f32; 8];
        mix_into(&mut out, &mut voices, 1.0, 1.0);
        assert_eq!(left(&out), [0.75, 0.75, 0.5, 0.5]);
        assert!(voices.is_empty(), "both played to completion");
    }

    #[test]
    fn clamps_to_unit_range() {
        let mut voices = vec![voice(vec![0.9; 2], false), voice(vec![0.9; 2], false)];
        let mut out = [0.0f32; 4];
        mix_into(&mut out, &mut voices, 1.0, 1.0);
        assert_eq!(out, [1.0; 4]);
    }

    #[test]
    fn master_volume_scales() {
        let mut voices = vec![voice(vec![0.8; 2], false)];
        let mut out = [0.0f32; 4];
        mix_into(&mut out, &mut voices, 0.5, 1.0);
        assert_eq!(out, [0.4; 4]);
    }

    #[test]
    fn looped_voice_wraps_and_survives() {
        let mut voices = vec![voice(vec![0.1, 0.2], true)];
        let mut out = [0.0f32; 10];
        mix_into(&mut out, &mut voices, 1.0, 1.0);
        assert_eq!(left(&out), [0.1, 0.2, 0.1, 0.2, 0.1]);
        assert_eq!(voices.len(), 1);
    }

    #[test]
    fn empty_looped_voice_does_not_hang() {
        let mut voices = vec![voice(vec![], true)];
        let mut out = [0.0f32; 8];
        mix_into(&mut out, &mut voices, 1.0, 1.0);
        assert_eq!(out, [0.0; 8]);
    }

    #[test]
    fn pan_gains_split_channels() {
        let mut v = voice(vec![1.0; 2], false);
        v.gains = trino_core::SoundParams {
            volume: 0.5,
            pan: 1.0,
        }
        .gains();
        let mut voices = vec![v];
        let mut out = [0.0f32; 4];
        mix_into(&mut out, &mut voices, 1.0, 1.0);
        assert_eq!(out, [0.0, 0.5, 0.0, 0.5], "fully right");
    }

    #[test]
    fn music_ducks_under_a_stinger_and_follows_music_volume() {
        let mut voices = vec![
            kind_voice(vec![0.5; 4], true, Kind::Music),
            kind_voice(vec![0.1; 2], false, Kind::Stinger),
        ];
        let mut out = [0.0f32; 8];
        mix_into(&mut out, &mut voices, 1.0, 1.0);
        let l = left(&out);
        assert!((l[0] - (0.5 * DUCK + 0.1)).abs() < 1e-6, "{l:?}");
        // Stinger done -> next buffer is back to full music.
        mix_into(&mut out, &mut voices, 1.0, 1.0);
        assert!((left(&out)[0] - 0.5).abs() < 1e-6);
        mix_into(&mut out, &mut voices, 1.0, 0.5);
        assert!((left(&out)[0] - 0.25).abs() < 1e-6, "music volume");
    }
}
