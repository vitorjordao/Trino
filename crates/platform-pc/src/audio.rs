//! cpal audio backend with a small software mixer.
//!
//! Sounds are mono `f32` sample buffers at the device sample rate (the asset
//! pipeline resamples at bake time from Fase 2 on). The mixer itself
//! ([`mixer`]) is pure and unit-tested; cpal only drains it.
//!
//! If no output device exists (common on CI runners) the backend degrades to
//! a silent no-op instead of failing — audio must never break a smoke test.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait};
use trino_core::{Audio, MusicId, SoundId};

pub mod mixer {
    //! Pure mixing core, no cpal types.

    use std::sync::Arc;

    pub struct Voice {
        pub data: Arc<Vec<f32>>,
        pub cursor: usize,
        pub looped: bool,
        pub is_music: bool,
    }

    /// Mix all voices into `out` (mono), advancing cursors, applying
    /// `master` gain, clamping to [-1, 1] and dropping finished voices.
    pub fn mix_into(out: &mut [f32], voices: &mut Vec<Voice>, master: f32) {
        out.fill(0.0);
        for voice in voices.iter_mut() {
            for slot in out.iter_mut() {
                if voice.cursor >= voice.data.len() {
                    if voice.looped && !voice.data.is_empty() {
                        voice.cursor = 0;
                    } else {
                        break;
                    }
                }
                *slot += voice.data[voice.cursor] * master;
                voice.cursor += 1;
            }
        }
        voices.retain(|v| v.looped || v.cursor < v.data.len());
        for slot in out.iter_mut() {
            *slot = slot.clamp(-1.0, 1.0);
        }
    }
}

use mixer::Voice;

struct Shared {
    voices: Vec<Voice>,
    master: f32,
}

pub struct PcAudio {
    shared: Arc<Mutex<Shared>>,
    sounds: HashMap<u32, Arc<Vec<f32>>>,
    music: HashMap<u32, Arc<Vec<f32>>>,
    sample_rate: u32,
    // Keeps the output stream alive; None = no audio device (silent mode).
    _stream: Option<cpal::Stream>,
}

impl PcAudio {
    pub fn new() -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            voices: Vec::new(),
            master: 1.0,
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

        let mut mono = Vec::new();
        let stream = device
            .build_output_stream(
                config.into(),
                move |data: &mut [f32], _| {
                    let frames = data.len() / channels;
                    mono.resize(frames, 0.0);
                    if let Ok(mut s) = shared.lock() {
                        let master = s.master;
                        mixer::mix_into(&mut mono, &mut s.voices, master);
                    } else {
                        mono.fill(0.0);
                    }
                    for (frame, sample) in mono.iter().enumerate() {
                        for ch in 0..channels {
                            data[frame * channels + ch] = *sample;
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

    /// Device sample rate — generate/resample sound data to this.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Register mono f32 samples for a sound handle. Re-uploading replaces
    /// the content (live reload); playing voices keep the old buffer until
    /// they finish.
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
        let Some(data) = self.sounds.get(&sound.0) else {
            return;
        };
        if let Ok(mut s) = self.shared.lock() {
            s.voices.push(Voice {
                data: data.clone(),
                cursor: 0,
                looped: false,
                is_music: false,
            });
        }
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        let Some(data) = self.music.get(&music.0) else {
            return;
        };
        if let Ok(mut s) = self.shared.lock() {
            s.voices.retain(|v| !v.is_music);
            s.voices.push(Voice {
                data: data.clone(),
                cursor: 0,
                looped,
                is_music: true,
            });
        }
    }

    fn stop_music(&mut self) {
        if let Ok(mut s) = self.shared.lock() {
            s.voices.retain(|v| !v.is_music);
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
    use super::mixer::{Voice, mix_into};
    use std::sync::Arc;

    fn voice(data: Vec<f32>, looped: bool) -> Voice {
        Voice {
            data: Arc::new(data),
            cursor: 0,
            looped,
            is_music: false,
        }
    }

    #[test]
    fn mixes_and_advances() {
        let mut voices = vec![voice(vec![0.5; 4], false), voice(vec![0.25; 2], false)];
        let mut out = [0.0f32; 4];
        mix_into(&mut out, &mut voices, 1.0);
        assert_eq!(out, [0.75, 0.75, 0.5, 0.5]);
        // Both voices played to completion and were dropped.
        assert!(voices.is_empty());
    }

    #[test]
    fn clamps_to_unit_range() {
        let mut voices = vec![voice(vec![0.9; 2], false), voice(vec![0.9; 2], false)];
        let mut out = [0.0f32; 2];
        mix_into(&mut out, &mut voices, 1.0);
        assert_eq!(out, [1.0, 1.0]);
    }

    #[test]
    fn master_volume_scales() {
        let mut voices = vec![voice(vec![0.8; 2], false)];
        let mut out = [0.0f32; 2];
        mix_into(&mut out, &mut voices, 0.5);
        assert_eq!(out, [0.4, 0.4]);
    }

    #[test]
    fn looped_voice_wraps_and_survives() {
        let mut voices = vec![voice(vec![0.1, 0.2], true)];
        let mut out = [0.0f32; 5];
        mix_into(&mut out, &mut voices, 1.0);
        assert_eq!(out, [0.1, 0.2, 0.1, 0.2, 0.1]);
        assert_eq!(voices.len(), 1);
    }

    #[test]
    fn empty_looped_voice_does_not_hang() {
        let mut voices = vec![voice(vec![], true)];
        let mut out = [0.0f32; 8];
        mix_into(&mut out, &mut voices, 1.0);
        assert_eq!(out, [0.0; 8]);
    }
}
