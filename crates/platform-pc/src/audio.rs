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
//! **Pitch, music control and filters** (Bongo-Kart #175, #177): a voice
//! reads its samples at its own rate (linear interpolation between them);
//! music can pause, report its position, crossfade, play as layers that
//! fade in and out; a bus can go through a low-pass. With none of them in
//! use the mix is the one before, sample for sample.
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
    Audio, AudioFeatures, AudioStats, Bus, BusFilter, BusSettings, DuckRule, MAX_MUSIC_LAYERS,
    MusicId, SoundBlob, SoundCodec, SoundId, SoundParams, VoiceId,
};

use stream::{Job, StreamSource, Streamer};

pub mod mixer {
    //! Pure stereo mixer (unit-tested without an audio device).

    use std::sync::Arc;

    use trino_core::{Bus, BusFilter, BusMixer, BusSet};

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

    /// A gain that glides to a target over a number of output frames
    /// (music layers and crossfades, Bongo-Kart #177).
    #[derive(Clone, Copy, PartialEq, Debug)]
    pub struct Ramp {
        pub value: f32,
        pub target: f32,
        /// Change per output frame (always toward the target).
        step: f32,
    }

    impl Ramp {
        pub const ONE: Ramp = Ramp {
            value: 1.0,
            target: 1.0,
            step: 0.0,
        };

        pub fn at(value: f32) -> Ramp {
            Ramp {
                value,
                target: value,
                step: 0.0,
            }
        }

        /// Glide to `target` over `frames` (at once for 0).
        pub fn to(&mut self, target: f32, frames: usize) {
            self.target = target;
            if frames == 0 {
                self.value = target;
                self.step = 0.0;
            } else {
                self.step = (target - self.value).abs() / frames as f32;
            }
        }

        pub fn still(&self) -> bool {
            self.value == self.target
        }

        /// The value after `frames` more frames.
        fn advance(&mut self, frames: usize) -> f32 {
            if !self.still() {
                let d = self.step * frames as f32;
                self.value = if self.value < self.target {
                    (self.value + d).min(self.target)
                } else {
                    (self.value - d).max(self.target)
                };
            }
            self.value
        }
    }

    pub struct Voice {
        pub id: u32,
        pub source: Source,
        /// Where it reads its samples: a whole index at pitch 1, between two
        /// samples while pitched (#175: linear interpolation).
        pub pos: f64,
        /// Its pitch ([`trino_core::SoundParams::rate`]); streams ignore it.
        pub rate: f32,
        pub looped: bool,
        pub kind: Kind,
        pub bus: Bus,
        /// Left / right gains.
        pub gains: (f32, f32),
        /// Music: its layer's gain and crossfade (1 for everything else).
        pub level: Ramp,
        /// Music: held by `pause_music` (nothing read, nothing mixed).
        pub paused: bool,
        /// Music: the layer it is (0 for a lone track).
        pub layer: u8,
        /// Music: fading out (a crossfade's old track), dropped at silence.
        pub leaving: bool,
        /// Samples it has read (a stream's position).
        pub played: u64,
        /// Its length in output samples, when known (a loop's position wraps).
        pub length: Option<u64>,
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
            let length = Some(data.len() as u64);
            Voice::new(id, Source::Pcm(data), looped, kind, bus, gains, length)
        }

        pub fn new(
            id: u32,
            source: Source,
            looped: bool,
            kind: Kind,
            bus: Bus,
            gains: (f32, f32),
            length: Option<u64>,
        ) -> Voice {
            Voice {
                id,
                source,
                pos: 0.0,
                rate: 1.0,
                looped,
                kind,
                bus,
                gains,
                level: Ramp::ONE,
                paused: false,
                layer: 0,
                leaving: false,
                played: 0,
                length,
            }
        }

        fn over(&self) -> bool {
            if self.leaving && self.level.value <= 0.0 && self.level.still() {
                return true;
            }
            match &self.source {
                Source::Pcm(d) => !self.looped && self.pos >= d.len() as f64,
                Source::Stream(r) => r.done(),
            }
        }

        /// Output samples into the track (a loop wraps).
        pub fn at(&self) -> u64 {
            let at = match &self.source {
                Source::Pcm(_) => self.pos as u64,
                Source::Stream(_) => self.played,
            };
            match self.length {
                Some(n) if n > 0 && self.looped => at % n,
                Some(n) => at.min(n),
                None => at,
            }
        }

        /// Seconds into the track (a loop wraps), at `rate` output samples
        /// per second.
        pub fn seconds(&self, rate: u32) -> f32 {
            self.at() as f32 / rate.max(1) as f32
        }

        /// Move on `frames` output samples without mixing them (a voice
        /// started a little late, in step with the one it replaces).
        pub fn skip(&mut self, frames: u64, scratch: &mut Vec<f32>) {
            match &self.source {
                Source::Pcm(d) if !d.is_empty() => {
                    let len = d.len() as f64;
                    let to = self.pos + frames as f64;
                    self.pos = if self.looped { to % len } else { to.min(len) };
                }
                Source::Pcm(_) => {}
                Source::Stream(ring) => {
                    let mut left = frames as usize;
                    scratch.resize(4096, 0.0);
                    while left > 0 {
                        let n = ring.pop(&mut scratch[..left.min(4096)]);
                        if n == 0 {
                            break;
                        }
                        left -= n;
                        self.played += n as u64;
                    }
                }
            }
        }

        /// Mix into `out` (stereo pairs) with gains `gl`, `gr`, scaled from
        /// `k0` to `k1` across the buffer (a ramp; 1 to 1 adds nothing).
        fn mix_into(
            &mut self,
            out: &mut [f32],
            (gl, gr): (f32, f32),
            (k0, k1): (f32, f32),
            scratch: &mut Vec<f32>,
        ) -> (u64, bool) {
            let frames = out.len() / 2;
            let ramp = k0 != 1.0 || k1 != 1.0;
            let gain = |i: usize| {
                if ramp {
                    k0 + (k1 - k0) * (i as f32 / frames.max(1) as f32)
                } else {
                    1.0
                }
            };
            match &self.source {
                Source::Pcm(data) => {
                    let len = data.len();
                    if self.rate == 1.0 && self.pos.fract() == 0.0 {
                        // Pitch 1: one sample per frame, as before #175.
                        let mut cursor = self.pos as usize;
                        for (i, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                            if cursor >= len {
                                if self.looped && len > 0 {
                                    cursor = 0;
                                } else {
                                    break;
                                }
                            }
                            let s = data[cursor];
                            if ramp {
                                let k = gain(i);
                                frame[0] += s * gl * k;
                                frame[1] += s * gr * k;
                            } else {
                                frame[0] += s * gl;
                                frame[1] += s * gr;
                            }
                            cursor += 1;
                        }
                        self.pos = cursor as f64;
                    } else {
                        let step = self.rate as f64;
                        let mut pos = self.pos;
                        for (i, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                            if pos >= len as f64 {
                                if self.looped && len > 0 {
                                    pos %= len as f64;
                                } else {
                                    break;
                                }
                            }
                            let at = pos as usize;
                            let frac = (pos - at as f64) as f32;
                            let a = data[at];
                            let b = match data.get(at + 1) {
                                Some(&b) => b,
                                None if self.looped => data[0],
                                None => a,
                            };
                            let s = (a + (b - a) * frac) * gain(i);
                            frame[0] += s * gl;
                            frame[1] += s * gr;
                            pos += step;
                        }
                        self.pos = pos;
                    }
                    (0, false)
                }
                Source::Stream(ring) => {
                    scratch.resize(frames, 0.0);
                    let n = ring.pop(&mut scratch[..frames]);
                    let pairs = out.as_chunks_mut::<2>().0.iter_mut();
                    for (i, (frame, s)) in pairs.zip(&scratch[..n]).enumerate() {
                        let s = s * gain(i);
                        frame[0] += s * gl;
                        frame[1] += s * gr;
                    }
                    self.played += n as u64;
                    let under = n < frames && !ring.finished();
                    (n as u64, under)
                }
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

    /// A bus's low-pass (Bongo-Kart #177): an RBJ biquad, Q = 1/√2, one
    /// state per channel (direct form I).
    #[derive(Clone, Copy, PartialEq, Debug)]
    pub struct LowPass {
        pub filter: BusFilter,
        b: [f32; 3],
        a: [f32; 2],
        /// Per channel: the last two inputs and outputs.
        x: [[f32; 2]; 2],
        y: [[f32; 2]; 2],
    }

    impl LowPass {
        pub fn new(filter: BusFilter, rate: u32) -> LowPass {
            let mut f = LowPass {
                filter,
                b: [1.0, 0.0, 0.0],
                a: [0.0, 0.0],
                x: [[0.0; 2]; 2],
                y: [[0.0; 2]; 2],
            };
            f.retune(filter, rate);
            f
        }

        /// New settings, keeping the state (no click while a cutoff glides).
        pub fn retune(&mut self, filter: BusFilter, rate: u32) {
            self.filter = filter;
            let rate = rate.max(1) as f32;
            let fc = filter.cutoff().min(rate * 0.45);
            let w0 = core::f32::consts::TAU * fc / rate;
            let (sin, cos) = w0.sin_cos();
            let alpha = sin / (2.0 * core::f32::consts::FRAC_1_SQRT_2);
            let a0 = 1.0 + alpha;
            let b1 = (1.0 - cos) / a0;
            self.b = [b1 * 0.5, b1, b1 * 0.5];
            self.a = [-2.0 * cos / a0, (1.0 - alpha) / a0];
        }

        /// Filter interleaved stereo in place.
        pub fn run(&mut self, buf: &mut [f32]) {
            for frame in buf.as_chunks_mut::<2>().0 {
                for (c, s) in frame.iter_mut().enumerate() {
                    let (x, y) = (&mut self.x[c], &mut self.y[c]);
                    let out = self.b[0] * *s + self.b[1] * x[0] + self.b[2] * x[1]
                        - self.a[0] * y[0]
                        - self.a[1] * y[1];
                    x[1] = x[0];
                    x[0] = *s;
                    y[1] = y[0];
                    // Flush denormals: a silent tail must not cost CPU.
                    y[0] = if out.abs() < 1e-20 { 0.0 } else { out };
                    *s = y[0];
                }
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
        /// A low-pass per bus (`None`: the bus mixes straight into the
        /// output, as before #177).
        pub filters: [Option<LowPass>; Bus::COUNT],
        scratch: Vec<f32>,
        /// Each filtered bus's voices, before its filter.
        bus_bufs: [Vec<f32>; Bus::COUNT],
    }

    impl Mixer {
        pub fn new(rate: u32) -> Self {
            Mixer {
                voices: Vec::new(),
                buses: BusMixer::new(),
                rate: rate.max(1),
                underruns: 0,
                streamed: 0,
                filters: [None; Bus::COUNT],
                scratch: Vec::new(),
                bus_bufs: Default::default(),
            }
        }

        /// The buses something plays on.
        pub fn sounding(&self) -> BusSet {
            let mut s = BusSet::EMPTY;
            for v in &self.voices {
                if !v.paused {
                    s.insert(v.bus);
                }
            }
            s
        }

        /// Put a filter on `bus` or take it off.
        pub fn set_filter(&mut self, bus: Bus, filter: Option<BusFilter>) {
            let slot = &mut self.filters[bus.index()];
            match (filter, slot.as_mut()) {
                (None, _) => *slot = None,
                (Some(f), Some(lp)) => lp.retune(f, self.rate),
                (Some(f), None) => *slot = Some(LowPass::new(f, self.rate)),
            }
        }

        /// Mix every voice into interleaved stereo `out` (`[l, r, l, r,
        /// ...]`): advance the duck ramps by its length, apply each bus's
        /// gain (and filter), advance cursors, drop finished voices, clamp
        /// to -1..1.
        pub fn mix(&mut self, out: &mut [f32]) {
            let frames = out.len() / 2;
            let sounding = self.sounding();
            self.buses
                .update(frames as f32 / self.rate as f32, sounding);
            let gains = self.buses.gains();
            out.fill(0.0);
            let ducked = self.voices.iter().any(|v| v.kind == Kind::Stinger);
            // Voices in their order, straight into `out` (the order and the
            // sums of before #177), except those on a filtered bus: into
            // the bus's buffer, filtered and added after.
            let mut filtered = [false; Bus::COUNT];
            for b in Bus::ALL {
                filtered[b.index()] = b != Bus::Master && self.filters[b.index()].is_some();
                let buf = &mut self.bus_bufs[b.index()];
                if filtered[b.index()] {
                    buf.clear();
                    buf.resize(out.len(), 0.0);
                } else {
                    buf.clear();
                }
            }
            for voice in self.voices.iter_mut().filter(|v| !v.paused) {
                let k = gains[voice.bus.index()]
                    * if voice.kind == Kind::Music && ducked {
                        DUCK
                    } else {
                        1.0
                    };
                let (gl, gr) = (voice.gains.0 * k, voice.gains.1 * k);
                let k0 = voice.level.value;
                let k1 = voice.level.advance(frames);
                let target: &mut [f32] = if filtered[voice.bus.index()] {
                    &mut self.bus_bufs[voice.bus.index()]
                } else {
                    out
                };
                let (n, under) = voice.mix_into(target, (gl, gr), (k0, k1), &mut self.scratch);
                self.streamed += n;
                if under {
                    self.underruns += 1;
                }
            }
            for b in Bus::ALL {
                if !filtered[b.index()] {
                    continue;
                }
                if let Some(lp) = self.filters[b.index()].as_mut() {
                    // Run on silence too: what played rings out.
                    let buf = &mut self.bus_bufs[b.index()];
                    lp.run(buf);
                    for (o, s) in out.iter_mut().zip(buf.iter()) {
                        *o += s;
                    }
                }
            }
            if let Some(lp) = self.filters[Bus::Master.index()].as_mut() {
                lp.run(out);
            }
            self.voices.retain(|v| !v.over());
            for slot in out.iter_mut() {
                *slot = slot.clamp(-1.0, 1.0);
            }
        }
    }
}

use mixer::{Kind, Mixer, Ramp, Source, Voice};

struct Shared {
    mixer: Mixer,
    next_id: u32,
}

impl Shared {
    fn push(
        &mut self,
        (source, length): (Source, Option<u64>),
        looped: bool,
        kind: Kind,
        bus: Bus,
        gains: (f32, f32),
    ) -> &mut Voice {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let v = Voice::new(self.next_id, source, looped, kind, bus, gains, length);
        self.mixer.voices.push(v);
        self.mixer.voices.last_mut().expect("just pushed")
    }

    /// The music's voices still playing (not a crossfade's old track).
    fn music(&mut self) -> impl Iterator<Item = &mut Voice> {
        self.mixer
            .voices
            .iter_mut()
            .filter(|v| v.kind == Kind::Music && !v.leaving)
    }
}

/// Output frames in `ms` milliseconds at `rate`.
fn frames_of(ms: u32, rate: u32) -> usize {
    (ms as u64 * rate as u64 / 1000) as usize
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

    /// A playable source for `clip`, and its length in output samples:
    /// its samples, or a new stream.
    fn source(&mut self, clip: &Clip, looped: bool) -> Option<(Source, Option<u64>)> {
        self.source_at(clip, looped, 0)
    }

    /// [`PcAudio::source`] from output sample `skip` on (a synced
    /// crossfade): a stream seeks there ([`Job::start_at`]).
    fn source_at(&mut self, clip: &Clip, looped: bool, skip: u64) -> Option<(Source, Option<u64>)> {
        match &clip.data {
            ClipData::Pcm(d) => Some((Source::Pcm(d.clone()), Some(d.len() as u64))),
            ClipData::Stream(src) => {
                let length = src.output_frames(self.sample_rate);
                // A loop wraps a position past its length; a one-shot
                // past its end starts from the top.
                let skip = match length {
                    Some(n) if n > 0 && looped => skip % n,
                    _ => skip,
                };
                // Half a second of output per stream.
                let capacity = (self.sample_rate / 2).max(1024) as usize;
                let (mut job, ring) =
                    Job::start_at(src, self.sample_rate, looped, capacity, skip).ok()?;
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
                Some((Source::Stream(ring), length))
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
        self.with_mixer(|s| {
            s.push(source, false, Kind::Sfx, bus, params.gains()).rate = params.rate();
        });
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
        self.with_mixer(|s| {
            let v = s.push(source, looped, Kind::Sfx, bus, params.gains());
            v.rate = params.rate();
            VoiceId(v.id)
        })
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        self.with_mixer(|s| {
            if let Some(v) = s.mixer.voices.iter_mut().find(|v| v.id == voice.0) {
                v.gains = params.gains();
                v.rate = params.rate();
            }
        });
    }

    fn set_voice_pitch(&mut self, voice: VoiceId, pitch: f32) {
        self.with_mixer(|s| {
            if let Some(v) = s.mixer.voices.iter_mut().find(|v| v.id == voice.0) {
                v.rate = trino_core::clamp_pitch(pitch);
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
        self.with_mixer(|s| {
            s.push(source, looped, Kind::Music, clip.bus, (1.0, 1.0));
        });
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
        self.with_mixer(|s| {
            s.push(source, false, Kind::Stinger, clip.bus, (1.0, 1.0));
        });
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

    fn audio_features(&self) -> AudioFeatures {
        AudioFeatures::ALL
    }

    fn pause_music(&mut self) {
        self.with_mixer(|s| {
            for v in s.mixer.voices.iter_mut().filter(|v| v.kind == Kind::Music) {
                v.paused = true;
            }
        });
    }

    fn resume_music(&mut self) {
        self.with_mixer(|s| {
            for v in s.mixer.voices.iter_mut().filter(|v| v.kind == Kind::Music) {
                v.paused = false;
            }
        });
    }

    fn music_paused(&self) -> bool {
        self.with_mixer(|s| s.music().next().is_some_and(|v| v.paused))
            .unwrap_or(false)
    }

    fn music_position(&self) -> Option<f32> {
        let rate = self.sample_rate;
        self.with_mixer(|s| s.music().min_by_key(|v| v.layer).map(|v| v.seconds(rate)))
            .flatten()
    }

    fn crossfade_music(&mut self, music: MusicId, looped: bool, fade_ms: u32, sync: bool) {
        let Some(clip) = self.music.get(&music.0).cloned() else {
            return;
        };
        let rate = self.sample_rate;
        let fade = frames_of(fade_ms, rate);
        // The old track's sample now; the device may mix a buffer more
        // before the swap below, and the new track catches up by as much.
        let first = |s: &mut Shared| {
            s.music()
                .min_by_key(|v| v.layer)
                .map(|v| (v.at(), v.length))
        };
        let then = if sync {
            self.with_mixer(first).flatten()
        } else {
            None
        };
        let skip = then.map_or(0, |(at, _)| at);
        let Some(source) = self.source_at(&clip, looped, skip) else {
            return;
        };
        self.with_mixer(|s| {
            let late = match (then, first(s)) {
                (Some((a, _)), Some((b, len))) => match len {
                    Some(n) if n > 0 && b < a => b + n - a,
                    _ => b.saturating_sub(a),
                },
                _ => 0,
            };
            let paused = s.music().next().is_some_and(|v| v.paused);
            // The old track (every layer) fades out and goes; one already
            // leaving keeps its own fade.
            s.mixer
                .voices
                .retain(|v| !(v.kind == Kind::Music && v.leaving && fade == 0));
            for v in s.music() {
                v.leaving = true;
                v.level.to(0.0, fade);
            }
            s.mixer
                .voices
                .retain(|v| !(v.kind == Kind::Music && v.leaving && v.level.value <= 0.0));
            let v = s.push(source, looped, Kind::Music, clip.bus, (1.0, 1.0));
            match &v.source {
                Source::Pcm(d) if !d.is_empty() => v.pos = (skip % d.len() as u64) as f64,
                Source::Pcm(_) => {}
                Source::Stream(_) => v.played = skip,
            }
            if late > 0 {
                let mut scratch = Vec::new();
                v.skip(late, &mut scratch);
            }
            v.paused = paused;
            v.level = Ramp::at(if fade == 0 { 1.0 } else { 0.0 });
            v.level.to(1.0, fade);
        });
    }

    fn play_music_layers(&mut self, layers: &[MusicId], looped: bool) {
        let mut sources = Vec::new();
        for (i, id) in layers.iter().take(MAX_MUSIC_LAYERS).enumerate() {
            let Some(clip) = self.music.get(&id.0).cloned() else {
                continue;
            };
            if let Some(src) = self.source(&clip, looped) {
                sources.push((i as u8, src, clip.bus));
            }
        }
        // One lock: every layer starts in the same buffer, sample-aligned.
        self.with_mixer(|s| {
            s.mixer.voices.retain(|v| v.kind != Kind::Music);
            for (i, src, bus) in sources {
                let v = s.push(src, looped, Kind::Music, bus, (1.0, 1.0));
                v.layer = i;
                v.level = Ramp::at(if i == 0 { 1.0 } else { 0.0 });
            }
        });
    }

    fn set_music_layer(&mut self, layer: usize, gain: f32, fade_ms: u32) {
        let fade = frames_of(fade_ms, self.sample_rate);
        let gain = if gain.is_nan() {
            0.0
        } else {
            gain.clamp(0.0, 1.0)
        };
        self.with_mixer(|s| {
            for v in s.music().filter(|v| v.layer as usize == layer) {
                v.level.to(gain, fade);
            }
        });
    }

    fn music_layer(&self, layer: usize) -> Option<f32> {
        self.with_mixer(|s| {
            s.music()
                .find(|v| v.layer as usize == layer)
                .map(|v| v.level.value)
        })
        .flatten()
    }

    fn set_bus_filter(&mut self, bus: Bus, filter: Option<BusFilter>) {
        self.with_mixer(|s| s.mixer.set_filter(bus, filter));
    }

    fn bus_filter(&self, bus: Bus) -> Option<BusFilter> {
        self.with_mixer(|s| s.mixer.filters[bus.index()].map(|f| f.filter))
            .flatten()
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
    use super::mixer::{DUCK, Kind, Mixer, Ramp, Voice};
    use std::sync::Arc;
    use trino_core::{Bus, BusFilter, DuckRule};

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

    /// Frames until `m`'s voices are all over, mixing `step` at a time.
    fn frames_to_end(m: &mut Mixer, step: usize) -> usize {
        let mut out = vec![0.0f32; step * 2];
        let mut n = 0;
        while !m.voices.is_empty() {
            m.mix(&mut out);
            n += step;
            assert!(n < 1_000_000, "never ends");
        }
        n
    }

    /// Bongo-Kart #175: a voice's pitch is its own. Twice the rate ends in
    /// half the frames; the voice beside it keeps its length and level.
    #[test]
    fn pitch_changes_one_voices_length_only() {
        let mut fast = voice(vec![0.25; 100], false);
        fast.rate = 2.0;
        let mut m = mixer(vec![fast]);
        assert_eq!(frames_to_end(&mut m, 1), 50);
        let mut slow = voice(vec![0.25; 100], false);
        slow.rate = 0.5;
        let mut m = mixer(vec![slow]);
        assert_eq!(frames_to_end(&mut m, 1), 200);
        // Together: the plain voice is heard alone from frame 50 to 100,
        // and ends at 100 as it would alone.
        let mut fast = voice(vec![0.25; 100], false);
        fast.rate = 2.0;
        let mut m = mixer(vec![fast, voice(vec![0.5; 100], false)]);
        let mut out = [0.0f32; 240];
        m.mix(&mut out);
        let l = left(&out);
        assert_eq!(l[..50], [0.75; 50]);
        assert_eq!(l[50..100], [0.5; 50]);
        assert_eq!(l[100..], [0.0; 20]);
        assert!(m.voices.is_empty());
    }

    #[test]
    fn half_pitch_reads_between_samples() {
        let mut v = voice(vec![0.0, 1.0, 0.0, -1.0], false);
        v.rate = 0.5;
        let mut m = mixer(vec![v]);
        let mut out = [0.0f32; 20];
        m.mix(&mut out);
        assert_eq!(
            left(&out),
            [0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -1.0, 0.0, 0.0]
        );
        // A loop reads its first sample after its last.
        let mut v = voice(vec![0.0, 1.0], true);
        v.rate = 0.5;
        let mut m = mixer(vec![v]);
        let mut out = [0.0f32; 12];
        m.mix(&mut out);
        assert_eq!(left(&out), [0.0, 0.5, 1.0, 0.5, 0.0, 0.5]);
    }

    /// Pitch 1 reads whole samples, as before #175, even after a pitched
    /// stretch left the cursor between two.
    #[test]
    fn pitch_one_after_a_bend_reads_on() {
        let mut v = voice((0..10).map(|i| i as f32 / 10.0).collect(), false);
        v.rate = 1.5;
        let mut m = mixer(vec![v]);
        let mut out = [0.0f32; 4];
        m.mix(&mut out);
        assert_eq!(m.voices[0].pos, 3.0);
        m.voices[0].rate = 1.0;
        m.mix(&mut out);
        assert_eq!(left(&out), [0.3, 0.4]);
    }

    #[test]
    fn a_paused_voice_holds_its_place() {
        let mut m = mixer(vec![kind_voice(
            vec![0.1, 0.2, 0.3, 0.4],
            false,
            Kind::Music,
        )]);
        let mut out = [0.0f32; 4];
        m.mix(&mut out);
        assert_eq!(left(&out), [0.1, 0.2]);
        m.voices[0].paused = true;
        m.mix(&mut out);
        assert_eq!(out, [0.0; 4], "silent while paused");
        assert_eq!(m.voices.len(), 1, "and kept");
        m.voices[0].paused = false;
        m.mix(&mut out);
        let l = left(&out);
        assert!(
            (l[0] - 0.3).abs() < 1e-6 && (l[1] - 0.4).abs() < 1e-6,
            "{l:?}"
        );
    }

    #[test]
    fn a_level_ramp_glides_and_a_leaving_voice_goes_at_silence() {
        let mut v = kind_voice(vec![1.0; 100], true, Kind::Music);
        v.level = Ramp::at(0.0);
        v.level.to(1.0, 10);
        let mut m = mixer(vec![v]);
        let mut out = [0.0f32; 20];
        m.mix(&mut out);
        let l = left(&out);
        for (i, x) in l.iter().enumerate() {
            assert!((x - i as f32 / 10.0).abs() < 1e-5, "{l:?}");
        }
        assert_eq!(m.voices[0].level.value, 1.0);
        m.voices[0].leaving = true;
        m.voices[0].level.to(0.0, 20);
        m.mix(&mut out);
        assert_eq!(m.voices.len(), 1, "still fading");
        m.mix(&mut out);
        assert!(m.voices.is_empty(), "gone at silence");
    }

    fn sine(hz: f32, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| 0.5 * (i as f32 * hz * std::f32::consts::TAU / rate as f32).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Bongo-Kart #177: a bus's low-pass keeps what is under its cutoff and
    /// cuts what is far above; other buses are untouched, and taking it off
    /// gives the unfiltered mix sample for sample.
    #[test]
    fn a_bus_low_pass_cuts_high_tones_and_only_on_its_bus() {
        let rate = 48_000;
        let run = |hz: f32, bus: Bus, filter: Option<BusFilter>| {
            let mut v = voice(sine(hz, rate, 9600), false);
            v.bus = bus;
            let mut m = Mixer::new(rate);
            m.voices = vec![v];
            m.set_filter(Bus::Sfx, filter);
            let mut out = vec![0.0f32; 9600 * 2];
            m.mix(&mut out);
            // After the filter settles.
            rms(&left(&out)[2400..])
        };
        let lp = Some(BusFilter::LowPass { cutoff_hz: 800.0 });
        let plain_low = run(100.0, Bus::Sfx, None);
        let plain_high = run(8000.0, Bus::Sfx, None);
        assert!(run(100.0, Bus::Sfx, lp) > 0.95 * plain_low);
        assert!(run(8000.0, Bus::Sfx, lp) < 0.02 * plain_high, "cut ~40 dB");
        assert_eq!(run(8000.0, Bus::Ui, lp), plain_high, "another bus");
        // On and off again: the old path, bit for bit.
        let mut a = mixer(vec![voice(sine(3000.0, 1000, 300), false)]);
        let mut b = mixer(vec![voice(sine(3000.0, 1000, 300), false)]);
        b.set_filter(Bus::Sfx, lp);
        b.set_filter(Bus::Sfx, None);
        let (mut oa, mut ob) = ([0.0f32; 600], [0.0f32; 600]);
        a.mix(&mut oa);
        b.mix(&mut ob);
        assert_eq!(oa, ob);
        // A filter on the master filters everything.
        let mut m = Mixer::new(rate);
        let mut v = voice(sine(8000.0, rate, 9600), false);
        v.bus = Bus::Ui;
        m.voices = vec![v];
        m.set_filter(Bus::Master, lp);
        let mut out = vec![0.0f32; 9600 * 2];
        m.mix(&mut out);
        assert!(rms(&left(&out)[2400..]) < 0.02 * plain_high);
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
