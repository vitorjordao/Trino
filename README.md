# Evidence: Bongo-Kart epic #24, the audio PR (#175, #177, #179 voices)

Rendered offline by the PC backend (`PcAudio::new_offline`, 48 kHz stereo) from
`crates/platform-pc/tests/audio_evidence.rs`:

```
cargo test --release -p trino-platform-pc --test audio_evidence -- --ignored --nocapture
```

`wav/` holds what the mixer produced, `spectrograms/` and `waveforms/` are `ffmpeg`
pictures of the same files (`showspectrumpic`, `showwavespic`).

| File | What you hear |
|---|---|
| `pitch.wav` | #175: `sounds/water_splash` at 0.5×, 0.75×, 1×, 1.5×, 2× (each shorter and higher), then `sounds/water` looping while `set_voice_pitch` bends it 1× → 2× over 3 s |
| `pause_resume.wav` | #177: `music/theme` 2 s, `pause_music` 1 s of silence, `resume_music` goes on from 2.000 s (the 3.2 s loop then wraps) |
| `crossfade.wav` | #177: a calm loop, a 1.5 s `crossfade_music(.., sync)` into its busy variation at 3 s (the beat doesn't skip), a synced cut back at 7 s |
| `layers.wav` | #177: `play_music_layers` of bass, melody and arpeggio: the melody fades in at 2 s, the arpeggio at 4 s, both fade out at 6 s |
| `bus_low_pass.wav` | #177: the theme on the music bus, `set_bus_filter(Music, LowPass 600 Hz)` from 2 s to 4 s, then off |
| `doppler_pass_by.wav` | #179: a 30 m/s engine hum passing 4 m from the listener through a `VoicePool` with physical doppler: 1.095× approaching, 0.920× leaving (3.0 semitones), panned left to right |

Mixer cost (release, 24 looping voices, 10 s of output, Ryzen 7 5825U): pitch 1 14.2 ms,
pitch 1.37 61.9 ms, with a bus low-pass 16.8 / 62.8 ms (under 0.7 % of real time).
