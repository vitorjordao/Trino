//! [`trino_core::Audio`] on Web Audio (issues #47, #53).
//!
//! Every playback is a source → gain → stereo panner → its **bus** gain →
//! master gain → speakers; the browser mixes. The bus and master gains are
//! [`trino_core::BusMixer`]'s arithmetic (volumes, mutes, duck ramps),
//! advanced once per frame by [`WebAudio::poll`] with the frame's real
//! time, like the consoles. Music ducks to [`DUCK`] of its level while a
//! stinger plays, like the PC mixer.
//!
//! **Baked sounds** come as `TSN2` blobs ([`WebAudio::upload_sound_blob`]):
//! Ogg Vorbis (the Web default) or PCM16, decoded by lewton in the wasm
//! module ([`crate::sound`]) — the PC's decoder, so both play the same
//! samples. Sound effects are decoded once at load into an `AudioBuffer`
//! (the browser resamples to the device). Anything baked `stream` (music by
//! default) keeps only its encoded bytes and is decoded as it plays: each
//! frame tops up chunks of [`CHUNK_FRAMES`] samples, each its own
//! `AudioBufferSourceNode` started at the exact time the previous one ends
//! (sub-sample accurate, so chunk joins and loop seams are seamless), never
//! more than [`LOOKAHEAD`] seconds ahead — memory stays at about a second
//! of decoded audio however long the track. A chunk that would start in
//! the past (the page stalled longer than the lookahead) counts as an
//! underrun and starts now.
//!
//! *Why not an `AudioWorklet`:* a worklet would decode on the audio thread
//! and survive a stalled main thread, but it needs the decoder in a second
//! wasm instance inside `AudioWorkletGlobalScope` and a shared ring
//! (`SharedArrayBuffer`): far more machinery for what a one-second
//! lookahead already covers in a page that draws every frame. A hidden tab
//! (no animation frames) runs out after the lookahead and resumes with an
//! underrun counted.
//!
//! **Autoplay:** every browser (Safari strictest) creates the context
//! suspended until the user interacts with the page. Playback requests
//! before that are scheduled and play once [`WebAudio::unlock`] resumes it
//! — the runtime calls it on the first click, key or touch, and shows a
//! banner until the context runs. A refused `resume()` is logged, never an
//! unhandled rejection. While suspended the context's clock stands still:
//! streams fill their lookahead and wait.
//!
//! **No Web Audio** (some embedded WebKit builds): the backend runs silent,
//! like the PC without an output device, and keeps the API's meaning —
//! voices, stingers and their ends follow the page's clock — so a game and
//! the self-test behave the same.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use trino_core::{
    Audio, AudioStats, Bus, BusMixer, BusSet, BusSettings, DuckRule, MusicId, SoundBlob,
    SoundCodec, SoundId, SoundParams, VoiceId,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{
    AudioBuffer, AudioBufferSourceNode, AudioContext, AudioContextState, GainNode, StereoPannerNode,
};

use crate::sound::{self, StreamDecoder};

/// Music level while a stinger plays (the PC mixer's `DUCK`).
pub const DUCK: f32 = 0.3;
/// Most playbacks at once (the PC's voice pool).
const MAX_VOICES: usize = 24;
/// Seconds of a stream kept scheduled ahead of the output.
pub const LOOKAHEAD: f64 = 1.0;
/// Source samples per scheduled stream chunk (~0.19 s at 44.1 kHz).
pub const CHUNK_FRAMES: usize = 8192;
/// A stream's first chunk starts this far ahead of now (seconds), so the
/// browser has it before it is due.
const START_DELAY: f64 = 0.03;
/// Bus gains glide to new values with this time constant (seconds): no
/// zipper noise from per-frame duck steps.
const GAIN_GLIDE: f64 = 0.01;

/// A loaded sound or track.
struct Clip {
    bus: Bus,
    rate: u32,
    frames: usize,
    data: ClipData,
}

enum ClipData {
    /// Decoded at load (none when running silent).
    Buffer(Option<AudioBuffer>),
    /// Kept encoded; decoded as it plays.
    Stream { codec: SoundCodec, bytes: Rc<[u8]> },
}

impl Clip {
    fn seconds(&self) -> f64 {
        self.frames as f64 / self.rate.max(1) as f64
    }

    /// Memory it holds: decoded samples (f32), or the encoded stream.
    fn bytes(&self) -> usize {
        match &self.data {
            ClipData::Buffer(_) => self.frames * 4,
            ClipData::Stream { bytes, .. } => bytes.len(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Sfx,
    Music,
    Stinger,
}

/// One scheduled piece of a stream.
struct Chunk {
    source: AudioBufferSourceNode,
    start: f64,
    frames: usize,
}

/// A stream being played: its decoder and the chunks in flight.
struct Streaming {
    dec: StreamDecoder,
    looped: bool,
    /// Context time where the next chunk starts.
    next_start: f64,
    chunks: VecDeque<Chunk>,
    /// The decoder is through (nothing more to schedule).
    done: bool,
    scratch: Vec<f32>,
}

impl Streaming {
    fn seconds(&self, frames: usize) -> f64 {
        frames as f64 / self.dec.rate() as f64
    }

    /// Decoded samples in memory (scheduled chunks, the decoder's carry).
    fn held(&self) -> usize {
        self.chunks.iter().map(|c| c.frames).sum::<usize>() + self.dec.held()
    }

    /// Of the chunk playing at `now`, the samples already played.
    fn playing(&self, now: f64) -> usize {
        self.chunks.front().map_or(0, |c| {
            (((now - c.start) * self.dec.rate() as f64).max(0.0) as usize).min(c.frames)
        })
    }

    fn stop(&mut self) {
        for c in self.chunks.drain(..) {
            let _ = scheduled(&c.source).stop();
            let _ = c.source.disconnect();
        }
        self.done = true;
    }
}

enum Play {
    /// No Web Audio: it ends by the clock.
    Silent,
    Buffer {
        source: AudioBufferSourceNode,
        _on_ended: Closure<dyn FnMut()>,
    },
    Stream(Box<Streaming>),
}

/// One playback: what it plays, its nodes and its state.
struct Playback {
    kind: Kind,
    bus: Bus,
    /// Its gain and panner (none when running silent).
    nodes: Option<(GainNode, StereoPannerNode)>,
    /// The game's volume for it (the track ducks under a stinger).
    volume: f32,
    play: Play,
    ended: Rc<Cell<bool>>,
    /// Silent playbacks: when it ends (seconds, page clock); `None` loops.
    until: Option<f64>,
}

impl Playback {
    fn ended(&self) -> bool {
        if self.ended.get() || self.until.is_some_and(|t| now() >= t) {
            return true;
        }
        matches!(&self.play, Play::Stream(s) if s.done && s.chunks.is_empty())
    }

    fn stop(&mut self) {
        self.ended.set(true);
        match &mut self.play {
            Play::Buffer { source, .. } => {
                let _ = scheduled(source).stop();
                scheduled(source).set_onended(None);
            }
            Play::Stream(s) => s.stop(),
            Play::Silent => {}
        }
        if let Some((_, pan)) = &self.nodes {
            let _ = pan.disconnect();
        }
    }
}

/// The page's clock in seconds.
fn now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now() / 1000.0)
}

/// Counters the streams keep.
#[derive(Clone, Copy, Default)]
struct StreamCounters {
    underruns: u32,
    /// Samples of chunks that finished playing.
    played: u64,
}

pub struct WebAudio {
    ctx: Option<AudioContext>,
    master: Option<GainNode>,
    /// A gain per bus (the master's slot unused: [`WebAudio::master`]).
    bus_nodes: Vec<Option<GainNode>>,
    /// The gains last set on the nodes, by bus index.
    applied: [f32; Bus::COUNT],
    buses: BusMixer,
    sounds: HashMap<u32, Clip>,
    music: HashMap<u32, Clip>,
    voices: HashMap<u32, Playback>,
    /// Fire-and-forget one-shots, until they end.
    shots: Vec<Playback>,
    next_voice: u32,
    track: Option<Playback>,
    stinger: Option<Playback>,
    counters: StreamCounters,
    /// The page clock at the last [`WebAudio::poll`].
    last_poll: Option<f64>,
}

impl WebAudio {
    /// A context (suspended until [`WebAudio::unlock`] where the browser
    /// requires a gesture). Without Web Audio the backend is silent, never
    /// an error: audio must not break a page.
    pub fn new() -> Self {
        let ctx = AudioContext::new().ok();
        let gain = |ctx: &AudioContext| ctx.create_gain().ok();
        let master = ctx.as_ref().and_then(gain);
        let bus_nodes: Vec<Option<GainNode>> = Bus::ALL
            .iter()
            .map(|&b| {
                if b == Bus::Master {
                    None
                } else {
                    ctx.as_ref().and_then(gain)
                }
            })
            .collect();
        let complete = bus_nodes
            .iter()
            .zip(Bus::ALL)
            .all(|(n, b)| b == Bus::Master || n.is_some());
        let ctx = match (ctx, &master) {
            (Some(ctx), Some(m)) if complete => {
                let _ = m.connect_with_audio_node(&ctx.destination());
                for node in bus_nodes.iter().flatten() {
                    let _ = node.connect_with_audio_node(m);
                }
                Some(ctx)
            }
            _ => {
                crate::js::log("warning: no Web Audio in this browser: running silent");
                None
            }
        };
        WebAudio {
            ctx,
            master,
            bus_nodes,
            applied: [1.0; Bus::COUNT],
            buses: BusMixer::new(),
            sounds: HashMap::new(),
            music: HashMap::new(),
            voices: HashMap::new(),
            shots: Vec::new(),
            next_voice: 0,
            track: None,
            stinger: None,
            counters: StreamCounters::default(),
            last_poll: None,
        }
    }

    /// The context runs (sound is audible), or there is no Web Audio at
    /// all (nothing to unlock).
    pub fn running(&self) -> bool {
        self.ctx
            .as_ref()
            .is_none_or(|c| c.state() == AudioContextState::Running)
    }

    /// The browser has Web Audio (else the backend runs silent).
    pub fn has_web_audio(&self) -> bool {
        self.ctx.is_some()
    }

    /// `running`, `suspended`, `closed` or `none` (no Web Audio).
    pub fn state(&self) -> &'static str {
        match self.ctx.as_ref().map(|c| c.state()) {
            None => "none",
            Some(AudioContextState::Running) => "running",
            Some(AudioContextState::Suspended) => "suspended",
            Some(_) => "closed",
        }
    }

    /// The context's clock in seconds (0 when silent; it stands still while
    /// suspended).
    pub fn current_time(&self) -> f64 {
        self.ctx.as_ref().map_or(0.0, |c| c.current_time())
    }

    /// The context's output rate (0 when silent).
    pub fn sample_rate(&self) -> u32 {
        self.ctx.as_ref().map_or(0, |c| c.sample_rate() as u32)
    }

    /// Resume a suspended context (call from a user gesture's handler).
    /// The browser answers asynchronously; a refusal is logged.
    pub fn unlock(&self) {
        let Some(ctx) = &self.ctx else { return };
        if ctx.state() != AudioContextState::Suspended {
            return;
        }
        match ctx.resume() {
            Ok(promise) => {
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(e) = wasm_bindgen_futures::JsFuture::from(promise).await {
                        crate::js::log(&format!("warning: audio resume refused: {e:?}"));
                    }
                });
            }
            Err(e) => crate::js::log(&format!("warning: audio resume failed: {e:?}")),
        }
    }

    /// An `AudioBuffer` of mono `samples` at `rate`.
    fn buffer(&self, rate: u32, samples: &[f32]) -> Option<AudioBuffer> {
        let ctx = self.ctx.as_ref()?;
        if samples.is_empty() || rate == 0 {
            return None;
        }
        let buf = ctx
            .create_buffer(1, samples.len() as u32, rate as f32)
            .ok()?;
        buf.copy_to_channel(samples, 0).ok()?;
        Some(buf)
    }

    fn pcm_clip(&self, bus: Bus, rate: u32, samples: &[f32]) -> Option<Clip> {
        (!samples.is_empty() && rate > 0).then(|| Clip {
            bus,
            rate,
            frames: samples.len(),
            data: ClipData::Buffer(self.buffer(rate, samples)),
        })
    }

    /// Register (or replace) a sound effect: mono samples at `rate`, on
    /// the effects bus.
    pub fn upload_sound(&mut self, id: SoundId, rate: u32, samples: &[f32]) {
        if let Some(c) = self.pcm_clip(Bus::Sfx, rate, samples) {
            self.sounds.insert(id.0, c);
        }
    }

    /// Register (or replace) a music track, on the music bus.
    pub fn upload_music(&mut self, id: MusicId, rate: u32, samples: &[f32]) {
        if let Some(c) = self.pcm_clip(Bus::Music, rate, samples) {
            self.music.insert(id.0, c);
        }
    }

    /// A baked `TSN2` sound: decoded now unless it streams; it plays on its
    /// baked bus unless the call names one.
    pub fn upload_sound_blob(&mut self, id: SoundId, blob: &[u8]) -> Result<(), String> {
        let clip = self.blob_clip(blob)?;
        self.sounds.insert(id.0, clip);
        Ok(())
    }

    /// A baked `TSN2` track (music or a stinger).
    pub fn upload_music_blob(&mut self, id: MusicId, blob: &[u8]) -> Result<(), String> {
        let clip = self.blob_clip(blob)?;
        self.music.insert(id.0, clip);
        Ok(())
    }

    /// Forget a sound (an unloaded asset group's): the bytes it held. A
    /// playback already started keeps its own buffer to its end.
    pub fn remove_sound(&mut self, id: SoundId) -> usize {
        self.sounds.remove(&id.0).map_or(0, |c| c.bytes())
    }

    /// Forget a music track or stinger ([`WebAudio::remove_sound`]).
    pub fn remove_music(&mut self, id: MusicId) -> usize {
        self.music.remove(&id.0).map_or(0, |c| c.bytes())
    }

    /// The bytes a loaded sound or track holds (0 when not loaded).
    pub fn clip_bytes(&self, id: u32, music: bool) -> usize {
        let map = if music { &self.music } else { &self.sounds };
        map.get(&id).map_or(0, Clip::bytes)
    }

    fn blob_clip(&self, blob: &[u8]) -> Result<Clip, String> {
        let b = SoundBlob::parse(blob).map_err(String::from)?;
        if b.stream {
            // Opened once here, so a broken stream fails at load, not later.
            let bytes: Rc<[u8]> = b.payload.into();
            let dec = StreamDecoder::open(b.codec, b.sample_rate, b.frames, bytes.clone())?;
            return Ok(Clip {
                bus: b.bus,
                rate: dec.rate(),
                frames: b.frames as usize,
                data: ClipData::Stream {
                    codec: b.codec,
                    bytes,
                },
            });
        }
        let d = sound::decode(&b)?;
        Ok(Clip {
            bus: b.bus,
            rate: d.rate,
            frames: d.samples.len(),
            data: ClipData::Buffer(self.buffer(d.rate, &d.samples)),
        })
    }

    /// The buses' volumes and mutes (what the runtime persists).
    pub fn bus_settings(&self) -> BusSettings {
        self.buses.settings()
    }

    /// Start a loaded clip (`music` picks the registry) on `bus` with
    /// `params`: its graph, or a silent playback that ends by the clock.
    fn start(
        &mut self,
        music: bool,
        id: u32,
        kind: Kind,
        bus: Bus,
        params: &SoundParams,
        looped: bool,
    ) -> Option<Playback> {
        let clip = if music {
            self.music.get(&id)?
        } else {
            self.sounds.get(&id)?
        };
        let ended = Rc::new(Cell::new(false));
        let silent = Playback {
            kind,
            bus,
            nodes: None,
            volume: params.volume,
            play: Play::Silent,
            ended: ended.clone(),
            until: (!looped).then(|| now() + clip.seconds()),
        };
        let (Some(ctx), Some(Some(bus_node))) = (&self.ctx, self.bus_nodes.get(bus.index())) else {
            return Some(silent);
        };
        if matches!(clip.data, ClipData::Buffer(None)) {
            return Some(silent);
        }
        let gain = ctx.create_gain().ok()?;
        let pan = ctx.create_stereo_panner().ok()?;
        gain.connect_with_audio_node(&pan).ok()?;
        pan.connect_with_audio_node(bus_node).ok()?;
        set_params(&gain, &pan, params);
        let play = match &clip.data {
            ClipData::Buffer(None) => return Some(silent),
            ClipData::Buffer(Some(buffer)) => {
                let source = ctx.create_buffer_source().ok()?;
                source.set_buffer(Some(buffer));
                source.set_loop(looped);
                source.connect_with_audio_node(&gain).ok()?;
                let flag = ended.clone();
                let on_ended = Closure::<dyn FnMut()>::new(move || flag.set(true));
                scheduled(&source).set_onended(Some(on_ended.as_ref().unchecked_ref()));
                scheduled(&source).start().ok()?;
                Play::Buffer {
                    source,
                    _on_ended: on_ended,
                }
            }
            ClipData::Stream { codec, bytes } => {
                let dec = StreamDecoder::open(*codec, clip.rate, clip.frames as u32, bytes.clone())
                    .ok()?;
                let mut s = Box::new(Streaming {
                    dec,
                    looped,
                    next_start: ctx.current_time() + START_DELAY,
                    chunks: VecDeque::new(),
                    done: false,
                    scratch: Vec::with_capacity(CHUNK_FRAMES),
                });
                // Fill the lookahead now: the first chunks are due at once.
                pump(ctx, &gain, &mut s, &mut self.counters);
                Play::Stream(s)
            }
        };
        Some(Playback {
            kind,
            bus,
            nodes: Some((gain, pan)),
            volume: params.volume,
            play,
            ended,
            until: None,
        })
    }

    /// Per frame: top up the streams, forget finished playbacks, bring the
    /// music back after a stinger, advance the duck ramps by the frame's
    /// time and set the bus gains.
    pub fn poll(&mut self) {
        let t = now();
        let dt = self.last_poll.map_or(0.0, |l| (t - l).clamp(0.0, 0.25)) as f32;
        self.last_poll = Some(t);
        if let Some(ctx) = &self.ctx {
            let counters = &mut self.counters;
            let all = self
                .voices
                .values_mut()
                .chain(self.shots.iter_mut())
                .chain(self.track.iter_mut())
                .chain(self.stinger.iter_mut());
            for p in all {
                if let (Play::Stream(s), Some((gain, _))) = (&mut p.play, &p.nodes) {
                    pump(ctx, gain, s, counters);
                }
            }
        }
        self.voices.retain(|_, v| !v.ended());
        self.shots.retain(|v| !v.ended());
        if self.track.as_ref().is_some_and(Playback::ended) {
            self.track = None;
        }
        if self.stinger.as_ref().is_some_and(Playback::ended) {
            self.stinger = None;
            self.apply_track_duck();
        }
        self.buses.update(dt, self.sounding());
        self.apply_bus_gains();
    }

    /// Every playback that hasn't ended.
    fn playbacks(&self) -> impl Iterator<Item = &Playback> {
        self.voices
            .values()
            .chain(self.shots.iter())
            .chain(self.track.iter())
            .chain(self.stinger.iter())
            .filter(|p| !p.ended())
    }

    /// The buses something plays on.
    fn sounding(&self) -> BusSet {
        let mut s = BusSet::EMPTY;
        for p in self.playbacks() {
            s.insert(p.bus);
        }
        s
    }

    /// The master and bus nodes follow the mixer (a short glide).
    fn apply_bus_gains(&mut self) {
        let Some(ctx) = &self.ctx else { return };
        let at = ctx.current_time();
        for b in Bus::ALL {
            let g = if b == Bus::Master {
                self.buses.gain(Bus::Master)
            } else if self.buses.muted(b) {
                0.0
            } else {
                self.buses.volume(b) * self.buses.duck(b)
            };
            if (g - self.applied[b.index()]).abs() < 1e-4 {
                continue;
            }
            self.applied[b.index()] = g;
            let node = if b == Bus::Master {
                self.master.as_ref()
            } else {
                self.bus_nodes[b.index()].as_ref()
            };
            if let Some(n) = node {
                let _ = n.gain().set_target_at_time(g, at, GAIN_GLIDE);
            }
        }
    }

    /// The track at its volume, ducked while a stinger plays.
    fn apply_track_duck(&self) {
        let duck = if self.stinger.is_some() { DUCK } else { 1.0 };
        if let Some(Playback {
            nodes: Some((gain, _)),
            volume,
            ..
        }) = &self.track
        {
            gain.gain().set_value(volume.max(0.0) * duck);
        }
    }

    fn active(&self) -> usize {
        self.playbacks().count()
    }
}

/// Schedule `s`'s chunks into `out` up to [`LOOKAHEAD`] past the context's
/// time, and retire the chunks that played.
fn pump(ctx: &AudioContext, out: &GainNode, s: &mut Streaming, counters: &mut StreamCounters) {
    let now = ctx.current_time();
    while let Some(c) = s.chunks.front() {
        if c.start + s.seconds(c.frames) > now {
            break;
        }
        counters.played += c.frames as u64;
        s.chunks.pop_front();
    }
    while !s.done && s.next_start - now < LOOKAHEAD {
        s.scratch.clear();
        let more = s.dec.fill(&mut s.scratch, CHUNK_FRAMES, s.looped);
        if s.scratch.is_empty() {
            s.done = true;
            break;
        }
        let Some(chunk) = schedule(ctx, out, s, now) else {
            s.done = true;
            break;
        };
        if chunk.start > s.next_start {
            counters.underruns += 1;
        }
        s.next_start = chunk.start + s.seconds(chunk.frames);
        s.chunks.push_back(chunk);
        s.done = !more;
    }
}

/// One chunk of `s.scratch`, started where the stream is due (or just
/// after now, when that is already past: an underrun).
fn schedule(ctx: &AudioContext, out: &GainNode, s: &Streaming, now: f64) -> Option<Chunk> {
    let frames = s.scratch.len();
    let buf = ctx
        .create_buffer(1, frames as u32, s.dec.rate() as f32)
        .ok()?;
    buf.copy_to_channel(&s.scratch, 0).ok()?;
    let source = ctx.create_buffer_source().ok()?;
    source.set_buffer(Some(&buf));
    source.connect_with_audio_node(out).ok()?;
    let start = if s.next_start < now {
        now + START_DELAY
    } else {
        s.next_start
    };
    scheduled(&source).start_with_when(start).ok()?;
    Some(Chunk {
        source,
        start,
        frames,
    })
}

impl Default for WebAudio {
    fn default() -> Self {
        Self::new()
    }
}

/// Volume and pan on a playback's nodes (constant-sum like the PC's
/// gains; Web Audio's panner is equal-power, close enough by ear).
fn set_params(gain: &GainNode, pan: &StereoPannerNode, params: &SoundParams) {
    gain.gain().set_value(params.volume.max(0.0));
    pan.pan().set_value(params.pan.clamp(-1.0, 1.0));
}

/// A source node's `AudioScheduledSourceNode` side (`start`, `stop`,
/// `onended`; web-sys deprecates the copies on `AudioBufferSourceNode`).
fn scheduled(source: &AudioBufferSourceNode) -> &web_sys::AudioScheduledSourceNode {
    source.as_ref()
}

impl Audio for WebAudio {
    fn play_sound(&mut self, sound: SoundId) {
        self.play_sound_with(sound, &SoundParams::DEFAULT);
    }

    fn play_sound_with(&mut self, sound: SoundId, params: &SoundParams) {
        let Some(bus) = self.sounds.get(&sound.0).map(|c| c.bus) else {
            return;
        };
        // One-shots count against the voice pool but are never refused
        // (the PC mixer's rule).
        self.shots.retain(|v| !v.ended());
        let bus = params.bus.unwrap_or(bus);
        if let Some(p) = self.start(false, sound.0, Kind::Sfx, bus, params, false) {
            self.shots.push(p);
        }
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        self.stop_music();
        let Some(bus) = self.music.get(&music.0).map(|c| c.bus) else {
            return;
        };
        let params = SoundParams::DEFAULT;
        self.track = self.start(true, music.0, Kind::Music, bus, &params, looped);
        self.apply_track_duck();
    }

    fn stop_music(&mut self) {
        if let Some(mut t) = self.track.take() {
            t.stop();
        }
    }

    fn set_master_volume(&mut self, volume: f32) {
        self.set_bus_volume(Bus::Master, volume);
    }

    fn play_voice(
        &mut self,
        sound: SoundId,
        params: &SoundParams,
        looped: bool,
    ) -> Option<VoiceId> {
        self.voices.retain(|_, v| !v.ended());
        self.shots.retain(|v| !v.ended());
        if self.active() >= MAX_VOICES {
            return None;
        }
        let bus = params.bus.unwrap_or(self.sounds.get(&sound.0)?.bus);
        let p = self.start(false, sound.0, Kind::Sfx, bus, params, looped)?;
        self.next_voice = self.next_voice.wrapping_add(1).max(1);
        self.voices.insert(self.next_voice, p);
        Some(VoiceId(self.next_voice))
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        if let Some(p) = self.voices.get_mut(&voice.0) {
            p.volume = params.volume;
            if let Some((gain, pan)) = &p.nodes {
                set_params(gain, pan, params);
            }
        }
    }

    fn stop_voice(&mut self, voice: VoiceId) {
        if let Some(mut v) = self.voices.remove(&voice.0) {
            v.stop();
        }
    }

    fn voice_playing(&self, voice: VoiceId) -> bool {
        self.voices.get(&voice.0).is_some_and(|v| !v.ended())
    }

    fn play_stinger(&mut self, music: MusicId) {
        if let Some(mut s) = self.stinger.take() {
            s.stop();
        }
        if let Some(bus) = self.music.get(&music.0).map(|c| c.bus) {
            // Stingers play at their bus's level, over the ducked track.
            let params = SoundParams::DEFAULT;
            self.stinger = self.start(true, music.0, Kind::Stinger, bus, &params, false);
        }
        self.apply_track_duck();
    }

    fn stinger_playing(&self) -> bool {
        self.stinger
            .as_ref()
            .is_some_and(|s| s.kind == Kind::Stinger && !s.ended())
    }

    fn set_music_volume(&mut self, volume: f32) {
        self.set_bus_volume(Bus::Music, volume);
    }

    fn set_bus_volume(&mut self, bus: Bus, volume: f32) {
        self.buses.set_volume(bus, volume);
        self.apply_bus_gains();
    }

    fn bus_volume(&self, bus: Bus) -> f32 {
        self.buses.volume(bus)
    }

    fn set_bus_muted(&mut self, bus: Bus, muted: bool) {
        self.buses.set_muted(bus, muted);
        self.apply_bus_gains();
    }

    fn bus_muted(&self, bus: Bus) -> bool {
        self.buses.muted(bus)
    }

    fn set_duck(&mut self, rule: DuckRule) {
        self.buses.set_duck(rule);
        self.apply_bus_gains();
    }

    fn clear_ducks(&mut self) {
        self.buses.clear_ducks();
        self.apply_bus_gains();
    }

    fn bus_duck(&self, bus: Bus) -> f32 {
        self.buses.duck(bus)
    }

    fn audio_stats(&self) -> AudioStats {
        let resident: usize = self
            .sounds
            .values()
            .chain(self.music.values())
            .map(Clip::bytes)
            .sum();
        let at = self.ctx.as_ref().map_or(0.0, |c| c.current_time());
        let (mut voices, mut streams, mut held, mut playing) = (0u16, 0u16, 0usize, 0u64);
        for p in self.playbacks() {
            voices += 1;
            if let Play::Stream(s) = &p.play {
                streams += 1;
                held += s.held();
                playing += s.playing(at) as u64;
            }
        }
        AudioStats {
            voices,
            streams,
            resident_bytes: resident as u32,
            stream_bytes: (held * 4) as u32,
            underruns: self.counters.underruns,
            streamed_frames: (self.counters.played + playing) as u32,
        }
    }
}
