# ADR 0044: What Bongo Kart's study asked of the audio — pitch, positional voices with a budget, music control, bus filters

Status: accepted (November 2026). Bongo Kart's study (its epic #24) ranked the engine
changes the game needs; this ADR covers the audio's: GitHub Bongo-Kart #175 (§1), the
voice half of #179 (§2) and #177 (§3, §4). ADR 0043 has the renderer's.

All of it is new methods with defaults on `Audio` and new core types: no
`GAME_API_VERSION` bump, and a backend that doesn't implement a method behaves as
before. `Audio::audio_features` says which ones a backend does, so a game (and the
self-test) knows what to expect instead of guessing from the platform.

| | PC | Web | N64 | 3DS |
|---|---|---|---|---|
| pitch | yes | yes (`playbackRate`) | yes, ≤ 44.1 kHz | yes (DSP rate) |
| music pause | yes | yes | — | yes (`ndspChnSetPaused`) |
| music position | yes | yes | — | — |
| crossfade | yes | yes | cut | cut |
| layers | 4 | 4 | the first | the first |
| bus filter | low-pass | low-pass (`BiquadFilterNode`) | — | — |
| `VoicePool` | yes | yes | yes | yes |

## 1. Pitch (Bongo-Kart #175)

### Context

A kart engine is one recorded loop played faster as the revs go up; skids, boosts and
coins vary by pitch so the hundredth one doesn't sound like the first. The mixers played
every sound at its own rate.

### Decisions

- **`SoundParams::pitch`** (default 1), a rate multiplier: 2 is an octave up and half as
  long, 0.5 an octave down and twice as long. `SoundParams::rate()` clamps it to
  `PITCH_MIN..=PITCH_MAX` (0.5..=2, NaN → 1), the range every backend can play.
  `play_sound_with`, `play_voice` and `set_voice` carry it; `Audio::set_voice_pitch`
  bends a playing voice alone (an engine's revs every frame).
- **Per voice, never global:** pitching one voice must not change another's length (the
  issue's acceptance test, `pitch_is_per_voice_and_scales_its_duration`).
- **PC:** the mixer's voice cursor is fractional (`f64`) with linear interpolation
  between samples. At pitch 1 on a whole position it keeps the old integer path, so every
  existing sound mixes byte for byte as before. Streamed sounds (music) ignore pitch:
  their ring is filled at the output rate.
- **Web:** `AudioBufferSourceNode.playbackRate`; a silent playback (no Web Audio) scales
  its end time.
- **N64:** `mixer_ch_set_freq` after `wav64_play` (`trino_ch_freq`, q16 across the
  o32/o64 boundary). A channel's default limit is the output rate, so the rate stops at
  44.1 kHz: a 22.05 kHz sound (`sample_rate = { n64 = 22050 }`) reaches 2×, a 44.1 kHz
  one can only go down. Raising the limit would grow every voice channel's sample buffer,
  RAM the base console doesn't have. libdragon resets a channel's rate only when another
  waveform plays there: a replay of the same sound keeps the last rate, so the backend
  sends the rate after every play (the first self-test run measured a 1× splash at the 2×
  one's length).
- **3DS:** `ndspChnSetRate` (`trino_ch_rate`), the DSP resamples.

## 2. Positional voices with a budget (Bongo-Kart #179, voice half)

### Context

Eight karts each have an engine, skids, a boost and item sounds; the N64 and the 3DS have
four voice channels, and a browser tab gets muddy long before it runs out. A game needs a
rule for who is heard, the same on every target, and doppler for a kart passing by.

### Decisions

- **`trino_core::VoicePool<N>`**, no allocation and no backend code: it drives any
  `Audio` through `play_voice` / `set_voice` / `stop_voice`, so all four targets have it
  and the rule is the same everywhere.
- **`SoundEmitter`**: sound, position, velocity, radius, volume, loop, `priority: u8`, its
  own pitch and an optional bus. `play_voice_at(audio, &emitter) -> Option<PoolVoice>`.
- **Who keeps a voice:** priority first, then loudness at the listener. A full pool gives
  the lowest-ranked slot to a sound that ranks above it, else refuses the newcomer
  (`PoolStats::refused`). When the backend runs out before the pool (the consoles' four
  channels), the lowest-ranked slot that holds a backend voice gives it up the same way.
- **Virtual loops:** a looping sound out of range, or whose voice was taken, keeps its
  slot and handle without a voice and starts again when it is heard; a one-shot out of
  range ends. Handles carry a generation: a stopped or replaced sound's handle goes stale.
- **Doppler:** `Doppler { speed_of_sound, scale }`, `(c + v_listener) / (c + v_source)`
  along the line between them, each speed kept under 0.9 c, times the emitter's own pitch,
  clamped to the pitch range. `PHYSICAL` is 343 m/s; a larger `scale` exaggerates it (an
  arcade racer), 0 turns it off. A kart at 30 m/s passing the listener is 1.095× coming and
  0.920× going: 3.0 semitones apart (`audio_evidence.rs::doppler_pass_by`).
- **`update` once a frame:** steers volume, pan and pitch, parks or wakes loops, frees
  one-shots that ended.
- **No float `Option`** in the pool's state (the N64 pitfall): the listener is plain
  fields plus a flag.

## 3. Music control (Bongo-Kart #177)

### Context

A pause menu that restarts the music, a lap-final track that cuts in on the wrong beat and
an intensity that can only switch tracks all sound cheap. The game asked for pause,
position, crossfades in sync, and layers it can fade per section.

### Decisions

- **`pause_music` / `resume_music` / `music_paused`:** the track (every layer) holds where
  it is. A crossfade's outgoing track has nothing to come back to and stops.
- **`music_position() -> Option<f32>`:** seconds into the track, a loop's wrapped;
  `None` with no music, or where the backend doesn't know (the consoles).
- **`crossfade_music(music, looped, fade_ms, sync)`:** the old track (every layer) fades
  out while the new one fades in, linear in gain. With `sync` the new one starts where
  the old one is, so two arrangements of one piece at one tempo swap without skipping a
  beat. The default (consoles) is `play_music`: a cut, from the start.
- **`play_music_layers(&[MusicId], looped)`** (up to `MAX_MUSIC_LAYERS` = 4): the tracks
  start together, the first at full and the rest silent; `set_music_layer(i, gain,
  fade_ms)` fades one, `music_layer(i)` reads its gain now. The layers share one
  clock, pause and resume together, and stay in step. The default plays the first layer.
- **PC:** every music voice has a level `Ramp` (linear, per sample) besides its volume; a
  crossfade marks the old voices `leaving` and drops them at silence. A synced start of a
  resident track sets the cursor; a stream seeks (below). The old track's position is read
  in samples, never through seconds, and the new voice skips whatever the device mixed
  between the two locks.
- **Web:** each music playback gets a level `GainNode` ramped with
  `linearRampToValueAtTime` on the context's clock (the backend keeps the same ramp to
  answer `music_layer`). Pause stops the sources but keeps what they play: a buffer's
  `AudioBuffer` and position; a stream's scheduled chunks (their buffers and how much of
  the first had played), so resuming schedules the same chunks again and nothing is
  decoded twice. All layers resume at one context time.
- **Stream seeks** (a synced crossfade into a streamed track): lewton's
  `seek_absgp_pg` lands on a page, then whole packets are decoded and dropped up to the
  exact sample, read from `get_last_absgp` once a packet ends a page. Two edges need care:
  near the start the page seek can land on the header pages (it then decodes from the
  top), and in the last page the padded last packet can't be placed (it seeks one page
  further back). The same code is on the PC (`VorbisDecode::seek`) and the Web
  (`StreamDecoder::seek`); both read the decode's samples exactly from any position.
- **3DS:** pause only (`ndspChnSetPaused`): a stream's queued chunks wait and the audio
  thread with them. Position would need the stream's chunk bookkeeping on the main
  thread, and nothing on the consoles asked for it yet.

## 4. Bus filters (Bongo-Kart #177)

- **`set_bus_filter(bus, Some(BusFilter::LowPass { cutoff_hz }))`**, `None` to take it
  off; `bus_filter` reads it back. The cutoff is clamped to 20..=20 000 Hz (NaN → open).
  One kind for now: a low-pass is what "under water", "pause menu" and "behind a wall"
  need; the enum leaves room for more.
- **PC:** an RBJ biquad (Q = 1/√2) per filtered bus; voices on a filtered bus mix into
  the bus's buffer, filtered once, then into the output. Retuning keeps the filter's
  state (no click). The master's filter runs last.
- **Web:** a `BiquadFilterNode` (`lowpass`) spliced between the bus's gain and the master
  (after the master for `Bus::Master`), its frequency glided; taken out again for `None`.
- **Consoles:** none. The N64 mixer has no per-channel filter and the DSP's per-channel
  IIR would need every voice's bus to agree; `audio_features().bus_filter` is false.

## 5. Measurements

The mixer's cost on the PC (`crates/platform-pc/tests/audio_evidence.rs::mixer_cost`,
release, 24 looping voices — the PC's limit —, 10 s of 48 kHz output in 512-sample blocks,
on a Ryzen 7 laptop):

| Voices at | Low-pass on their bus | 10 s mixed in | Share of real time |
|---|---|---|---|
| pitch 1 (the integer path, as before) | no | 14.2 ms | 0.14 % |
| pitch 1.37 (interpolated) | no | 61.9 ms | 0.62 % |
| pitch 1 | yes | 16.8 ms | 0.17 % |
| pitch 1.37 | yes | 62.8 ms | 0.63 % |

Interpolation costs about 4× the whole-sample path per voice and stays under 1 % of a
core; the bus filter is one biquad per filtered bus, not per voice.

The doppler pass-by and the other features are rendered to WAV by the same file's
ignored tests (`target/evidence/audio-control`).

## 6. Tests

- `crates/core/tests/voices.rs`: the pool's budget, priorities, the backend's limit,
  virtual loops, range, doppler and stale handles over a mixer with a fixed voice count.
- `crates/core/tests/audio.rs`: the defaults (a four-method backend reports no feature, a
  crossfade cuts, layers play the first) and the clamps.
- `crates/platform-pc/tests/audio_music.rs`: pitch per voice and bent, pause and
  position, crossfades (a cut in sync, a fade, a synced crossfade into a Vorbis stream
  equal to the stream's own samples within 1e-4), layers and the bus filter, through the
  trait, offline.
- Unit tests of the PC mixer (fractional cursor, ramps, leaving voices, the biquad) and of
  both stream decoders' seeks at the start, mid-way and in the last page.
- **The `audio control` self-test step** (`castle64::selftest::AudioControl`) on the PC
  (`engine_checks`), in ares (both tiers), in Azahar and in the browsers. It reads
  `audio_features` and holds the backend to it: a bus filter reads back only where it
  exists; a `VoicePool` of two over the backend's real voices refuses a lower priority,
  gives a higher one the lowest's slot and parks a loop out of range; one sound at 2×, 1×
  and 0.5× lasts in that order, each at least 1.5× the last; paused music stops pulling
  stream samples and holds its position, then goes on; a synced crossfade keeps the
  position; a layer fades in. On the N64 it runs last: each voice channel it plays on
  keeps a sample buffer, heap the base console's self-test is short of.
