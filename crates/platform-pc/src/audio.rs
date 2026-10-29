//! cpal audio backend with a small software mixer.
//!
//! Sounds are mono `f32` sample buffers at the device sample rate (resampled
//! at load time). The mixer ([`mixer`]) is pure and unit-tested; cpal only
//! drains it. It mixes in **stereo**: every voice carries left/right gains
//! (volume + pan, [`trino_core::SoundParams::gains`]), music ducks while a
//! stinger plays, and voices have ids so games can steer them.
//!
//! **Buses** (issue #53): every voice plays on a [`trino_core::Bus`]; the
//! mixer's [`trino_core::BusMixer`] gives each bus its gain (master × bus ×
//! ducking, mutes) and advances the duck ramps by each buffer's length.
//!
//! **Baked sounds** come as `TSN2` blobs ([`PcAudio::upload_sound_blob`]):
//! PCM16 or Ogg Vorbis. Sounds are decoded once at load; anything marked
//! `stream` (music by default) keeps only its encoded bytes and plays
//! through [`stream`]: a worker thread decodes into a ring per playback, so
//! RAM stays flat however long the track.
//!
//! If no output device exists (common on CI runners) the backend degrades to
//! a silent no-op instead of failing — audio must never break a smoke test.

pub mod stream;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait};
use trino_core::{
    Audio, AudioStats, Bus, BusSettings, DuckRule, MusicId, SoundBlob, SoundCodec, SoundId,
    SoundParams, VoiceId,
};

use stream::{Job, StreamSource, Streamer};

pub mod mixer {
    //! Pure stereo mixer (unit-tested without an audio device).

    use std::sync::Arc;

    use trino_core::{Bus, BusMixer, BusSet};

    use super::stream::Ring;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Kind {
        Sfx,
        Music,
        Stinger,
    }

    /// A voice's samples: in memory, or a stream's ring.
    pub enum Source {
        Pcm(Arc<Vec<f32>>),
        Stream(Arc<Ring>),
    }

    pub struct Voice {
        pub id: u32,
        pub source: Source,
        pub cursor: usize,
        pub looped: bool,
        pub kind: Kind,
        pub bus: Bus,
        /// Left / right gains.
        pub gains: (f32, f32),
    }

    impl Voice {
        /// A voice over samples in memory.
        pub fn pcm(
            id: u32,
            data: Arc<Vec<f32>>,
            looped: bool,
            kind: Kind,
            bus: Bus,
            gains: (f32, f32),
        ) -> Voice {
            Voice {
                id,
                source: Source::Pcm(data),
                cursor: 0,
                looped,
                kind,
                bus,
                gains,
            }
        }

        fn over(&self) -> bool {
            match &self.source {
                Source::Pcm(d) => !self.looped && self.cursor >= d.len(),
                Source::Stream(r) => r.done(),
            }
        }
    }

    impl Drop for Voice {
        fn drop(&mut self) {
            // A stopped or finished stream lets its decoder go.
            if let Source::Stream(r) = &self.source {
                r.close();
            }
        }
    }

    /// Music level while a stinger plays.
    pub const DUCK: f32 = 0.3;

    /// The voices and the buses, mixed into interleaved stereo.
    pub struct Mixer {
        pub voices: Vec<Voice>,
        pub buses: BusMixer,
        pub rate: u32,
        /// Stream buffers found empty while their decoder was still going.
        pub underruns: u32,
        /// Samples the streams delivered.
        pub streamed: u64,
        scratch: Vec<f32>,
    }

    impl Mixer {
        pub fn new(rate: u32) -> Self {
            Mixer {
                voices: Vec::new(),
                buses: BusMixer::new(),
                rate: rate.max(1),
                underruns: 0,
                streamed: 0,
                scratch: Vec::new(),
            }
        }

        /// The buses something plays on.
        pub fn sounding(&self) -> BusSet {
            let mut s = BusSet::EMPTY;
            for v in &self.voices {
                s.insert(v.bus);
            }
            s
        }

        /// Mix every voice into interleaved stereo `out` (`[l, r, l, r,
        /// ...]`): advance the duck ramps by its length, apply each bus's
        /// gain, advance cursors, drop finished voices, clamp to -1..1.
        pub fn mix(&mut self, out: &mut [f32]) {
            let frames = out.len() / 2;
            let sounding = self.sounding();
            self.buses
                .update(frames as f32 / self.rate as f32, sounding);
            let gains = self.buses.gains();
            out.fill(0.0);
            let ducked = self.voices.iter().any(|v| v.kind == Kind::Stinger);
            for voice in self.voices.iter_mut() {
                let k = gains[voice.bus.index()]
                    * if voice.kind == Kind::Music && ducked {
                        DUCK
                    } else {
                        1.0
                    };
                let (gl, gr) = (voice.gains.0 * k, voice.gains.1 * k);
                match &voice.source {
                    Source::Pcm(data) => {
                        for frame in out.as_chunks_mut::<2>().0 {
                            if voice.cursor >= data.len() {
                                if voice.looped && !data.is_empty() {
                                    voice.cursor = 0;
                                } else {
                                    break;
                                }
                            }
                            let s = data[voice.cursor];
                            frame[0] += s * gl;
                            frame[1] += s * gr;
                            voice.cursor += 1;
                        }
                    }
                    Source::Stream(ring) => {
                        self.scratch.resize(frames, 0.0);
                        let n = ring.pop(&mut self.scratch[..frames]);
                        let pairs = out.as_chunks_mut::<2>().0.iter_mut();
                        for (frame, s) in pairs.zip(&self.scratch[..n]) {
                            frame[0] += s * gl;
                            frame[1] += s * gr;
                        }
                        self.streamed += n as u64;
                        if n < frames && !ring.finished() {
                            self.underruns += 1;
                        }
                    }
                }
            }
            self.voices.retain(|v| !v.over());
            for slot in out.iter_mut() {
                *slot = slot.clamp(-1.0, 1.0);
            }
        }
    }
}

use mixer::{Kind, Mixer, Source, Voice};

struct Shared {
    mixer: Mixer,
    next_id: u32,
}

impl Shared {
    fn push(
        &mut self,
        source: Source,
        looped: bool,
        kind: Kind,
        bus: Bus,
        gains: (f32, f32),
    ) -> u32 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        self.mixer.voices.push(Voice {
            id,
            source,
            cursor: 0,
            looped,
            kind,
            bus,
            gains,
        });
        id
    }
}

/// Most simultaneous steerable voices (like a console mixer's channels).
const MAX_VOICES: usize = 24;

/// A loaded sound or track.
#[derive(Clone)]
enum ClipData {
    /// Decoded, at the device rate.
    Pcm(Arc<Vec<f32>>),
    /// Kept encoded, decoded as it plays.
    Stream(StreamSource),
}

#[derive(Clone)]
struct Clip {
    data: ClipData,
    bus: Bus,
}

impl Clip {
    fn bytes(&self) -> usize {
        match &self.data {
            ClipData::Pcm(d) => d.len() * 4,
            ClipData::Stream(s) => s.bytes(),
        }
    }
}

pub struct PcAudio {
    shared: Arc<Mutex<Shared>>,
    sounds: HashMap<u32, Clip>,
    music: HashMap<u32, Clip>,
    sample_rate: u32,
    /// The decoder thread; `None` offline, where [`PcAudio::render`] fills
    /// the streams itself.
    streamer: Option<Streamer>,
    inline_jobs: Vec<Job>,
    _stream: Option<cpal::Stream>,
}

impl PcAudio {
    pub fn new() -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            mixer: Mixer::new(44_100),
            next_id: 0,
        }));

        let (stream, sample_rate) = match Self::open_stream(shared.clone()) {
            Some((stream, rate)) => (Some(stream), rate),
            None => {
                eprintln!("trino-audio: no output device, running silent");
                (None, 44_100)
            }
        };
        if let Ok(mut s) = shared.lock() {
            s.mixer.rate = sample_rate;
        }

        PcAudio {
            shared,
            sounds: HashMap::new(),
            music: HashMap::new(),
            sample_rate,
            streamer: Some(Streamer::spawn()),
            inline_jobs: Vec::new(),
            _stream: stream,
        }
    }

    /// No device and no threads: [`PcAudio::render`] pulls the mix (tests,
    /// tools that write the output to a file).
    pub fn new_offline(sample_rate: u32) -> Self {
        PcAudio {
            shared: Arc::new(Mutex::new(Shared {
                mixer: Mixer::new(sample_rate),
                next_id: 0,
            })),
            sounds: HashMap::new(),
            music: HashMap::new(),
            sample_rate,
            streamer: None,
            inline_jobs: Vec::new(),
            _stream: None,
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
                        s.mixer.mix(&mut stereo);
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

    /// Mono samples at the device rate, played on the effects bus.
    pub fn upload_sound(&mut self, id: SoundId, samples: Vec<f32>) {
        self.sounds.insert(
            id.0,
            Clip {
                data: ClipData::Pcm(Arc::new(samples)),
                bus: Bus::Sfx,
            },
        );
    }

    /// Mono samples at the device rate, played on the music bus.
    pub fn upload_music(&mut self, id: MusicId, samples: Vec<f32>) {
        self.music.insert(
            id.0,
            Clip {
                data: ClipData::Pcm(Arc::new(samples)),
                bus: Bus::Music,
            },
        );
    }

    /// A baked `TSN2` sound: decoded now (and resampled to the device) unless
    /// it streams; it plays on its baked bus unless the call names one.
    pub fn upload_sound_blob(&mut self, id: SoundId, blob: &[u8]) -> Result<(), String> {
        let clip = self.clip(blob)?;
        self.sounds.insert(id.0, clip);
        Ok(())
    }

    /// A baked `TSN2` track (music or a stinger).
    pub fn upload_music_blob(&mut self, id: MusicId, blob: &[u8]) -> Result<(), String> {
        let clip = self.clip(blob)?;
        self.music.insert(id.0, clip);
        Ok(())
    }

    /// Free a sound (its asset group unloaded, issue #56); returns the
    /// bytes it held. A voice still playing it keeps its samples until it
    /// ends; new plays of the handle are silent.
    pub fn remove_sound(&mut self, id: SoundId) -> usize {
        self.sounds.remove(&id.0).map_or(0, |c| c.bytes())
    }

    /// Free a music track or stinger ([`PcAudio::remove_sound`]).
    pub fn remove_music(&mut self, id: MusicId) -> usize {
        self.music.remove(&id.0).map_or(0, |c| c.bytes())
    }

    /// RAM a loaded sound or track holds (0 when not loaded).
    pub fn clip_bytes(&self, id: u32, music: bool) -> usize {
        let map = if music { &self.music } else { &self.sounds };
        map.get(&id).map_or(0, Clip::bytes)
    }

    fn clip(&self, blob: &[u8]) -> Result<Clip, String> {
        let b = SoundBlob::parse(blob).map_err(String::from)?;
        let data = match (b.codec, b.stream) {
            (SoundCodec::Vorbis, true) => ClipData::Stream(StreamSource::Vorbis {
                bytes: b.payload.into(),
                frames: b.frames as usize,
            }),
            (SoundCodec::Pcm16, true) => ClipData::Stream(StreamSource::Pcm {
                rate: b.sample_rate,
                data: Arc::new(pcm16(b.payload)),
            }),
            (codec, false) => {
                let (rate, mut samples) = decode_blob(codec, b.sample_rate, b.payload)?;
                samples.truncate(b.frames as usize);
                ClipData::Pcm(Arc::new(resample(&samples, rate, self.sample_rate)))
            }
        };
        Ok(Clip { data, bus: b.bus })
    }

    /// A playable source for `clip`: its samples, or a new stream.
    fn source(&mut self, clip: &Clip, looped: bool) -> Option<Source> {
        match &clip.data {
            ClipData::Pcm(d) => Some(Source::Pcm(d.clone())),
            ClipData::Stream(src) => {
                // Half a second of output per stream.
                let capacity = (self.sample_rate / 2).max(1024) as usize;
                let (mut job, ring) = Job::start(src, self.sample_rate, looped, capacity).ok()?;
                // Prefill here, so the first buffer never runs dry.
                if job.fill() {
                    match &self.streamer {
                        Some(s) => {
                            if let Err(job) = s.send(job) {
                                self.inline_jobs.push(*job);
                            }
                        }
                        None => self.inline_jobs.push(job),
                    }
                }
                Some(Source::Stream(ring))
            }
        }
    }

    /// Pull `out` (interleaved stereo) from the mixer — what the device
    /// callback does. Offline backends fill their streams first.
    pub fn render(&mut self, out: &mut [f32]) {
        self.inline_jobs.retain_mut(Job::fill);
        if let Ok(mut s) = self.shared.lock() {
            s.mixer.mix(out);
        }
    }

    /// The buses' volumes and mutes (what the runner persists).
    pub fn bus_settings(&self) -> BusSettings {
        self.shared
            .lock()
            .map(|s| s.mixer.buses.settings())
            .unwrap_or_default()
    }

    fn with_mixer<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> Option<R> {
        self.shared.lock().ok().map(|mut s| f(&mut s))
    }
}

impl Default for PcAudio {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode a `TSN2` payload to mono f32: `(rate, samples)`.
pub fn decode_blob(
    codec: SoundCodec,
    rate: u32,
    payload: &[u8],
) -> Result<(u32, Vec<f32>), String> {
    decode_blob_limit(codec, rate, payload, usize::MAX)
}

/// [`decode_blob`] that stops once `limit` samples are out (the last
/// packet's may pass it): the `tsn2_decode` fuzz target decodes a bounded
/// prefix of every payload (issue #121), so its time goes to the parsers,
/// not to minutes of valid audio.
pub fn decode_blob_limit(
    codec: SoundCodec,
    rate: u32,
    payload: &[u8],
    limit: usize,
) -> Result<(u32, Vec<f32>), String> {
    match codec {
        SoundCodec::Pcm16 => {
            let n = payload.len().min(limit.saturating_mul(2));
            Ok((rate, pcm16(&payload[..n])))
        }
        SoundCodec::Vorbis => {
            use lewton::inside_ogg::OggStreamReader;
            use lewton::samples::InterleavedSamples;
            // lewton reserves what a codebook's header says before reading
            // it: a damaged file asked for 40 GB (`tsn2_decode`, #121).
            trino_core::vorbis::check(payload).map_err(|e| format!("Vorbis: {e:?}"))?;
            let mut r = OggStreamReader::new(std::io::Cursor::new(payload))
                .map_err(|e| format!("Vorbis: {e}"))?;
            let rate = r.ident_hdr.audio_sample_rate;
            let mut out = Vec::new();
            while out.len() < limit
                && let Some(p) = r
                    .read_dec_packet_generic::<InterleavedSamples<f32>>()
                    .map_err(|e| format!("Vorbis: {e}"))?
            {
                let c = p.channel_count.max(1);
                out.extend(
                    p.samples
                        .chunks(c)
                        .map(|f| f.iter().sum::<f32>() / c as f32),
                );
            }
            Ok((rate, out))
        }
    }
}

fn pcm16(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b) as f32 / 32767.0)
        .collect()
}

/// Linear resample of mono samples (load time).
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (samples.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let src = i as f64 * ratio;
            let idx = src as usize;
            let frac = (src - idx as f64) as f32;
            let a = samples[idx.min(samples.len() - 1)];
            let b = samples[(idx + 1).min(samples.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// `<save_name>.audio`: one line per bus, `<bus> <volume>` and `muted` when
/// it is (comments with `#`).
pub fn settings_to_text(s: &BusSettings) -> String {
    let mut out = String::from("# Trino audio: bus volume (0..1), `muted` when muted\n");
    for b in Bus::ALL {
        out.push_str(&format!("{} {:.2}", b.label(), s.volumes[b.index()]));
        if s.muted.contains(b) {
            out.push_str(" muted");
        }
        out.push('\n');
    }
    out
}

/// Reads [`settings_to_text`]'s file; unknown or broken lines keep the
/// defaults (a hand-edited file never stops the game).
pub fn settings_from_text(text: &str) -> BusSettings {
    let mut s = BusSettings::default();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(bus), Some(vol)) = (words.next(), words.next()) else {
            continue;
        };
        let (Some(bus), Ok(vol)) = (Bus::from_label(bus), vol.parse::<f32>()) else {
            continue;
        };
        s.volumes[bus.index()] = vol.clamp(0.0, 1.0);
        s.muted.set(bus, words.next() == Some("muted"));
    }
    s
}

impl Audio for PcAudio {
    fn play_sound(&mut self, sound: SoundId) {
        self.play_sound_with(sound, &SoundParams::DEFAULT);
    }

    fn play_sound_with(&mut self, sound: SoundId, params: &SoundParams) {
        let Some(clip) = self.sounds.get(&sound.0).cloned() else {
            return;
        };
        let Some(source) = self.source(&clip, false) else {
            return;
        };
        let bus = params.bus.unwrap_or(clip.bus);
        self.with_mixer(|s| s.push(source, false, Kind::Sfx, bus, params.gains()));
    }

    fn play_voice(
        &mut self,
        sound: SoundId,
        params: &SoundParams,
        looped: bool,
    ) -> Option<VoiceId> {
        let clip = self.sounds.get(&sound.0)?.clone();
        if self.with_mixer(|s| s.mixer.voices.len())? >= MAX_VOICES {
            return None;
        }
        let source = self.source(&clip, looped)?;
        let bus = params.bus.unwrap_or(clip.bus);
        self.with_mixer(|s| VoiceId(s.push(source, looped, Kind::Sfx, bus, params.gains())))
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        self.with_mixer(|s| {
            if let Some(v) = s.mixer.voices.iter_mut().find(|v| v.id == voice.0) {
                v.gains = params.gains();
            }
        });
    }

    fn stop_voice(&mut self, voice: VoiceId) {
        self.with_mixer(|s| s.mixer.voices.retain(|v| v.id != voice.0));
    }

    fn voice_playing(&self, voice: VoiceId) -> bool {
        self.with_mixer(|s| s.mixer.voices.iter().any(|v| v.id == voice.0))
            .unwrap_or(false)
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        let Some(clip) = self.music.get(&music.0).cloned() else {
            return;
        };
        self.with_mixer(|s| s.mixer.voices.retain(|v| v.kind != Kind::Music));
        let Some(source) = self.source(&clip, looped) else {
            return;
        };
        self.with_mixer(|s| s.push(source, looped, Kind::Music, clip.bus, (1.0, 1.0)));
    }

    fn stop_music(&mut self) {
        self.with_mixer(|s| s.mixer.voices.retain(|v| v.kind != Kind::Music));
    }

    fn play_stinger(&mut self, music: MusicId) {
        let Some(clip) = self.music.get(&music.0).cloned() else {
            return;
        };
        self.with_mixer(|s| s.mixer.voices.retain(|v| v.kind != Kind::Stinger));
        let Some(source) = self.source(&clip, false) else {
            return;
        };
        self.with_mixer(|s| s.push(source, false, Kind::Stinger, clip.bus, (1.0, 1.0)));
    }

    fn stinger_playing(&self) -> bool {
        self.with_mixer(|s| s.mixer.voices.iter().any(|v| v.kind == Kind::Stinger))
            .unwrap_or(false)
    }

    fn set_music_volume(&mut self, volume: f32) {
        self.set_bus_volume(Bus::Music, volume);
    }

    fn set_master_volume(&mut self, volume: f32) {
        self.set_bus_volume(Bus::Master, volume);
    }

    fn set_bus_volume(&mut self, bus: Bus, volume: f32) {
        self.with_mixer(|s| s.mixer.buses.set_volume(bus, volume));
    }

    fn bus_volume(&self, bus: Bus) -> f32 {
        self.with_mixer(|s| s.mixer.buses.volume(bus))
            .unwrap_or(1.0)
    }

    fn set_bus_muted(&mut self, bus: Bus, muted: bool) {
        self.with_mixer(|s| s.mixer.buses.set_muted(bus, muted));
    }

    fn bus_muted(&self, bus: Bus) -> bool {
        self.with_mixer(|s| s.mixer.buses.muted(bus))
            .unwrap_or(false)
    }

    fn set_duck(&mut self, rule: DuckRule) {
        self.with_mixer(|s| s.mixer.buses.set_duck(rule));
    }

    fn clear_ducks(&mut self) {
        self.with_mixer(|s| s.mixer.buses.clear_ducks());
    }

    fn bus_duck(&self, bus: Bus) -> f32 {
        self.with_mixer(|s| s.mixer.buses.duck(bus)).unwrap_or(1.0)
    }

    fn audio_stats(&self) -> AudioStats {
        let resident: usize = self
            .sounds
            .values()
            .chain(self.music.values())
            .map(Clip::bytes)
            .sum();
        self.with_mixer(|s| {
            let (mut streams, mut stream_bytes) = (0u16, 0usize);
            for v in &s.mixer.voices {
                if let Source::Stream(r) = &v.source {
                    streams += 1;
                    stream_bytes += r.bytes();
                }
            }
            AudioStats {
                voices: s.mixer.voices.len() as u16,
                streams,
                resident_bytes: resident as u32,
                stream_bytes: stream_bytes as u32,
                underruns: s.mixer.underruns,
                streamed_frames: s.mixer.streamed as u32,
            }
        })
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::mixer::{DUCK, Kind, Mixer, Voice};
    use std::sync::Arc;
    use trino_core::{Bus, DuckRule};

    fn voice(data: Vec<f32>, looped: bool) -> Voice {
        kind_voice(data, looped, Kind::Sfx)
    }

    fn kind_voice(data: Vec<f32>, looped: bool, kind: Kind) -> Voice {
        let bus = if kind == Kind::Sfx {
            Bus::Sfx
        } else {
            Bus::Music
        };
        Voice::pcm(1, Arc::new(data), looped, kind, bus, (1.0, 1.0))
    }

    fn mixer(voices: Vec<Voice>) -> Mixer {
        let mut m = Mixer::new(1000);
        m.voices = voices;
        m
    }

    /// Left channel of an interleaved buffer.
    fn left(out: &[f32]) -> Vec<f32> {
        out.as_chunks::<2>().0.iter().map(|f| f[0]).collect()
    }

    #[test]
    fn mixes_and_advances() {
        let mut m = mixer(vec![
            voice(vec![0.5; 4], false),
            voice(vec![0.25; 2], false),
        ]);
        let mut out = [0.0f32; 8];
        m.mix(&mut out);
        assert_eq!(left(&out), [0.75, 0.75, 0.5, 0.5]);
        assert!(m.voices.is_empty(), "both played to completion");
    }

    #[test]
    fn clamps_to_unit_range() {
        let mut m = mixer(vec![voice(vec![0.9; 2], false), voice(vec![0.9; 2], false)]);
        let mut out = [0.0f32; 4];
        m.mix(&mut out);
        assert_eq!(out, [1.0; 4]);
    }

    #[test]
    fn master_volume_scales() {
        let mut m = mixer(vec![voice(vec![0.8; 2], false)]);
        m.buses.set_volume(Bus::Master, 0.5);
        let mut out = [0.0f32; 4];
        m.mix(&mut out);
        assert_eq!(out, [0.4; 4]);
    }

    #[test]
    fn looped_voice_wraps_and_survives() {
        let mut m = mixer(vec![voice(vec![0.1, 0.2], true)]);
        let mut out = [0.0f32; 10];
        m.mix(&mut out);
        assert_eq!(left(&out), [0.1, 0.2, 0.1, 0.2, 0.1]);
        assert_eq!(m.voices.len(), 1);
    }

    #[test]
    fn empty_looped_voice_does_not_hang() {
        let mut m = mixer(vec![voice(vec![], true)]);
        let mut out = [0.0f32; 8];
        m.mix(&mut out);
        assert_eq!(out, [0.0; 8]);
    }

    #[test]
    fn pan_gains_split_channels() {
        let mut v = voice(vec![1.0; 2], false);
        v.gains = trino_core::SoundParams {
            volume: 0.5,
            pan: 1.0,
            ..trino_core::SoundParams::DEFAULT
        }
        .gains();
        let mut m = mixer(vec![v]);
        let mut out = [0.0f32; 4];
        m.mix(&mut out);
        assert_eq!(out, [0.0, 0.5, 0.0, 0.5], "fully right");
    }

    #[test]
    fn music_ducks_under_a_stinger_and_follows_music_volume() {
        let mut m = mixer(vec![
            kind_voice(vec![0.5; 4], true, Kind::Music),
            kind_voice(vec![0.1; 2], false, Kind::Stinger),
        ]);
        let mut out = [0.0f32; 8];
        m.mix(&mut out);
        let l = left(&out);
        assert!((l[0] - (0.5 * DUCK + 0.1)).abs() < 1e-6, "{l:?}");
        // Stinger done -> next buffer is back to full music.
        m.mix(&mut out);
        assert!((left(&out)[0] - 0.5).abs() < 1e-6);
        m.buses.set_volume(Bus::Music, 0.5);
        m.mix(&mut out);
        assert!((left(&out)[0] - 0.25).abs() < 1e-6, "music volume");
    }

    #[test]
    fn buses_scale_mute_and_duck_their_voices() {
        let mut ui = voice(vec![0.5; 400], true);
        ui.bus = Bus::Ui;
        let mut talk = voice(vec![0.25; 100], false);
        talk.bus = Bus::Voice;
        let music = kind_voice(vec![0.5; 400], true, Kind::Music);
        let mut m = mixer(vec![ui, music]);
        m.buses.set_volume(Bus::Ui, 0.5);
        // 1 kHz here: 10 frames = 10 ms per buffer.
        let mut out = [0.0f32; 20];
        m.mix(&mut out);
        assert!(
            (left(&out)[0] - (0.25 + 0.5)).abs() < 1e-6,
            "ui at half, music full"
        );
        m.buses.set_muted(Bus::Ui, true);
        m.mix(&mut out);
        assert!((left(&out)[0] - 0.5).abs() < 1e-6, "ui muted");
        // Dialogue ducks the music to 0.4 at once.
        assert!(
            m.buses
                .set_duck(DuckRule::new(Bus::Voice, Bus::Music, 0.4, 0, 20))
        );
        m.voices.push(talk);
        m.mix(&mut out);
        let l = left(&out);
        assert!((l[0] - (0.5 * 0.4 + 0.25)).abs() < 1e-6, "{l:?}");
        // Talk over after 100 frames: the music comes back over 20 ms.
        for _ in 0..9 {
            m.mix(&mut out);
        }
        assert!(m.voices.iter().all(|v| v.bus != Bus::Voice));
        m.mix(&mut out);
        let half_back = left(&out)[0];
        assert!(half_back > 0.2 && half_back < 0.5, "ramping: {half_back}");
        m.mix(&mut out);
        m.mix(&mut out);
        assert!((left(&out)[0] - 0.5).abs() < 1e-6, "back: {:?}", left(&out));
    }

    #[test]
    fn settings_text_round_trips_and_forgives() {
        let mut s = trino_core::BusSettings::default();
        s.volumes[Bus::Music.index()] = 0.25;
        s.muted.insert(Bus::Ui);
        let text = super::settings_to_text(&s);
        assert!(text.contains("music 0.25\n") && text.contains("ui 1.00 muted\n"));
        assert_eq!(super::settings_from_text(&text), s);
        let odd = super::settings_from_text("# hi\nmusic loud\ndrums 0.5\nsfx 7\n");
        assert_eq!(odd.volumes[Bus::Sfx.index()], 1.0, "clamped");
        assert_eq!(odd.volumes[Bus::Music.index()], 1.0, "unparsed: default");
    }
}
