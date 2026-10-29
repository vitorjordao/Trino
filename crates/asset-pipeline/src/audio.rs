//! Audio bake (issue #53): one master, a codec per platform.
//!
//! - **Codecs** ([`Codec`]): `auto` is VADPCM on the N64 (our encoder,
//!   decoded on the RSP by libdragon's mixer), DSP-ADPCM on the 3DS
//!   (decoded by the DSP in hardware), Ogg Vorbis on the PC and the Web
//!   (decoded by lewton). `pcm` everywhere; Opus on the N64 (opt-in).
//! - **Settings** ([`AudioSettings`]): codec, Vorbis quality, a per-platform
//!   resample, streaming and the default bus, from a `[sounds.*]` /
//!   `[music.*]` entry.
//! - **Containers:** PC and Web [`trino_core::SoundBlob`] (`TSN2`), the 3DS
//!   `T3AU` ([`n3ds_container`]), the N64 `wav64` ([`vadpcm`] writes and
//!   reads it; only Opus goes through libdragon's `audioconv64`, in `xtask`).
//! - **Previews** ([`encode`]): every platform's bytes and what it will
//!   play, decoded on the host — `audio inspect`/`preview` and the editor's
//!   audio inspector all call it.
//! - **The project mix** ([`MixFile`]): `assets/audio.toml` → `TMIX`.

pub mod dspadpcm;
pub mod vadpcm;
pub mod vorbis;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use trino_core::{Bus, DuckRule, MAX_DUCK_RULES, MixerConfig, SoundBlob, SoundCodec};

use crate::manifest::{Platform, SoundDecl};

/// A codec a platform can play.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Codec {
    /// 16-bit PCM, every platform.
    Pcm,
    /// Ogg Vorbis (PC, Web): lewton on the CPU.
    Vorbis,
    /// N64 VADPCM (wav64 `--wav-compress 1`): ~3.5:1, decoded by the mixer.
    Vadpcm,
    /// N64 Opus (wav64 `--wav-compress 3`): ~14:1, RSP-assisted decode;
    /// the runtime enables it (`wav64_init_compression(3)`).
    Opus,
    /// 3DS DSP-ADPCM: 3.5:1, decoded by the DSP for free.
    DspAdpcm,
}

impl Codec {
    pub const ALL: [Codec; 5] = [
        Codec::Pcm,
        Codec::Vorbis,
        Codec::Vadpcm,
        Codec::Opus,
        Codec::DspAdpcm,
    ];

    /// The name in the manifest.
    pub fn label(self) -> &'static str {
        match self {
            Codec::Pcm => "pcm",
            Codec::Vorbis => "vorbis",
            Codec::Vadpcm => "vadpcm",
            Codec::Opus => "opus",
            Codec::DspAdpcm => "dspadpcm",
        }
    }

    pub fn parse(s: &str) -> Option<Codec> {
        Codec::ALL.into_iter().find(|c| c.label() == s)
    }

    /// What `platform` plays.
    pub fn supported(platform: Platform) -> &'static [Codec] {
        match platform {
            Platform::Pc | Platform::Web => &[Codec::Pcm, Codec::Vorbis],
            Platform::N64 => &[Codec::Pcm, Codec::Vadpcm, Codec::Opus],
            Platform::N3ds => &[Codec::Pcm, Codec::DspAdpcm],
        }
    }

    /// `auto` on `platform`.
    pub fn auto(platform: Platform) -> Codec {
        match platform {
            Platform::Pc | Platform::Web => Codec::Vorbis,
            Platform::N64 => Codec::Vadpcm,
            Platform::N3ds => Codec::DspAdpcm,
        }
    }
}

/// Vorbis quality when the manifest has none (the issue's `quality = 6`:
/// ~130 kbit/s on 44.1 kHz mono music, a fifth of PCM16).
pub const DEFAULT_QUALITY: f32 = 6.0;

/// How one sound bakes for one platform.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AudioSettings {
    pub codec: Codec,
    /// The codec came from `auto` (or no `codec` key).
    pub auto: bool,
    /// Vorbis quality, 0..=10.
    pub quality: f32,
    /// Resample the master to this rate (`None` keeps it).
    pub sample_rate: Option<u32>,
    pub stream: bool,
    pub looped: bool,
    pub bus: Bus,
}

impl AudioSettings {
    pub fn from_decl(decl: &SoundDecl, music: bool, platform: Platform) -> Result<Self, String> {
        let mut unknown = Vec::new();
        if let Some(c) = &decl.codec {
            unknown.extend(c.unknown_keys());
        }
        if let Some(r) = &decl.sample_rate {
            unknown.extend(r.unknown_keys());
        }
        if let Some(k) = unknown.first() {
            return Err(format!("unknown platform `{k}` (pc, n64, 3ds, web)"));
        }
        let label = decl
            .codec
            .as_ref()
            .and_then(|c| c.get(platform))
            .unwrap_or_else(|| "auto".into());
        let (codec, auto) = if label == "auto" {
            (Codec::auto(platform), true)
        } else {
            let codec = Codec::parse(&label).ok_or_else(|| {
                format!("codec `{label}` (auto, pcm, vorbis, vadpcm, opus, dspadpcm)")
            })?;
            if !Codec::supported(platform).contains(&codec) {
                let names: Vec<_> = Codec::supported(platform)
                    .iter()
                    .map(|c| c.label())
                    .collect();
                return Err(format!(
                    "codec `{label}` doesn't play on {} (auto, {})",
                    platform.key(),
                    names.join(", ")
                ));
            }
            (codec, false)
        };
        let quality = decl.quality.unwrap_or(DEFAULT_QUALITY);
        if !(0.0..=10.0).contains(&quality) {
            return Err(format!("quality {quality} (0 to 10)"));
        }
        let sample_rate = decl.sample_rate.as_ref().and_then(|r| r.get(platform));
        if let Some(r) = sample_rate
            && !(4_000..=96_000).contains(&r)
        {
            return Err(format!("sample_rate {r} (4000 to 96000)"));
        }
        let bus = match &decl.bus {
            Some(b) => Bus::from_label(b)
                .filter(|&b| b != Bus::Master)
                .ok_or_else(|| format!("bus `{b}` (music, sfx, voice, ambience, ui)"))?,
            None if music => Bus::Music,
            None => Bus::Sfx,
        };
        Ok(AudioSettings {
            codec,
            auto,
            quality,
            sample_rate,
            stream: decl.stream.unwrap_or(music),
            looped: if music {
                decl.music_loops()
            } else {
                decl.sound_loops()
            },
            bus,
        })
    }
}

/// Mono master samples (-1..1) and their rate.
#[derive(Clone, Debug, PartialEq)]
pub struct Master {
    pub rate: u32,
    pub samples: Vec<f32>,
}

impl Master {
    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / self.rate.max(1) as f64
    }

    /// The master as `settings` resamples it.
    pub fn for_settings(&self, settings: &AudioSettings) -> Master {
        match settings.sample_rate {
            Some(r) if r != self.rate => Master {
                rate: r,
                samples: resample(&self.samples, self.rate, r),
            },
            _ => self.clone(),
        }
    }
}

/// One platform's result of baking a sound.
#[derive(Clone, Debug)]
pub struct Encoded {
    pub platform: Platform,
    pub settings: AudioSettings,
    pub rate: u32,
    pub frames: usize,
    /// What the platform stores (ROM, RomFS, pack, PC asset file). Exact for
    /// every container made here; the N64's Opus (`audioconv64`'s) is an
    /// estimate ([`Encoded::exact`]).
    pub bytes: usize,
    pub exact: bool,
    /// The container itself: PC/Web `TSN2`, 3DS `T3AU`, N64 `wav64` (VADPCM
    /// or raw); `None` for the N64's Opus.
    pub blob: Option<Vec<u8>>,
    /// What the platform plays, decoded on the host (`None`: N64 Opus).
    pub decoded: Option<Vec<f32>>,
}

impl Encoded {
    /// The same audio as 16-bit PCM at the platform's rate, for ratios.
    pub fn pcm16_bytes(&self) -> usize {
        self.frames * 2
    }

    /// Signal-to-noise ratio of what plays against the master, resampled to
    /// the platform's rate.
    pub fn snr_db(&self, master: &Master) -> Option<f64> {
        let decoded = self.decoded.as_ref()?;
        let reference = resample(&master.samples, master.rate, self.rate);
        Some(snr_db(&reference, decoded))
    }
}

/// Bake `master` for `platform` with `settings`: the container bytes and
/// what the platform will play. `serial` makes Vorbis reproducible (the
/// asset id).
pub fn encode(
    platform: Platform,
    settings: &AudioSettings,
    master: &Master,
    serial: u32,
) -> Result<Encoded, String> {
    let rate = match (platform, settings.sample_rate) {
        (_, Some(r)) => r,
        (Platform::N64, None) => master.rate.min(N64_MAX_RATE),
        (_, None) => master.rate,
    };
    let m = Master {
        rate,
        samples: resample(&master.samples, master.rate, rate),
    };
    let mut pcm = to_pcm16(&m.samples);
    if platform == Platform::N64 {
        pcm = n64_pcm(pcm, settings.looped)?;
    }
    let frames = pcm.len();
    let mut out = Encoded {
        platform,
        settings: *settings,
        rate: m.rate,
        frames,
        bytes: 0,
        exact: true,
        blob: None,
        decoded: None,
    };
    match (platform, settings.codec) {
        (Platform::Pc | Platform::Web, Codec::Pcm | Codec::Vorbis) => {
            let (codec, payload, decoded) = if settings.codec == Codec::Vorbis {
                let ogg = vorbis::encode(&m.samples, m.rate, settings.quality, serial as i32)?;
                let (_, decoded) = vorbis::decode(&ogg)?;
                (SoundCodec::Vorbis, ogg, decoded)
            } else {
                let bytes = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
                (SoundCodec::Pcm16, bytes, from_pcm16(&pcm))
            };
            let blob = sound_blob(codec, settings, m.rate, frames, &payload);
            out.bytes = blob.len();
            out.blob = Some(blob);
            out.decoded = Some(decoded);
        }
        (Platform::N3ds, Codec::Pcm | Codec::DspAdpcm) => {
            let (blob, decoded) = n3ds_container(settings, m.rate, &pcm);
            out.bytes = blob.len();
            out.blob = Some(blob);
            out.decoded = Some(decoded);
        }
        (Platform::N64, Codec::Pcm) => {
            let blob = vadpcm::write_wav64_raw(&pcm, m.rate, settings.looped);
            out.bytes = blob.len();
            out.blob = Some(blob);
            out.decoded = Some(from_pcm16(&pcm));
        }
        (Platform::N64, Codec::Vadpcm) => {
            let v = vadpcm::encode(&pcm);
            let blob = vadpcm::write_wav64(&v, m.rate, settings.looped);
            out.bytes = blob.len();
            out.blob = Some(blob);
            out.decoded = Some(from_pcm16(&vadpcm::decode(&v)));
        }
        (Platform::N64, Codec::Opus) => {
            out.bytes = opus_estimate_bytes(frames, m.rate);
            out.exact = false;
        }
        (p, c) => {
            return Err(format!("{} doesn't play {}", p.key(), c.label()));
        }
    }
    Ok(out)
}

/// The N64 mixer's channel limit: masters above it bake at it.
pub const N64_MAX_RATE: u32 = 44_100;

/// Every N64 length is a whole number of 16-sample blocks, the only loops
/// libdragon's VADPCM supports — and any sound may loop at run time
/// (`play_voice(.., true)`): a loop is trimmed (the few dropped samples are
/// inaudible, the seam stays the master's) or, shorter than a block,
/// repeated whole until it fills blocks exactly; a one-shot is padded with
/// silence.
pub fn n64_pcm(mut pcm: Vec<i16>, looped: bool) -> Result<Vec<i16>, String> {
    if looped {
        if pcm.is_empty() {
            return Err("an empty sound can't loop".into());
        }
        if pcm.len() < 16 {
            let (mut a, mut b) = (pcm.len(), 16);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            let lcm = pcm.len() / a * 16;
            pcm = pcm.iter().copied().cycle().take(lcm).collect();
        }
        pcm.truncate(pcm.len() / 16 * 16);
    } else {
        pcm.resize(pcm.len().div_ceil(16) * 16, 0);
    }
    Ok(pcm)
}

/// libdragon's Opus wav64 at the pinned commit runs ~50 kbit/s on mono
/// music (castle64's theme: 282 264 B raw → 19 858 B); a size estimate
/// for tools without Docker. The ROM build reports the real size.
pub fn opus_estimate_bytes(frames: usize, rate: u32) -> usize {
    let seconds = frames as f64 / rate.max(1) as f64;
    64 + (seconds * 50_000.0 / 8.0) as usize
}

/// A PC/Web `TSN2` blob.
pub fn sound_blob(
    codec: SoundCodec,
    settings: &AudioSettings,
    rate: u32,
    frames: usize,
    payload: &[u8],
) -> Vec<u8> {
    let header = SoundBlob {
        codec,
        bus: settings.bus,
        looped: settings.looped,
        stream: settings.stream,
        sample_rate: rate,
        frames: frames as u32,
        payload,
    }
    .header();
    let mut blob = header.to_vec();
    blob.extend_from_slice(payload);
    blob
}

/// The 3DS container's magic (`trino_wav_load` in the 3DS shim).
pub const T3AU_MAGIC: [u8; 4] = *b"T3AU";
/// Bytes before the contexts and the data.
pub const T3AU_HEADER_LEN: usize = 60;
/// A streamed 3DS sound plays in chunks of this many DSP-ADPCM frames
/// (14 336 samples, 8 KB of linear memory each; three in flight).
pub const N3DS_CHUNK_FRAMES: usize = 1024;
/// The most chunk contexts a `T3AU` holds (the 3DS shim mallocs them).
pub const T3AU_MAX_CONTEXTS: usize = 1 << 20;
/// The longest stream chunk, in samples (the 3DS shim allocates three).
pub const T3AU_MAX_CHUNK: usize = 1 << 24;

/// The 3DS container (`T3AU`, little-endian): `"T3AU"`, u8 codec (0 PCM16,
/// 1 DSP-ADPCM), u8 bus, u8 flags (bit 0 loop, bit 1 stream), u8 channels
/// (1), u32 rate, u32 samples, u32 data bytes, u32 chunk samples (streamed:
/// how many samples each wave buffer holds; 0 = one buffer), u32 context
/// count, 16 u16 DSP-ADPCM coefficients, then a context per chunk (u16
/// predictor/scale, i16 last sample, i16 the one before, u16 0: what
/// `ndspAdpcmData` takes), then the data. Returns the bytes and what the DSP
/// will play.
pub fn n3ds_container(settings: &AudioSettings, rate: u32, pcm: &[i16]) -> (Vec<u8>, Vec<f32>) {
    let adpcm = settings.codec == Codec::DspAdpcm;
    let (coefs, data, decoded, contexts, chunk) = if adpcm {
        let enc = dspadpcm::encode(pcm);
        let decoded = dspadpcm::decode(&enc.coefs, &enc.data, enc.samples);
        let (contexts, chunk) = if settings.stream {
            (
                dspadpcm::contexts(&enc, N3DS_CHUNK_FRAMES),
                N3DS_CHUNK_FRAMES * dspadpcm::SAMPLES_PER_FRAME,
            )
        } else {
            (Vec::new(), 0)
        };
        (enc.coefs, enc.data, decoded, contexts, chunk)
    } else {
        let data = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        // PCM streams in chunks of the same duration.
        let chunk = if settings.stream {
            N3DS_CHUNK_FRAMES * dspadpcm::SAMPLES_PER_FRAME
        } else {
            0
        };
        ([0i16; 16], data, pcm.to_vec(), Vec::new(), chunk)
    };
    let mut b = Vec::with_capacity(T3AU_HEADER_LEN + contexts.len() * 8 + data.len());
    b.extend_from_slice(&T3AU_MAGIC);
    b.push(adpcm as u8);
    b.push(settings.bus as u8);
    b.push(settings.looped as u8 | (settings.stream as u8) << 1);
    b.push(1);
    for v in [
        rate,
        pcm.len() as u32,
        data.len() as u32,
        chunk as u32,
        contexts.len() as u32,
    ] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    for c in coefs {
        b.extend_from_slice(&c.to_le_bytes());
    }
    debug_assert_eq!(b.len(), T3AU_HEADER_LEN);
    for c in &contexts {
        b.extend_from_slice(&(c.header as u16).to_le_bytes());
        b.extend_from_slice(&c.hist1.to_le_bytes());
        b.extend_from_slice(&c.hist2.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
    }
    b.extend_from_slice(&data);
    (b, from_pcm16(&decoded))
}

/// A parsed `T3AU` (tools and tests; the 3DS shim reads it in C).
#[derive(Clone, Debug, PartialEq)]
pub struct T3au {
    pub codec: Codec,
    pub bus: Bus,
    pub looped: bool,
    pub stream: bool,
    pub rate: u32,
    pub samples: usize,
    pub chunk_samples: usize,
    pub coefs: [i16; 16],
    pub contexts: Vec<dspadpcm::Context>,
    pub data: Vec<u8>,
}

impl T3au {
    pub fn parse(b: &[u8]) -> Result<T3au, String> {
        if b.len() < T3AU_HEADER_LEN || b[..4] != T3AU_MAGIC {
            return Err("T3AU: bad magic".into());
        }
        let u32_at = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let codec = match b[4] {
            0 => Codec::Pcm,
            1 => Codec::DspAdpcm,
            c => return Err(format!("T3AU: codec {c}")),
        };
        let bus = Bus::from_index(b[5] as usize).ok_or("T3AU: bus")?;
        // The 3DS shim plays mono only, and bounds what a header may make it
        // allocate (`t3au_header.h`, the same rules: the `t3au_shim` fuzz
        // target holds the two to each other, issue #121).
        if b[7] != 1 {
            return Err(format!("T3AU: {} channels", b[7]));
        }
        let (data_len, ctx_count) = (u32_at(16) as usize, u32_at(24) as usize);
        if ctx_count > T3AU_MAX_CONTEXTS || u32_at(20) as usize > T3AU_MAX_CHUNK {
            return Err("T3AU: too many contexts, or a chunk too long".into());
        }
        let ctx_end = ctx_count
            .checked_mul(8)
            .and_then(|n| n.checked_add(T3AU_HEADER_LEN))
            .ok_or("T3AU: length")?;
        if ctx_end.checked_add(data_len) != Some(b.len()) {
            return Err("T3AU: length".into());
        }
        // The data must hold every sample the header announces, and the
        // chunks' contexts must cover them (issue #68: `decode` asserted).
        let samples = u32_at(12) as usize;
        let chunk = u32_at(20) as usize;
        let enough = match (codec, chunk) {
            (Codec::DspAdpcm, 0) => dspadpcm::data_bytes(samples) <= data_len,
            (Codec::DspAdpcm, c) => {
                c % dspadpcm::SAMPLES_PER_FRAME == 0
                    && ctx_count == samples.div_ceil(c)
                    && (ctx_count == 0
                        || (ctx_count - 1)
                            * (c / dspadpcm::SAMPLES_PER_FRAME)
                            * dspadpcm::BYTES_PER_FRAME
                            + dspadpcm::data_bytes(samples - (ctx_count - 1) * c)
                            <= data_len)
            }
            _ => samples.checked_mul(2).is_some_and(|n| n <= data_len),
        };
        if !enough {
            return Err("T3AU: the data does not hold its samples".into());
        }
        let mut coefs = [0i16; 16];
        for (i, c) in coefs.iter_mut().enumerate() {
            *c = i16::from_le_bytes([b[28 + i * 2], b[29 + i * 2]]);
        }
        let contexts = b[T3AU_HEADER_LEN..ctx_end]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| dspadpcm::Context {
                header: c[0],
                hist1: i16::from_le_bytes([c[2], c[3]]),
                hist2: i16::from_le_bytes([c[4], c[5]]),
            })
            .collect();
        Ok(T3au {
            codec,
            bus,
            looped: b[6] & 1 != 0,
            stream: b[6] & 2 != 0,
            rate: u32_at(8),
            samples: u32_at(12) as usize,
            chunk_samples: u32_at(20) as usize,
            coefs,
            contexts,
            data: b[ctx_end..].to_vec(),
        })
    }

    /// What the DSP plays (chunk by chunk from the contexts when streamed,
    /// as the shim feeds it).
    pub fn decode(&self) -> Vec<i16> {
        match self.codec {
            Codec::DspAdpcm if self.chunk_samples > 0 => {
                let frames = self.chunk_samples / dspadpcm::SAMPLES_PER_FRAME;
                let mut out = Vec::with_capacity(self.samples);
                for (k, ctx) in self.contexts.iter().enumerate() {
                    let start = k * self.chunk_samples;
                    let n = self.chunk_samples.min(self.samples - start);
                    let data = &self.data[k * frames * dspadpcm::BYTES_PER_FRAME..];
                    out.extend(dspadpcm::decode_from(&self.coefs, data, n, *ctx));
                }
                out
            }
            Codec::DspAdpcm => dspadpcm::decode(&self.coefs, &self.data, self.samples),
            _ => self
                .data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&b| i16::from_le_bytes(b))
                .collect(),
        }
    }
}

/// -1..1 floats to 16-bit PCM (rounded, clamped).
pub fn to_pcm16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|s| (s * 32767.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect()
}

pub fn from_pcm16(pcm: &[i16]) -> Vec<f32> {
    pcm.iter().map(|&s| s as f32 / 32767.0).collect()
}

/// Signal-to-noise ratio (dB) of `decoded` against `reference`, over the
/// shorter of the two; `INFINITY` when they are equal.
pub fn snr_db(reference: &[f32], decoded: &[f32]) -> f64 {
    let (mut signal, mut noise) = (0.0f64, 0.0f64);
    for (&r, &d) in reference.iter().zip(decoded) {
        signal += (r as f64) * (r as f64);
        noise += (r as f64 - d as f64).powi(2);
    }
    if noise == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (signal / noise).log10()
    }
}

/// Band-limited resample (windowed sinc, 16 zero crossings a side; the
/// cutoff follows the lower rate), for the bake. Runtime resamplers are
/// linear.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() || from == 0 || to == 0 {
        return samples.to_vec();
    }
    const ZEROS: f64 = 16.0;
    let step = from as f64 / to as f64;
    let cutoff = (to as f64 / from as f64).min(1.0) * 0.97;
    let half = ZEROS / cutoff;
    let out_len = ((samples.len() as f64) / step).round() as usize;
    let sinc = |x: f64| {
        if x.abs() < 1e-9 {
            1.0
        } else {
            let px = std::f64::consts::PI * x;
            px.sin() / px
        }
    };
    (0..out_len)
        .map(|i| {
            let center = i as f64 * step;
            let lo = (center - half).ceil().max(0.0) as usize;
            let hi = ((center + half).floor() as usize).min(samples.len() - 1);
            let (mut acc, mut norm) = (0.0f64, 0.0f64);
            for (j, &s) in samples.iter().enumerate().take(hi + 1).skip(lo) {
                let x = j as f64 - center;
                // Blackman window over ±half.
                let w = 0.42
                    + 0.5 * (std::f64::consts::PI * x / half).cos()
                    + 0.08 * (2.0 * std::f64::consts::PI * x / half).cos();
                let k = cutoff * sinc(cutoff * x) * w;
                acc += s as f64 * k;
                norm += k;
            }
            if norm.abs() > 1e-12 {
                (acc / norm) as f32
            } else {
                0.0
            }
        })
        .collect()
}

/// The output rate the PC and the Web mix at, for decoded-size estimates
/// (cpal's and Web Audio's usual default).
pub const OUTPUT_RATE: u32 = 48_000;
/// RAM a PC stream holds while it plays: half a second of f32 ring at
/// [`OUTPUT_RATE`] (96 000 B) and lewton's decoder state; the peak the
/// 10-minute test measures (`platform-pc/tests/audio_stream.rs`) rounded up.
pub const PC_STREAM_BYTES: usize = 320 * 1024;
/// The 3DS streams through three wave buffers of [`N3DS_CHUNK_FRAMES`].
pub const N3DS_STREAM_BUFFERS: usize = 3;
/// RDRAM an open `wav64` holds (its header and state; the samples stay on
/// the cartridge): castle64's self-test measures 12 128 B for 12.
pub const N64_OPEN_BYTES: usize = 1_011;
/// RDRAM the mixer's sample buffer takes on a channel that plays: 48 032 B
/// measured for castle64's 44.1 kHz theme (the self-test's `TRINO_AUDIO`).
pub const N64_CHANNEL_BYTES: usize = 48_032;

/// What one baked sound costs its platform at run time: `cargo xtask audio
/// inspect|report`, the editor's inspector and its audio budgets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    /// Bytes on the medium: ROM (N64), RomFS (3DS), asset file (PC), pack
    /// download (Web).
    pub bytes: usize,
    /// RAM held once loaded: decoded samples, the encoded track a stream
    /// decodes from, or (N64) an open wav64's state.
    pub resident: usize,
    /// More RAM per playback while it plays (stream buffers; the N64
    /// mixer's sample buffer).
    pub playing: usize,
}

impl Encoded {
    /// The PC and the Web decode a sound that loads whole to f32 at the
    /// device rate ([`OUTPUT_RATE`]) and keep a stream's encoded file plus a
    /// ring; the 3DS keeps a whole `T3AU`'s data in linear memory and three
    /// chunks of a stream; every N64 wav64 plays from the cartridge.
    pub fn footprint(&self) -> Footprint {
        let bytes = self.bytes;
        let stream = self.settings.stream;
        let (resident, playing) = match self.platform {
            Platform::Pc | Platform::Web if stream => (bytes, PC_STREAM_BYTES),
            Platform::Pc | Platform::Web => (
                self.frames * OUTPUT_RATE as usize / self.rate.max(1) as usize * 4,
                0,
            ),
            Platform::N3ds if stream => {
                let chunk = N3DS_CHUNK_FRAMES
                    * if self.settings.codec == Codec::DspAdpcm {
                        dspadpcm::BYTES_PER_FRAME
                    } else {
                        dspadpcm::SAMPLES_PER_FRAME * 2
                    };
                (0, N3DS_STREAM_BUFFERS * chunk)
            }
            Platform::N3ds => (bytes.saturating_sub(T3AU_HEADER_LEN), 0),
            Platform::N64 => (N64_OPEN_BYTES, N64_CHANNEL_BYTES),
        };
        Footprint {
            bytes,
            resident,
            playing,
        }
    }

    /// Who decodes it and what that costs.
    pub fn decoder(&self) -> &'static str {
        match (self.platform, self.settings.codec) {
            (Platform::Pc | Platform::Web, Codec::Vorbis) => "CPU: lewton (Rust; wasm on the Web)",
            (Platform::Pc | Platform::Web, _) => "none (PCM)",
            (Platform::N3ds, Codec::DspAdpcm) => "DSP hardware (no CPU)",
            (Platform::N3ds, _) => "none (PCM, DSP)",
            (Platform::N64, Codec::Vadpcm) => "RSP (libdragon's mixer)",
            (Platform::N64, Codec::Opus) => "CPU + RSP (libdragon's Opus; IMDCT on the RSP)",
            (Platform::N64, _) => "none (PCM, libdragon's mixer)",
        }
    }
}

/// What one sound costs one platform: `cargo xtask audio inspect|report`
/// and the editor's audio inspector.
#[derive(Clone, Debug)]
pub struct AudioReport {
    pub logical: String,
    pub encoded: Encoded,
    /// Why this codec (`auto`'s default, or the manifest's choice).
    pub why: String,
    /// Bytes on the medium: ROM (N64), RomFS (3DS), asset file (PC), pack
    /// download (Web).
    pub bytes: usize,
    /// RAM the sound holds once loaded (decoded samples, or the encoded
    /// track a stream decodes from; on the N64 an open wav64's state: every
    /// wav64 streams from the cartridge).
    pub resident_bytes: usize,
    /// More RAM while it plays (stream buffers).
    pub playing_bytes: usize,
    /// Who decodes it and what that costs.
    pub decode: &'static str,
    /// Signal-to-noise ratio of what plays against the master, dB.
    pub snr_db: Option<f64>,
    /// N64: what one playing voice of it adds to a 60 Hz frame, µs
    /// (`trino_core::perf::N64_COST.audio_us`, measured in ares; the RSP
    /// mixes while the CPU waits).
    pub n64_us_per_frame: Option<u32>,
}

impl AudioReport {
    /// Bytes against 16-bit PCM at the same rate (1.0 = no saving).
    pub fn ratio(&self) -> f64 {
        self.bytes as f64 / self.encoded.pcm16_bytes().max(1) as f64
    }
}

fn report_for(
    logical: &str,
    platform: Platform,
    settings: &AudioSettings,
    master: &Master,
    id: u32,
) -> Result<AudioReport, String> {
    let enc = encode(platform, settings, master, id)?;
    let why = if settings.auto {
        format!("auto: {}'s default", platform.key())
    } else {
        format!("the manifest asks for {}", settings.codec.label())
    };
    let Footprint {
        resident, playing, ..
    } = enc.footprint();
    let decode = enc.decoder();
    let n64_us_per_frame = (platform == Platform::N64).then(|| {
        let cost = &trino_core::perf::N64_COST;
        // libdragon's Opus always plays at 48 kHz.
        let (rate, opus) = match settings.codec {
            Codec::Opus => (48_000, 1),
            _ => (enc.rate, 0),
        };
        cost.audio_us(&[rate], opus, 60.0) - cost.audio_us(&[], 0, 60.0)
    });
    Ok(AudioReport {
        logical: logical.to_string(),
        n64_us_per_frame,
        snr_db: enc.snr_db(master),
        bytes: enc.bytes,
        encoded: enc,
        why,
        resident_bytes: resident,
        playing_bytes: playing,
        decode,
    })
}

/// The master and settings behind `sounds/x` or `music/x`.
fn audio_decl<'m>(
    manifest: &'m crate::Manifest,
    logical: &str,
) -> Result<(&'m SoundDecl, bool), String> {
    let (decl, music) = match logical.split_once('/') {
        Some(("sounds", name)) => (manifest.sounds.get(name), false),
        Some(("music", name)) => (manifest.music.get(name), true),
        _ => {
            return Err(format!(
                "`{logical}`: expected sounds/<name> or music/<name>"
            ));
        }
    };
    decl.map(|d| (d, music))
        .ok_or_else(|| format!("`{logical}` is not in the manifest"))
}

/// Every platform's report for one sound (`audio inspect`).
pub fn inspect(
    assets_root: &std::path::Path,
    logical: &str,
    platforms: &[Platform],
) -> Result<Vec<AudioReport>, String> {
    let manifest = crate::Manifest::load(&assets_root.join("manifest.toml"))?;
    let (decl, music) = audio_decl(&manifest, logical)?;
    let id = trino_core::asset_id(logical);
    platforms
        .iter()
        .map(|&platform| {
            let source = crate::resolve::resolve_source(assets_root, platform, &decl.file)?;
            let master = crate::bake::decode_master(&source)?;
            let settings = decl
                .audio_settings(music, platform)
                .map_err(|e| format!("`{logical}`: {e}"))?;
            report_for(logical, platform, &settings, &master, id)
        })
        .collect()
}

/// One sound of `audio report`: its logical path and each platform's
/// report, or why its bake would fail.
pub type ProjectSound = (String, Result<Vec<AudioReport>, String>);

/// Every sound and track of the project on each of `platforms`, in
/// manifest order (`audio report` sorts them), or why one fails.
pub fn report_project(
    assets_root: &std::path::Path,
    platforms: &[Platform],
) -> Result<Vec<ProjectSound>, String> {
    let manifest = crate::Manifest::load(&assets_root.join("manifest.toml"))?;
    let logicals = manifest
        .sounds
        .keys()
        .map(|n| format!("sounds/{n}"))
        .chain(manifest.music.keys().map(|n| format!("music/{n}")));
    Ok(logicals
        .map(|l| {
            let r = inspect(assets_root, &l, platforms);
            (l, r)
        })
        .collect())
}

/// A mono 16-bit WAV of `samples` (`audio preview`, the editor's players).
pub fn write_wav(path: &std::path::Path, rate: u32, samples: &[f32]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w =
        hound::WavWriter::create(path, spec).map_err(|e| format!("{}: {e}", path.display()))?;
    for s in to_pcm16(samples) {
        w.write_sample(s).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// `assets/audio.toml`: the project's mix — each bus's starting volume and
/// the duck rules. The editor's Mixer panel writes it ([`MixFile::to_toml`]);
/// the bake turns it into the `audio/mixer` asset (`TMIX`). A rewrite keeps
/// the comment lines at the top of the file and right above `[buses]` and
/// each `[[duck]]`; comments anywhere else don't survive it.
///
/// ```toml
/// version = 1
/// [buses]
/// music = 0.8
/// [[duck]]
/// trigger = "voice"
/// target = "music"
/// level = 0.4
/// attack_ms = 150
/// release_ms = 300
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixFile {
    pub version: u32,
    /// Bus label → volume 0..=1 (missing buses: 1).
    #[serde(default)]
    pub buses: BTreeMap<String, f32>,
    #[serde(default)]
    pub duck: Vec<DuckDecl>,
    /// The file's leading comment lines (`#` included), kept on a rewrite.
    #[serde(skip)]
    pub header: String,
    /// The comment lines right above `[buses]`.
    #[serde(skip)]
    pub buses_comment: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuckDecl {
    pub trigger: String,
    pub target: String,
    pub level: f32,
    #[serde(default)]
    pub attack_ms: u16,
    #[serde(default)]
    pub release_ms: u16,
    /// The comment lines right above this `[[duck]]`.
    #[serde(skip)]
    pub comment: String,
}

impl MixFile {
    /// The file name under `assets/`.
    pub const FILE: &'static str = "audio.toml";

    pub fn parse(text: &str) -> Result<MixFile, String> {
        let mut f: MixFile = toml::from_str(text).map_err(|e| e.to_string())?;
        f.config()?;
        // Comment blocks, by the line that follows them.
        let (mut block, mut leading, mut ducks) = (String::new(), true, 0);
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with('#') {
                block.push_str(t);
                block.push('\n');
                continue;
            }
            if t.is_empty() {
                continue;
            }
            let comment = std::mem::take(&mut block);
            if leading {
                leading = false;
                f.header = comment;
            } else if t == "[buses]" {
                f.buses_comment = comment;
            } else if t == "[[duck]]" {
                if let Some(d) = f.duck.get_mut(ducks) {
                    d.comment = comment;
                }
                ducks += 1;
            }
        }
        Ok(f)
    }

    /// The validated mix.
    pub fn config(&self) -> Result<MixerConfig, String> {
        if self.version != 1 {
            return Err(format!("version {} (expected 1)", self.version));
        }
        let mut cfg = MixerConfig::default();
        for (label, &v) in &self.buses {
            let bus = Bus::from_label(label).ok_or_else(|| {
                format!("unknown bus `{label}` (master, music, sfx, voice, ambience, ui)")
            })?;
            if !(0.0..=1.0).contains(&v) {
                return Err(format!("bus `{label}`: volume {v} (0 to 1)"));
            }
            cfg.volumes[bus.index()] = v;
        }
        if self.duck.len() > MAX_DUCK_RULES {
            return Err(format!(
                "{} duck rules (at most {MAX_DUCK_RULES})",
                self.duck.len()
            ));
        }
        let mut probe = trino_core::BusMixer::new();
        for (i, d) in self.duck.iter().enumerate() {
            let bus = |l: &str| {
                Bus::from_label(l).ok_or_else(|| format!("duck {}: unknown bus `{l}`", i + 1))
            };
            let rule = DuckRule::new(
                bus(&d.trigger)?,
                bus(&d.target)?,
                d.level,
                d.attack_ms,
                d.release_ms,
            );
            if !(0.0..1.0).contains(&d.level) {
                return Err(format!("duck {}: level {} (0 to below 1)", i + 1, d.level));
            }
            if !probe.set_duck(rule) {
                return Err(format!(
                    "duck {}: `{}` can't duck `{}` (not itself, not the master)",
                    i + 1,
                    d.trigger,
                    d.target
                ));
            }
            if cfg
                .ducks
                .iter()
                .flatten()
                .any(|r| r.trigger == rule.trigger && r.target == rule.target)
            {
                return Err(format!(
                    "duck {}: a second `{}` → `{}` rule",
                    i + 1,
                    d.trigger,
                    d.target
                ));
            }
            cfg.ducks[i] = Some(rule);
        }
        Ok(cfg)
    }

    pub fn from_config(cfg: &MixerConfig) -> MixFile {
        let round = |v: f32| (v * 1000.0).round() / 1000.0;
        MixFile {
            version: 1,
            buses: Bus::ALL
                .iter()
                .map(|b| (b.label().to_string(), round(cfg.volumes[b.index()])))
                .collect(),
            duck: cfg
                .ducks
                .iter()
                .flatten()
                .map(|r| DuckDecl {
                    trigger: r.trigger.label().into(),
                    target: r.target.label().into(),
                    level: round(r.level),
                    attack_ms: r.attack_ms,
                    release_ms: r.release_ms,
                    comment: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    /// The file as the editor writes it (buses in mixer order), with the
    /// comments [`MixFile::parse`] kept.
    pub fn to_toml(&self) -> String {
        let mut s = if self.header.is_empty() {
            "# The project's mix (#53): each bus's starting volume and the duck rules.\n\
             # Written by the editor's Mixer panel; edit by hand just as well.\n"
                .to_string()
        } else {
            self.header.clone()
        };
        s.push_str("version = 1\n\n");
        s.push_str(&self.buses_comment);
        s.push_str("[buses]\n");
        for b in Bus::ALL {
            if let Some(v) = self.buses.get(b.label()) {
                s.push_str(&format!("{} = {v}\n", b.label()));
            }
        }
        for d in &self.duck {
            s.push_str(&format!(
                "\n{}[[duck]]\ntrigger = \"{}\"\ntarget = \"{}\"\nlevel = {}\nattack_ms = {}\nrelease_ms = {}\n",
                d.comment, d.trigger, d.target, d.level, d.attack_ms, d.release_ms
            ));
        }
        s
    }

    /// The `TMIX` blob.
    pub fn bake(&self) -> Result<Vec<u8>, String> {
        let cfg = self.config()?;
        let mut out = vec![0u8; cfg.encoded_len()];
        cfg.encode(&mut out);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(text: &str) -> SoundDecl {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn n64_lengths_are_whole_blocks() {
        let ramp: Vec<i16> = (0..37).collect();
        assert_eq!(n64_pcm(ramp.clone(), false).unwrap().len(), 48, "padded");
        assert_eq!(n64_pcm(ramp, true).unwrap().len(), 32, "trimmed");
        // A 6-sample loop repeats whole into 48 samples: the seam stays.
        let short = n64_pcm(vec![1, 2, 3, 4, 5, 6], true).unwrap();
        assert_eq!(short.len(), 48);
        assert!(short.chunks(6).all(|c| c == [1, 2, 3, 4, 5, 6]));
        assert!(n64_pcm(Vec::new(), true).is_err());
        assert!(n64_pcm(Vec::new(), false).unwrap().is_empty());
    }

    #[test]
    fn settings_default_per_platform_and_validate() {
        let d = decl("file = \"x.wav\"");
        let auto = |p| AudioSettings::from_decl(&d, false, p).unwrap();
        assert_eq!(auto(Platform::N64).codec, Codec::Vadpcm);
        assert_eq!(auto(Platform::N3ds).codec, Codec::DspAdpcm);
        assert_eq!(auto(Platform::Pc).codec, Codec::Vorbis);
        assert_eq!(auto(Platform::Web).codec, Codec::Vorbis);
        assert!(auto(Platform::Pc).auto && !auto(Platform::Pc).stream);
        assert_eq!(auto(Platform::Pc).bus, Bus::Sfx);
        let music = AudioSettings::from_decl(&d, true, Platform::Pc).unwrap();
        assert!(music.stream && music.looped && music.bus == Bus::Music);

        let d = decl(
            "file = \"x.wav\"\ncodec = { n64 = \"opus\", 3ds = \"pcm\", pc = \"pcm\" }\n\
             sample_rate = { n64 = 22050 }\nbus = \"voice\"\nquality = 7\nstream = false",
        );
        let s = |p| AudioSettings::from_decl(&d, true, p).unwrap();
        assert_eq!(s(Platform::N64).codec, Codec::Opus);
        assert_eq!(s(Platform::N64).sample_rate, Some(22050));
        assert_eq!(s(Platform::N3ds).codec, Codec::Pcm);
        assert_eq!(s(Platform::Web).codec, Codec::Pcm, "the Web takes the PC's");
        assert_eq!(s(Platform::Pc).sample_rate, None);
        assert!(!s(Platform::Pc).auto && !s(Platform::Pc).stream);
        assert_eq!(s(Platform::Pc).bus, Bus::Voice);

        let err = |t: &str, p| AudioSettings::from_decl(&decl(t), false, p).unwrap_err();
        assert!(
            err("file = \"x\"\ncodec = \"dspadpcm\"", Platform::Pc).contains("doesn't play on pc")
        );
        assert!(err("file = \"x\"\ncodec = \"mp3\"", Platform::Pc).contains("codec `mp3`"));
        assert!(err("file = \"x\"\ncodec = { wii = \"pcm\" }", Platform::Pc).contains("`wii`"));
        assert!(err("file = \"x\"\nquality = 11", Platform::Pc).contains("quality"));
        assert!(err("file = \"x\"\nsample_rate = 100", Platform::Pc).contains("sample_rate"));
        assert!(err("file = \"x\"\nbus = \"master\"", Platform::Pc).contains("bus"));
    }

    #[test]
    fn resample_keeps_a_tone_and_its_length() {
        let tone: Vec<f32> = (0..44_100)
            .map(|i| (i as f32 / 44_100.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5)
            .collect();
        let half = resample(&tone, 44_100, 22_050);
        assert_eq!(half.len(), 22_050);
        let want: Vec<f32> = (0..22_050)
            .map(|i| (i as f32 / 22_050.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5)
            .collect();
        // Away from the edges the band-limited result is the tone itself.
        assert!(snr_db(&want[100..22_000], &half[100..22_000]) > 60.0);
        // Above the new Nyquist it is gone (a 15 kHz tone at 22.05 kHz).
        let high: Vec<f32> = (0..44_100)
            .map(|i| (i as f32 / 44_100.0 * 15_000.0 * std::f32::consts::TAU).sin())
            .collect();
        let gone = resample(&high, 44_100, 22_050);
        let rms = (gone[100..22_000].iter().map(|s| s * s).sum::<f32>() / 21_900.0).sqrt();
        assert!(rms < 0.01, "aliasing: rms {rms}");
        assert_eq!(resample(&tone, 100, 100), tone);
    }

    #[test]
    fn pcm16_round_trip_and_snr() {
        let x = [0.0, 0.5, -0.5, 1.0, -1.0, 2.0];
        let p = to_pcm16(&x);
        assert_eq!(p, [0, 16384, -16384, 32767, -32767, 32767]);
        assert!(snr_db(&x[..5], &from_pcm16(&p)[..5]) > 90.0);
        assert_eq!(snr_db(&[0.5, 0.5], &[0.5, 0.5]), f64::INFINITY);
        assert!((snr_db(&[1.0, 1.0], &[0.9, 0.9]) - 20.0).abs() < 1e-3);
    }

    #[test]
    fn mix_files_validate_round_trip_and_bake() {
        let text = "version = 1\n[buses]\nmusic = 0.8\nui = 0.5\n\n[[duck]]\ntrigger = \"voice\"\ntarget = \"music\"\nlevel = 0.4\nattack_ms = 150\nrelease_ms = 300\n";
        let f = MixFile::parse(text).unwrap();
        let cfg = f.config().unwrap();
        assert_eq!(cfg.volumes[Bus::Music.index()], 0.8);
        assert_eq!(cfg.volumes[Bus::Sfx.index()], 1.0);
        assert_eq!(
            cfg.ducks[0],
            Some(DuckRule::new(Bus::Voice, Bus::Music, 0.4, 150, 300))
        );
        let written = MixFile::from_config(&cfg).to_toml();
        assert_eq!(MixFile::parse(&written).unwrap().config().unwrap(), cfg);
        // The editor's rewrite keeps the comments at the top and above the
        // tables, and the project's own file reads back the same.
        let commented = format!(
            "# Mine.\n\n# Two lines.\n{}",
            text.replace("[buses]", "# Loud.\n[buses]")
                .replace("[[duck]]", "# Talk.\n[[duck]]")
        );
        let f = MixFile::parse(&commented).unwrap();
        assert_eq!(f.header, "# Mine.\n# Two lines.\n");
        let again = f.to_toml();
        assert!(
            again.starts_with("# Mine.\n# Two lines.\nversion = 1"),
            "{again}"
        );
        assert!(
            again.contains("# Loud.\n[buses]") && again.contains("# Talk.\n[[duck]]"),
            "{again}"
        );
        assert_eq!(MixFile::parse(&again).unwrap(), f);
        let project = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/audio.toml"
        ))
        .unwrap();
        let p = MixFile::parse(&project).unwrap();
        assert_eq!(MixFile::parse(&p.to_toml()).unwrap(), p);
        assert!(p.to_toml().contains("# Dialogue pulls the music down"));
        assert_eq!(MixerConfig::parse(&f.bake().unwrap()).unwrap(), cfg);
        for (bad, why) in [
            ("version = 2", "version"),
            ("version = 1\n[buses]\ndrums = 1", "drums"),
            ("version = 1\n[buses]\nmusic = 1.5", "volume"),
            (
                "version = 1\n[[duck]]\ntrigger = \"music\"\ntarget = \"music\"\nlevel = 0.5",
                "itself",
            ),
            (
                "version = 1\n[[duck]]\ntrigger = \"voice\"\ntarget = \"music\"\nlevel = 1.0",
                "level",
            ),
            (
                "version = 1\n[[duck]]\ntrigger = \"voice\"\ntarget = \"music\"\nlevel = 0.5\n[[duck]]\ntrigger = \"voice\"\ntarget = \"music\"\nlevel = 0.3",
                "second",
            ),
        ] {
            let e = MixFile::parse(bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
    }
}
