//! [`trino_core::Audio`] on Web Audio.
//!
//! Every playback is an `AudioBufferSourceNode` → gain → stereo panner →
//! (music or effects bus) → master gain → speakers; the browser mixes.
//! Sounds are uploaded as `AudioBuffer`s at their baked rate (the browser
//! resamples). Music ducks to [`DUCK`] of its volume while a stinger plays,
//! like the PC mixer.
//!
//! **Autoplay:** every browser (Safari strictest) creates the context
//! suspended until the user interacts with the page. Playback requests
//! before that are scheduled and play once [`WebAudio::unlock`] resumes it
//! — the runtime calls it on the first click, key or touch, and shows a
//! banner until the context runs. A refused `resume()` is logged, never an
//! unhandled rejection.
//!
//! **No Web Audio** (some embedded WebKit builds): the backend runs silent,
//! like the PC without an output device, and keeps the API's meaning —
//! voices, stingers and their ends follow the page's clock — so a game and
//! the self-test behave the same.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use trino_core::{Audio, MusicId, SoundId, SoundParams, VoiceId};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{
    AudioBuffer, AudioBufferSourceNode, AudioContext, AudioContextState, GainNode, StereoPannerNode,
};

/// Music level while a stinger plays (the PC mixer's `DUCK`).
pub const DUCK: f32 = 0.3;
/// Most steerable voices at once (the PC's).
const MAX_VOICES: usize = 24;

/// A sound or track: its buffer (none when running silent) and length.
struct Clip {
    buffer: Option<AudioBuffer>,
    seconds: f64,
}

/// One playback's audio graph.
struct Nodes {
    source: AudioBufferSourceNode,
    gain: GainNode,
    pan: StereoPannerNode,
    _on_ended: Closure<dyn FnMut()>,
}

/// One playback: its nodes, or none when running silent (it then ends by
/// the clock).
struct Playback {
    nodes: Option<Nodes>,
    ended: Rc<Cell<bool>>,
    /// Silent playbacks: when it ends (seconds, page clock); `None` loops.
    until: Option<f64>,
}

impl Playback {
    fn ended(&self) -> bool {
        self.ended.get() || self.until.is_some_and(|t| now() >= t)
    }
}

/// The page's clock in seconds.
fn now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now() / 1000.0)
}

pub struct WebAudio {
    ctx: Option<AudioContext>,
    master: Option<GainNode>,
    effects: Option<GainNode>,
    music_bus: Option<GainNode>,
    sounds: HashMap<u32, Clip>,
    music: HashMap<u32, Clip>,
    voices: HashMap<u32, Playback>,
    next_voice: u32,
    track: Option<Playback>,
    stinger: Option<Playback>,
    music_volume: f32,
}

impl WebAudio {
    /// A context (suspended until [`WebAudio::unlock`] where the browser
    /// requires a gesture). Without Web Audio the backend is silent, never
    /// an error: audio must not break a page.
    pub fn new() -> Self {
        let ctx = AudioContext::new().ok();
        let gain = |ctx: &AudioContext| ctx.create_gain().ok();
        let master = ctx.as_ref().and_then(gain);
        let effects = ctx.as_ref().and_then(gain);
        let music_bus = ctx.as_ref().and_then(gain);
        let ctx = match (ctx, &master, &effects, &music_bus) {
            (Some(ctx), Some(m), Some(e), Some(b)) => {
                let _ = m.connect_with_audio_node(&ctx.destination());
                let _ = e.connect_with_audio_node(m);
                let _ = b.connect_with_audio_node(m);
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
            effects,
            music_bus,
            sounds: HashMap::new(),
            music: HashMap::new(),
            voices: HashMap::new(),
            next_voice: 0,
            track: None,
            stinger: None,
            music_volume: 1.0,
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

    fn clip(&self, rate: u32, samples: &[f32]) -> Option<Clip> {
        if samples.is_empty() || rate == 0 {
            return None;
        }
        let buffer = self.ctx.as_ref().and_then(|ctx| {
            let buf = ctx
                .create_buffer(1, samples.len() as u32, rate as f32)
                .ok()?;
            buf.copy_to_channel(samples, 0).ok()?;
            Some(buf)
        });
        Some(Clip {
            buffer,
            seconds: samples.len() as f64 / rate as f64,
        })
    }

    /// Register (or replace) a sound effect: mono samples at `rate`.
    pub fn upload_sound(&mut self, id: SoundId, rate: u32, samples: &[f32]) {
        if let Some(c) = self.clip(rate, samples) {
            self.sounds.insert(id.0, c);
        }
    }

    /// Register (or replace) a music track.
    pub fn upload_music(&mut self, id: MusicId, rate: u32, samples: &[f32]) {
        if let Some(c) = self.clip(rate, samples) {
            self.music.insert(id.0, c);
        }
    }

    /// Start `clip` into `bus` with `params`: its graph, or a silent
    /// playback that ends by the clock.
    fn start(
        &self,
        clip: &Clip,
        bus: Option<&GainNode>,
        params: &SoundParams,
        looped: bool,
    ) -> Option<Playback> {
        let ended = Rc::new(Cell::new(false));
        let (Some(ctx), Some(buffer), Some(bus)) = (&self.ctx, &clip.buffer, bus) else {
            return Some(Playback {
                nodes: None,
                ended,
                until: (!looped).then(|| now() + clip.seconds),
            });
        };
        let source = ctx.create_buffer_source().ok()?;
        source.set_buffer(Some(buffer));
        source.set_loop(looped);
        let gain = ctx.create_gain().ok()?;
        let pan = ctx.create_stereo_panner().ok()?;
        source.connect_with_audio_node(&gain).ok()?;
        gain.connect_with_audio_node(&pan).ok()?;
        pan.connect_with_audio_node(bus).ok()?;
        set_params(&gain, &pan, params);
        let flag = ended.clone();
        let on_ended = Closure::<dyn FnMut()>::new(move || flag.set(true));
        scheduled(&source).set_onended(Some(on_ended.as_ref().unchecked_ref()));
        scheduled(&source).start().ok()?;
        Some(Playback {
            nodes: Some(Nodes {
                source,
                gain,
                pan,
                _on_ended: on_ended,
            }),
            ended,
            until: None,
        })
    }

    /// Per frame: forget finished voices, bring the music back after a
    /// stinger.
    pub fn poll(&mut self) {
        self.voices.retain(|_, v| !v.ended());
        if self.stinger.as_ref().is_some_and(Playback::ended) {
            self.stinger = None;
            self.apply_music_volume();
        }
    }

    fn apply_music_volume(&self) {
        if let Some(bus) = &self.music_bus {
            let duck = if self.stinger.is_some() { DUCK } else { 1.0 };
            bus.gain().set_value(self.music_volume * duck);
        }
    }

    fn stop(p: &Playback) {
        p.ended.set(true);
        if let Some(n) = &p.nodes {
            let _ = scheduled(&n.source).stop();
            scheduled(&n.source).set_onended(None);
            let _ = n.pan.disconnect();
        }
    }
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
        let Some(clip) = self.sounds.get(&sound.0) else {
            return;
        };
        // Fire and forget: the audio graph keeps the nodes alive until the
        // sound ends.
        if let Some(Playback { nodes: Some(n), .. }) =
            self.start(clip, self.effects.as_ref(), params, false)
        {
            scheduled(&n.source).set_onended(None);
        }
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        self.stop_music();
        if let Some(clip) = self.music.get(&music.0) {
            self.track = self.start(clip, self.music_bus.as_ref(), &SoundParams::DEFAULT, looped);
        }
    }

    fn stop_music(&mut self) {
        if let Some(t) = self.track.take() {
            Self::stop(&t);
        }
    }

    fn set_master_volume(&mut self, volume: f32) {
        if let Some(m) = &self.master {
            m.gain().set_value(volume.clamp(0.0, 1.0));
        }
    }

    fn play_voice(
        &mut self,
        sound: SoundId,
        params: &SoundParams,
        looped: bool,
    ) -> Option<VoiceId> {
        self.voices.retain(|_, v| !v.ended());
        if self.voices.len() >= MAX_VOICES {
            return None;
        }
        let clip = self.sounds.get(&sound.0)?;
        let p = self.start(clip, self.effects.as_ref(), params, looped)?;
        self.next_voice = self.next_voice.wrapping_add(1).max(1);
        self.voices.insert(self.next_voice, p);
        Some(VoiceId(self.next_voice))
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        if let Some(Playback { nodes: Some(n), .. }) = self.voices.get(&voice.0) {
            set_params(&n.gain, &n.pan, params);
        }
    }

    fn stop_voice(&mut self, voice: VoiceId) {
        if let Some(v) = self.voices.remove(&voice.0) {
            Self::stop(&v);
        }
    }

    fn voice_playing(&self, voice: VoiceId) -> bool {
        self.voices.get(&voice.0).is_some_and(|v| !v.ended())
    }

    fn play_stinger(&mut self, music: MusicId) {
        if let Some(s) = self.stinger.take() {
            Self::stop(&s);
        }
        // Stingers play at the music's volume, past the duck.
        let Some(clip) = self.music.get(&music.0) else {
            return;
        };
        let params = SoundParams {
            volume: self.music_volume,
            pan: 0.0,
        };
        self.stinger = self.start(clip, self.effects.as_ref(), &params, false);
        self.apply_music_volume();
    }

    fn stinger_playing(&self) -> bool {
        self.stinger.as_ref().is_some_and(|s| !s.ended())
    }

    fn set_music_volume(&mut self, volume: f32) {
        self.music_volume = volume.clamp(0.0, 1.0);
        self.apply_music_volume();
    }
}
