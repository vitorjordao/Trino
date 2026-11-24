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
//! **Pitch, music control, bus filters** (Bongo-Kart #175, #177): a sound's
//! pitch is its source's `playbackRate` (streamed sounds ignore it, as on
//! the PC). A music track gets a level gain ahead of its volume: layers and
//! crossfades ramp it on the context's clock (`linearRampToValueAtTime`),
//! and the backend keeps the same ramp to answer [`Audio::music_layer`].
//! Pausing stops the sources but keeps what they play: a buffer's
//! `AudioBuffer` and position, a stream's scheduled chunks (their buffers
//! and how much of the first had played), so resuming schedules them again
//! and nothing is decoded twice. All the layers resume at one context time,
//! in step. A synced crossfade starts the new track where the old one is: a
//! buffer at that offset, a stream after a decoder seek. A bus filter is a
//! `BiquadFilterNode` spliced between the bus gain and the master (or after
//! the master), and taken out again for `None`.
//!
//! **No Web Audio** (some embedded WebKit builds): the backend runs silent,
//! like the PC without an output device, and keeps the API's meaning —
//! voices, stingers and their ends follow the page's clock — so a game and
//! the self-test behave the same.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use trino_core::{
    Audio, AudioFeatures, AudioStats, Bus, BusFilter, BusMixer, BusSet, BusSettings, DuckRule,
    MAX_MUSIC_LAYERS, MusicId, SoundBlob, SoundCodec, SoundId, SoundParams, VoiceId,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{
    AudioBuffer, AudioBufferSourceNode, AudioContext, AudioContextState, AudioNode,
    BiquadFilterNode, BiquadFilterType, GainNode, StereoPannerNode,
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
    /// Samples before the first chunk in `chunks`: where the stream started
    /// (a seek) plus every chunk that played.
    base: u64,
    /// Paused: the chunks' buffers to schedule again, and how much of the
    /// first had played.
    held: Vec<(AudioBuffer, usize)>,
    held_offset: usize,
    paused: bool,
}

impl Streaming {
    /// Samples into the stream at context time `now` (a loop counts on:
    /// the caller wraps it).
    fn position(&self, now: f64) -> u64 {
        self.base
            + if self.paused {
                self.held_offset as u64
            } else {
                self.playing(now) as u64
            }
    }

    /// Stop the scheduled chunks, keeping their buffers.
    fn pause(&mut self, now: f64) {
        self.held_offset = self.playing(now);
        self.held.clear();
        for c in self.chunks.drain(..) {
            let _ = scheduled(&c.source).stop();
            let _ = c.source.disconnect();
            if let Some(b) = c.source.buffer() {
                self.held.push((b, c.frames));
            }
        }
        self.paused = true;
    }

    /// Schedule the kept chunks again from context time `when`.
    fn resume(&mut self, ctx: &AudioContext, out: &AudioNode, when: f64) {
        let rate = self.dec.rate() as f64;
        let mut t = when;
        for (k, (buf, frames)) in self.held.drain(..).enumerate() {
            let skip = if k == 0 {
                self.held_offset.min(frames)
            } else {
                0
            };
            let Ok(source) = ctx.create_buffer_source() else {
                continue;
            };
            source.set_buffer(Some(&buf));
            if source.connect_with_audio_node(out).is_err()
                || source
                    .start_with_when_and_grain_offset(t, skip as f64 / rate)
                    .is_err()
            {
                continue;
            }
            self.chunks.push_back(Chunk {
                source,
                start: t - skip as f64 / rate,
                frames,
            });
            t += (frames - skip) as f64 / rate;
        }
        self.held_offset = 0;
        self.next_start = t;
        self.paused = false;
    }

    fn seconds(&self, frames: usize) -> f64 {
        frames as f64 / self.dec.rate() as f64
    }

    /// Decoded samples in memory (scheduled chunks, the decoder's carry).
    fn held(&self) -> usize {
        self.chunks.iter().map(|c| c.frames).sum::<usize>()
            + self.held.iter().map(|(_, f)| f).sum::<usize>()
            + self.dec.held()
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
        self.held.clear();
        self.done = true;
    }
}

/// A music track's level: a linear ramp on the clock it plays by (the
/// context's, or the page's when silent), mirrored on its level gain.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Level {
    from: f32,
    to: f32,
    t0: f64,
    t1: f64,
}

impl Level {
    const fn flat(v: f32) -> Level {
        Level {
            from: v,
            to: v,
            t0: 0.0,
            t1: 0.0,
        }
    }

    fn at(&self, t: f64) -> f32 {
        if t >= self.t1 || self.t1 <= self.t0 {
            self.to
        } else if t <= self.t0 {
            self.from
        } else {
            self.from + (self.to - self.from) * ((t - self.t0) / (self.t1 - self.t0)) as f32
        }
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
    /// Music: the level gain ahead of `nodes` (layers, crossfades).
    level_node: Option<GainNode>,
    /// The game's volume for it (the track ducks under a stinger).
    volume: f32,
    /// Its pitch (a buffer's `playbackRate`; 1 for streams).
    rate: f32,
    play: Play,
    ended: Rc<Cell<bool>>,
    /// Silent playbacks: when it ends (seconds, page clock); `None` loops.
    until: Option<f64>,
    looped: bool,
    /// Its length in seconds (at pitch 1).
    length: f64,
    /// Music: its layer and level.
    layer: u8,
    level: Level,
    /// Buffers and silent playbacks: seconds into the track at clock time
    /// `since`; paused, `since` is `None` and `offset` holds.
    offset: f64,
    since: Option<f64>,
    paused: bool,
}

impl Playback {
    fn ended(&self) -> bool {
        if self.paused {
            return false;
        }
        if self.ended.get() || self.until.is_some_and(|t| now() >= t) {
            return true;
        }
        matches!(&self.play, Play::Stream(s) if s.done && s.chunks.is_empty())
    }

    fn stop(&mut self) {
        self.ended.set(true);
        self.paused = false;
        match &mut self.play {
            Play::Buffer { source, .. } => {
                scheduled(source).set_onended(None);
                let _ = scheduled(source).stop();
            }
            Play::Stream(s) => s.stop(),
            Play::Silent => {}
        }
        if let Some((_, pan)) = &self.nodes {
            let _ = pan.disconnect();
        }
    }

    /// Where its sources connect: the level gain, else the volume gain.
    fn input(&self) -> Option<&GainNode> {
        self.level_node
            .as_ref()
            .or(self.nodes.as_ref().map(|(g, _)| g))
    }

    /// Seconds into the track at clock time `t` (a loop wraps).
    fn position(&self, t: f64) -> f64 {
        let at = match &self.play {
            Play::Stream(s) => s.position(t) as f64 / s.dec.rate().max(1) as f64,
            _ => self.offset + self.since.map_or(0.0, |s| (t - s).max(0.0)) * self.rate as f64,
        };
        if self.length <= 0.0 {
            0.0
        } else if self.looped {
            at % self.length
        } else {
            at.min(self.length)
        }
    }

    /// Ramp the level to `to` over `secs` from clock time `t`.
    fn ramp(&mut self, t: f64, to: f32, secs: f64) {
        let from = self.level.at(t);
        self.level = Level {
            from,
            to,
            t0: t,
            t1: t + secs.max(0.0),
        };
        if let Some(n) = &self.level_node {
            let g = n.gain();
            let _ = g.cancel_scheduled_values(t);
            if secs > 0.0 {
                let _ = g.set_value_at_time(from, t);
                let _ = g.linear_ramp_to_value_at_time(to, t + secs);
            } else {
                g.set_value(to);
            }
        }
    }

    /// Hold the track where it is at clock time `t`.
    fn pause(&mut self, t: f64) {
        if self.paused {
            return;
        }
        let at = self.position(t);
        match &mut self.play {
            Play::Stream(s) => s.pause(t),
            Play::Buffer { source, .. } => {
                self.offset = at;
                self.since = None;
                scheduled(source).set_onended(None);
                let _ = scheduled(source).stop();
            }
            Play::Silent => {
                self.offset = at;
                self.since = None;
                self.until = None;
            }
        }
        self.paused = true;
    }

    /// Go on from where [`Playback::pause`] held it, at clock time `when`.
    fn resume(&mut self, ctx: Option<&AudioContext>, when: f64) {
        if !self.paused {
            return;
        }
        self.paused = false;
        let input = self.input().cloned();
        match (&mut self.play, ctx, input) {
            (Play::Stream(s), Some(ctx), Some(input)) => s.resume(ctx, &input, when),
            (Play::Buffer { source, _on_ended }, Some(ctx), Some(input)) => {
                let Ok(fresh) = ctx.create_buffer_source() else {
                    return;
                };
                fresh.set_buffer(source.buffer().as_ref());
                fresh.set_loop(self.looped);
                fresh.playback_rate().set_value(self.rate);
                let flag = self.ended.clone();
                let on_ended = Closure::<dyn FnMut()>::new(move || flag.set(true));
                scheduled(&fresh).set_onended(Some(on_ended.as_ref().unchecked_ref()));
                if fresh.connect_with_audio_node(&input).is_ok()
                    && fresh
                        .start_with_when_and_grain_offset(when, self.offset)
                        .is_ok()
                {
                    *source = fresh;
                    *_on_ended = on_ended;
                }
                self.since = Some(when);
            }
            _ => {
                self.since = Some(when);
                if !self.looped {
                    self.until = Some(now() + (self.length - self.offset).max(0.0));
                }
            }
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
    /// The music: one track, or up to [`MAX_MUSIC_LAYERS`] layers.
    tracks: Vec<Playback>,
    /// A crossfade's old tracks, fading out until their clock time.
    leaving: Vec<(Playback, f64)>,
    stinger: Option<Playback>,
    /// A bus's filter node (the master's after the master gain) and the
    /// settings the game asked for (kept when silent).
    filters: Vec<Option<BiquadFilterNode>>,
    filter_set: [Option<BusFilter>; Bus::COUNT],
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
            tracks: Vec::new(),
            leaving: Vec::new(),
            stinger: None,
            filters: (0..Bus::COUNT).map(|_| None).collect(),
            filter_set: [None; Bus::COUNT],
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

    /// The clock playbacks run by: the context's, or the page's when silent.
    fn clock(&self) -> f64 {
        self.ctx.as_ref().map_or_else(now, |c| c.current_time())
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
        self.start_at(music, id, kind, bus, params, looped, Start::NOW)
    }

    /// [`WebAudio::start`] at a context time, from an offset, with a level
    /// gain (music).
    #[allow(clippy::too_many_arguments)]
    fn start_at(
        &mut self,
        music: bool,
        id: u32,
        kind: Kind,
        bus: Bus,
        params: &SoundParams,
        looped: bool,
        how: Start,
    ) -> Option<Playback> {
        let clock = self.clock();
        let clip = if music {
            self.music.get(&id)?
        } else {
            self.sounds.get(&id)?
        };
        let streamed = matches!(clip.data, ClipData::Stream { .. });
        let rate = if streamed { 1.0 } else { params.rate() };
        let length = clip.seconds();
        let offset = if length > 0.0 && looped {
            how.offset % length
        } else {
            how.offset.min(length)
        };
        let ended = Rc::new(Cell::new(false));
        let when = how.when.unwrap_or(clock);
        let silent = Playback {
            kind,
            bus,
            nodes: None,
            level_node: None,
            volume: params.volume,
            rate,
            play: Play::Silent,
            ended: ended.clone(),
            until: (!looped).then(|| now() + (length - offset) / rate as f64),
            looped,
            length,
            layer: 0,
            level: Level::flat(how.level),
            offset,
            since: Some(when),
            paused: false,
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
        let level_node = if how.level_node {
            let n = ctx.create_gain().ok()?;
            n.gain().set_value(how.level);
            n.connect_with_audio_node(&gain).ok()?;
            Some(n)
        } else {
            None
        };
        let input: &GainNode = level_node.as_ref().unwrap_or(&gain);
        let play = match &clip.data {
            ClipData::Buffer(None) => return Some(silent),
            ClipData::Buffer(Some(buffer)) => {
                let source = ctx.create_buffer_source().ok()?;
                source.set_buffer(Some(buffer));
                source.set_loop(looped);
                source.playback_rate().set_value(rate);
                source.connect_with_audio_node(input).ok()?;
                let flag = ended.clone();
                let on_ended = Closure::<dyn FnMut()>::new(move || flag.set(true));
                scheduled(&source).set_onended(Some(on_ended.as_ref().unchecked_ref()));
                if how.when.is_none() && offset == 0.0 {
                    scheduled(&source).start().ok()?;
                } else {
                    source.start_with_when_and_grain_offset(when, offset).ok()?;
                }
                Play::Buffer {
                    source,
                    _on_ended: on_ended,
                }
            }
            ClipData::Stream { codec, bytes } => {
                let mut dec =
                    StreamDecoder::open(*codec, clip.rate, clip.frames as u32, bytes.clone())
                        .ok()?;
                let frame = (offset * dec.rate() as f64) as usize;
                let base = if frame > 0 && dec.seek(frame) {
                    frame as u64
                } else {
                    0
                };
                let mut s = Box::new(Streaming {
                    dec,
                    looped,
                    next_start: how.when.unwrap_or(ctx.current_time() + START_DELAY),
                    chunks: VecDeque::new(),
                    done: false,
                    scratch: Vec::with_capacity(CHUNK_FRAMES),
                    base,
                    held: Vec::new(),
                    held_offset: 0,
                    paused: false,
                });
                // Fill the lookahead now: the first chunks are due at once.
                pump(ctx, input, &mut s, &mut self.counters);
                Play::Stream(s)
            }
        };
        Some(Playback {
            nodes: Some((gain, pan)),
            level_node,
            play,
            until: None,
            ..silent
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
                .chain(self.tracks.iter_mut())
                .chain(self.leaving.iter_mut().map(|(p, _)| p))
                .chain(self.stinger.iter_mut());
            for p in all {
                let input = p.input().cloned();
                if let (Play::Stream(s), Some(input)) = (&mut p.play, input) {
                    pump(ctx, &input, s, counters);
                }
            }
        }
        self.voices.retain(|_, v| !v.ended());
        self.shots.retain(|v| !v.ended());
        self.tracks.retain(|t| !t.ended());
        let clock = self.clock();
        self.leaving.retain_mut(|(p, until)| {
            if clock >= *until || p.ended() {
                p.stop();
                false
            } else {
                true
            }
        });
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
            .chain(self.tracks.iter())
            .chain(self.leaving.iter().map(|(p, _)| p))
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

    /// The music at its volume, ducked while a stinger plays.
    fn apply_track_duck(&self) {
        let duck = if self.stinger.is_some() { DUCK } else { 1.0 };
        let all = self
            .tracks
            .iter()
            .chain(self.leaving.iter().map(|(p, _)| p));
        for p in all {
            if let Some((gain, _)) = &p.nodes {
                gain.gain().set_value(p.volume.max(0.0) * duck);
            }
        }
    }

    /// Stop the music (every layer and any track still fading out).
    fn stop_tracks(&mut self) {
        for t in self.tracks.iter_mut() {
            t.stop();
        }
        for (t, _) in self.leaving.iter_mut() {
            t.stop();
        }
        self.tracks.clear();
        self.leaving.clear();
    }

    /// Start music `id` as layer `layer` at `level`, from `offset` seconds,
    /// at context time `when`.
    fn start_track(
        &mut self,
        id: MusicId,
        looped: bool,
        layer: u8,
        level: f32,
        offset: f64,
        when: Option<f64>,
    ) -> Option<Playback> {
        let bus = self.music.get(&id.0)?.bus;
        let how = Start {
            offset,
            when,
            level,
            level_node: true,
        };
        let mut p = self.start_at(
            true,
            id.0,
            Kind::Music,
            bus,
            &SoundParams::DEFAULT,
            looped,
            how,
        )?;
        p.layer = layer;
        Some(p)
    }

    /// The node a bus's filter goes after, and what it feeds.
    fn filter_ends(&self, bus: Bus) -> Option<(AudioNode, AudioNode)> {
        let ctx = self.ctx.as_ref()?;
        if bus == Bus::Master {
            let m = self.master.as_ref()?;
            Some((m.clone().into(), ctx.destination().into()))
        } else {
            let n = self.bus_nodes.get(bus.index())?.as_ref()?;
            Some((n.clone().into(), self.master.as_ref()?.clone().into()))
        }
    }

    fn active(&self) -> usize {
        self.playbacks().count()
    }
}

/// Schedule `s`'s chunks into `out` up to [`LOOKAHEAD`] past the context's
/// time, and retire the chunks that played.
fn pump(ctx: &AudioContext, out: &AudioNode, s: &mut Streaming, counters: &mut StreamCounters) {
    if s.paused {
        return;
    }
    let now = ctx.current_time();
    while let Some(c) = s.chunks.front() {
        if c.start + s.seconds(c.frames) > now {
            break;
        }
        counters.played += c.frames as u64;
        s.base += c.frames as u64;
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
fn schedule(ctx: &AudioContext, out: &AudioNode, s: &Streaming, now: f64) -> Option<Chunk> {
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

/// How [`WebAudio::start_at`] starts a playback.
#[derive(Clone, Copy)]
struct Start {
    /// Seconds into the clip.
    offset: f64,
    /// Context time (`None`: now).
    when: Option<f64>,
    /// The level gain's value, and whether it has one (music).
    level: f32,
    level_node: bool,
}

impl Start {
    const NOW: Start = Start {
        offset: 0.0,
        when: None,
        level: 1.0,
        level_node: false,
    };
}

/// A new pitch on a playback: its source's rate, or a silent one's end.
fn set_rate(p: &mut Playback, rate: f32) {
    if matches!(p.play, Play::Stream(_)) || p.rate == rate {
        return;
    }
    if let Play::Buffer { source, .. } = &p.play {
        source.playback_rate().set_value(rate);
    }
    if let Some(until) = p.until {
        let t = now();
        p.until = Some(t + (until - t).max(0.0) * p.rate as f64 / rate as f64);
    }
    p.rate = rate;
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
        if let Some(t) = self.start_track(music, looped, 0, 1.0, 0.0, None) {
            self.tracks.push(t);
        }
        self.apply_track_duck();
    }

    fn stop_music(&mut self) {
        self.stop_tracks();
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
            set_rate(p, params.rate());
        }
    }

    fn set_voice_pitch(&mut self, voice: VoiceId, pitch: f32) {
        if let Some(p) = self.voices.get_mut(&voice.0) {
            set_rate(p, trino_core::clamp_pitch(pitch));
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

    fn audio_features(&self) -> AudioFeatures {
        AudioFeatures::ALL
    }

    fn pause_music(&mut self) {
        let t = self.clock();
        // A crossfade's old track has nothing to come back to.
        for (p, _) in self.leaving.iter_mut() {
            p.stop();
        }
        self.leaving.clear();
        for p in self.tracks.iter_mut() {
            p.pause(t);
        }
    }

    fn resume_music(&mut self) {
        // Every layer from one context time: they stay in step.
        let when = self.clock() + if self.ctx.is_some() { START_DELAY } else { 0.0 };
        for p in self.tracks.iter_mut() {
            p.resume(self.ctx.as_ref(), when);
        }
    }

    fn music_paused(&self) -> bool {
        self.tracks.first().is_some_and(|p| p.paused)
    }

    fn music_position(&self) -> Option<f32> {
        let t = self.clock();
        self.tracks
            .iter()
            .min_by_key(|p| p.layer)
            .map(|p| p.position(t) as f32)
    }

    fn crossfade_music(&mut self, music: MusicId, looped: bool, fade_ms: u32, sync: bool) {
        if !self.music.contains_key(&music.0) {
            return;
        }
        let clock = self.clock();
        let delay = if self.ctx.is_some() { START_DELAY } else { 0.0 };
        let when = clock + delay;
        // In sync: where the old track will be when the new one starts.
        let offset = if sync {
            self.music_position().map_or(0.0, |p| p as f64 + delay)
        } else {
            0.0
        };
        let paused = self.music_paused();
        let fade = fade_ms as f64 / 1000.0;
        for mut t in std::mem::take(&mut self.tracks) {
            if fade <= 0.0 || t.paused {
                t.stop();
            } else {
                t.ramp(when, 0.0, fade);
                self.leaving.push((t, when + fade));
            }
        }
        let level = if fade > 0.0 { 0.0 } else { 1.0 };
        if let Some(mut t) = self.start_track(music, looped, 0, level, offset, Some(when)) {
            if fade > 0.0 {
                t.ramp(when, 1.0, fade);
            }
            if paused {
                t.pause(when);
            }
            self.tracks.push(t);
        }
        self.apply_track_duck();
    }

    fn play_music_layers(&mut self, layers: &[MusicId], looped: bool) {
        self.stop_music();
        let delay = if self.ctx.is_some() { START_DELAY } else { 0.0 };
        let when = self.clock() + delay;
        for (i, &id) in layers.iter().take(MAX_MUSIC_LAYERS).enumerate() {
            let level = if i == 0 { 1.0 } else { 0.0 };
            if let Some(t) = self.start_track(id, looped, i as u8, level, 0.0, Some(when)) {
                self.tracks.push(t);
            }
        }
        self.apply_track_duck();
    }

    fn set_music_layer(&mut self, layer: usize, gain: f32, fade_ms: u32) {
        let t = self.clock();
        let gain = if gain.is_nan() {
            0.0
        } else {
            gain.clamp(0.0, 1.0)
        };
        for p in self.tracks.iter_mut().filter(|p| p.layer as usize == layer) {
            p.ramp(t, gain, fade_ms as f64 / 1000.0);
        }
    }

    fn music_layer(&self, layer: usize) -> Option<f32> {
        let t = self.clock();
        self.tracks
            .iter()
            .find(|p| p.layer as usize == layer)
            .map(|p| p.level.at(t))
    }

    fn set_bus_filter(&mut self, bus: Bus, filter: Option<BusFilter>) {
        self.filter_set[bus.index()] = filter;
        let Some((from, to)) = self.filter_ends(bus) else {
            return;
        };
        let at = self.clock();
        let slot = bus.index();
        match (filter, self.filters[slot].take()) {
            (Some(f), Some(node)) => {
                let _ = node
                    .frequency()
                    .set_target_at_time(f.cutoff(), at, GAIN_GLIDE);
                self.filters[slot] = Some(node);
            }
            (Some(f), None) => {
                let Some(ctx) = &self.ctx else { return };
                let Ok(node) = ctx.create_biquad_filter() else {
                    return;
                };
                node.set_type(BiquadFilterType::Lowpass);
                node.q().set_value(core::f32::consts::FRAC_1_SQRT_2);
                node.frequency().set_value(f.cutoff());
                let _ = from.disconnect();
                let _ = from.connect_with_audio_node(&node);
                let _ = node.connect_with_audio_node(&to);
                self.filters[slot] = Some(node);
            }
            (None, Some(node)) => {
                let _ = from.disconnect();
                let _ = node.disconnect();
                let _ = from.connect_with_audio_node(&to);
            }
            (None, None) => {}
        }
    }

    fn bus_filter(&self, bus: Bus) -> Option<BusFilter> {
        self.filter_set[bus.index()]
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
                playing += if s.paused {
                    s.held_offset
                } else {
                    s.playing(at)
                } as u64;
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
