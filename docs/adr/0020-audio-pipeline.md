# ADR 0020: Audio pipeline — a codec per platform, streaming, buses

Status: accepted (October 2026, GitHub #53). Every phase of the issue's roadmap is
implemented: buses in the core and every mixer (§5), PC streaming and Vorbis (§3, §4),
3DS DSP-ADPCM and RomFS streaming (§3, §4), N64 VADPCM/Opus with a measured audio term
in `N64_COST` (§3, §7), the inspector, CLI and docs (§8), and the Web (§4).

## Context

Audio was the largest uncompressed payload on every target, and the mixer had two
knobs:

- **PC:** `TSND` was f32 PCM, decoded whole into RAM (`upload_music(Vec<f32>)`):
  castle64's 9 sounds took 1.36 MB on disk, Solid Eleven's 10 took 2.29 MB, and a
  10-minute track would hold 115 MB of floats at 48 kHz.
- **3DS:** raw PCM16, every sound and track loaded whole into linear memory
  (castle64: 678 KB of the app's linear heap at boot; Solid Eleven: 1.14 MB).
- **N64:** libdragon's `audioconv64` already wrote VADPCM by default
  (`--wav-compress 1`), but its encoder clipped loud masters: castle64's theme decoded
  at 7.6 dB SNR, with ~110 full-scale clicks on near-silent passages.
- **Web:** 16-bit PCM in the pack — downloaded by every player of a URL.
- **Mixer:** `set_master_volume` and `set_music_volume` only; no buses, no ducking
  rules, nothing persisted, nothing measured.

## Decisions

### 1. One master, a codec per platform, in the manifest

`[sounds.*]` / `[music.*]` gain `codec`, `quality`, `stream`, `sample_rate` and `bus`
(`trino_asset_pipeline::audio::AudioSettings`). `codec` and `sample_rate` take one
value or a per-platform table (`3ds` or `n3ds`; the Web takes the PC's value when it
has none, like every Web setting). `auto` (the default) is:

| Platform | `auto` | Also | Why |
|---|---|---|---|
| N64 | VADPCM | `opus`, `pcm` | the RSP decodes it in libdragon's mixer; 3.5:1 |
| 3DS | DSP-ADPCM | `pcm` | the DSP decodes it in hardware, free of CPU; 3.5:1 |
| PC | Ogg Vorbis | `pcm` | 5–6:1 on music; lewton decodes it in pure Rust |
| Web | Ogg Vorbis | `pcm` | the same decoder in wasm, the smallest download |

A codec the platform can't play is a bake error listing the ones it can (ADR 0009: never a
silent fallback). Music streams by default; sounds load whole (`stream = true` opts in).
Vorbis `quality` defaults to 6. Stereo masters are averaged to mono, as before.

### 2. Formats, versioned (ADR 0005 §3)

- **`TSN2`** (`trino_core::SoundBlob`, PC and Web): a 20-byte header — codec (PCM16 or
  Vorbis), default bus, loop and stream flags, rate, frames, payload length — and the
  payload. The loader converts a v1 `TSND` (f32) into a PCM16 `TSN2` with the kind's old
  defaults, so an old bake still loads.
- **`T3AU`** (3DS, `audio::n3ds_container`): codec, bus, flags, rate, samples, the
  DSP-ADPCM coefficients and, for streamed sounds, one decoder context per 1024-frame
  chunk (what `ndspAdpcmData` takes), then the data.
- **`wav64`** (N64): written by the engine for VADPCM and raw (`audio::vadpcm`), by
  `audioconv64 --wav-compress 3` for Opus only.
- **`TMIX`** (`trino_core::MixerConfig`, every platform): the project mix, baked from
  `assets/audio.toml` (version 1) as the `audio/mixer` asset.

### 3. Encoders the engine owns

- **VADPCM** (`audio/vadpcm.rs`): four order-2 predictors by k-means over each frame's
  prediction error, each frame searched over every predictor and scale against the
  decoder's own output. Its decoder matches libdragon's RSP decoder sample for sample
  (three references, seven files). It replaces `audioconv64`'s encoder: 30.8 dB on
  castle64's theme instead of 7.6, 3–9 dB better on every castle64 sound, same sizes.
- **DSP-ADPCM** (`audio/dspadpcm.rs`): eight predictors (entry 0 pinned to (0, 0)),
  every frame searched over predictors and shifts, decoded as Citra/Azahar's DSP does.
  No devkitPro tool in the bake path, byte-for-byte reproducible.
- **Vorbis:** libvorbis (aoTuV, through `vorbis_rs`) encodes in the bake with the asset
  id as the Ogg serial (reproducible bakes); lewton decodes on the PC, in the browser's
  wasm and in the tools, and matches libvorbis's own decoder to > 100 dB.
- Every bake, `audio inspect|report|preview` and the editor call one function,
  `audio::encode`, so the tools measure the bytes that ship.

### 4. Streaming per platform

- **PC:** a streamed sound keeps its encoded file in RAM. Playing it starts a decoder
  (Vorbis or PCM) and a lock-free ring of half a second; a worker thread keeps every
  ring topped up, the device callback only pops. A 10-minute track plays with 317 KB of
  heap growth (`platform-pc/tests/audio_stream.rs`, counting allocator) instead of
  115 MB; loops are sample-exact (lewton's padded last block is cut at the baked length).
  Offline backends (`PcAudio::new_offline` + `render`) decode inline: tests and tools
  render exactly what a device would play.
- **3DS:** a streamed sound keeps only its header and contexts; each playback reads
  1024-frame chunks from RomFS into three linear wave buffers (24 KB for DSP-ADPCM), each
  with its own decoder context, so loops and seams play exactly what the bake previews.
  An audio thread woken by ndsp refills them, on the New 3DS's third core when it can
  start there (#54), else on the app core.
- **N64:** every `wav64` already streams from the cartridge; RDRAM holds ~1 KB per open
  file and the mixer's sample buffer per channel.
- **Web:** chunks decoded ahead in the wasm (§9).

### 5. Buses in the core, mixed by every backend

`Bus` (`Master`, `Music`, `Sfx`, `Voice`, `Ambience`, `Ui`), `DuckRule` (trigger bus,
target bus, level, attack and release in ms) and `BusMixer` — the arithmetic: gain =
master × bus × duck, per-bus mutes, linear ramps advanced by elapsed time, the deepest
duck wins, eight rules at most. The `Audio` trait gains `set_bus_volume`, `bus_volume`,
`set_bus_muted`, `bus_muted`, `set_duck`, `clear_ducks`, `bus_duck` and `audio_stats`, all
with defaults: `set_bus_volume(Master | Music)` calls the old `set_master_volume` /
`set_music_volume`, so existing backends and callers are unchanged. Every backend (PC,
N64, 3DS, Web) keeps a `BusMixer`; the N64 and 3DS advance it in `poll()` and send a
channel's gain only when it changes. The stinger's 30% music duck stays as it was.

`SoundParams` gains `bus: Option<Bus>` (`SoundParams::on(bus)`): `None` plays on the
sound's baked bus — the issue's "default from the manifest's `bus`" needs the option.

### 6. The project's mix and the player's

`assets/audio.toml` (`MixFile`) sets each bus's starting volume and the duck rules; hosts
apply the baked `audio/mixer` at start. The player's own volumes (a game's options menu)
are `BusSettings`: the PC runner persists them per game in `<save_name>.audio` (text, one
line per bus) and applies them over the mix; consoles keep `BusSettings::to_bytes()` in the
game's save if it opts in. That is **4 bytes**, not the issue's 2: six buses at 4 bits are
24 bits before the six mute bits.

### 7. Measured costs, quoted honestly

- **N64** (ares, mean of the 8 `TRINO_N64_TEXT_PAD` layouts): the mixer costs 504 µs of a
  60 Hz frame idle, +260 µs per 44.1 kHz VADPCM voice (+164 µs at 22.05 kHz), ~4.6 ms
  for an Opus track — CPU time in `poll`, which waits for the RSP. `N64_COST` gains
  `audio_us(voice_rates, opus_voices, fps)` from these fits; `audio inspect` quotes it per
  asset; the self-test prints `TRINO_AUDIO_COST:` lines. castle64's perf scenes pass 8/8
  layouts (their timers cover update and render; `poll` is outside them).
- **3DS:** the DSP decodes ADPCM; the CPU cost is the refill thread's RomFS reads.
- **PC/Web:** lewton decodes 10 minutes of 16 kHz Vorbis in about a second on one core.

### 8. Tools and the editor

`cargo xtask audio inspect|report|preview|mix` (`--json`) and the editor's Assets › Audio
inspector and Mixer panel read and write the same text files (the manifest keys,
`assets/audio.toml`); the self-test step `audio codecs` runs on the PC, in ares, in
Azahar and in the browsers.

### 9. The Web

- **Pack `TPAK` v2:** sound and music entries are `TSN2` as the Web bake makes them
  (Vorbis by default); entry kind 7 is the `TMIX` mix. v1 packs still load: their `TS16`
  sounds become PCM16 `TSN2`; a mixer entry in a v1 pack is an error.
- **Decoding:** lewton in the wasm module, the PC's decoder, so the browser plays the
  PC's samples (`platform-web/src/sound.rs`, host-tested against the PC decode). Effects
  decode once into `AudioBuffer`s at load.
- **Streams:** music keeps only its Ogg bytes; each playback decodes 8192-sample chunks,
  each its own buffer source started exactly where the previous one ends, at most 1 s
  ahead — about 1 s of decoded audio per stream, seamless at the loop point.
- **Graph:** source → gain → panner → bus gain → master gain → speakers. The bus and
  master gains come from `BusMixer`, advanced in `poll()` with the frame's real time and
  smoothed over 10 ms. The player's volumes persist in `localStorage`
  (`trino-audio:<save>`), applied over the pack's mix.
- **Not an AudioWorklet:** it would need a second wasm instance in the worklet scope and a
  `SharedArrayBuffer` ring; a 1 s lookahead covers a page that draws every frame. A
  hidden tab runs dry after 1 s and counts an underrun.
- **Measured:** castle64's audio in the pack 678 KB → 154 KB (−77%), Solid Eleven's
  1.14 MB → 253 KB (−78%); `trino.pak` gzip 419 → 194 KB; the wasm grows by 72 KB
  gzipped (lewton). The `audio codecs` step passes in Chromium (WebGL2 and WebGPU),
  Firefox and WebKit with 0 underruns in the step.

## Deviations from the issue

- **Vorbis q6 measures 27–41 dB of waveform SNR** on the fixtures, not ≥ 30: a perceptual
  codec spends bits by audibility, and square-wave chiptune masters cost it most. The
  decoder is not the cause (lewton ≡ libvorbis). The test asserts 26 dB at q6 and 29.5 dB
  at q8; the issue's VADPCM ≥ 25 dB and DSP-ADPCM ≥ 30 dB hold as written.
- **The Web plays Vorbis, not Opus:** one decoder (lewton) for the PC, the browser and the
  tools; Opus in wasm needs libopus (C) or `decodeAudioData`, which can neither stream nor
  loop sample-exactly. Opus stays an option for later.
- **N64 Opus** is opt-in per asset: it links libopus (+99 KB of ROM) and raises every
  channel's rate limit to 48 kHz; it decodes only on the console (no host preview).
- **Mono only**, as before: stereo masters are averaged.
- **The N64 ROM grows** (1 376 256 → 1 507 328 B for castle64): the engine's audio is the
  same size as main's (VADPCM was already the default), the growth is libopus for the
  self-test's Opus asset and the self-test's three new sounds.

## Follow-ups

- Tracker music (XM/MOD; libdragon's `xm64`) on every target — tiny ROM, era-authentic.
- Opus on the PC and the Web; stereo masters.
- `auto` on the PC/Web choosing PCM16 when Vorbis is bigger: Ogg's headers (~4 KB) make a
  0.05 s click twice its PCM16 size.
- A per-bus captions/accessibility layer (#60) on top of the buses.
- 3DS: decide the audio thread's core with the CPU budgets of #54/#56.
