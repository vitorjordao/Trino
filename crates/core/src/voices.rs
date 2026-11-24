//! Positional voices with a budget, priorities and doppler (Bongo-Kart
//! #179, ADR 0044 §2): [`VoicePool`] keeps up to `N` sounds placed in the
//! world, each a backend voice while it is audible, and decides who gets a
//! voice when more want one than the budget (or the backend) allows.
//!
//! - **Priority, then loudness:** a sound that wants a voice when the pool
//!   is full takes the slot of the lowest-ranked one — the lowest
//!   [`SoundEmitter::priority`], the quietest among equals — if it ranks above
//!   it, else it is refused. The same rule frees a backend voice when the
//!   backend runs out first (the N64 has four).
//! - **Virtual loops:** a looping sound out of range (or stolen from the
//!   backend) keeps its slot and its handle without a voice, and starts
//!   again from its beginning when it is heard again. A one-shot out of
//!   range stops for good.
//! - **Doppler:** each frame's pitch is the emitter's own times
//!   [`Doppler::pitch`] of the listener's and the emitter's velocities,
//!   within [`PITCH_MIN`]..=[`PITCH_MAX`] (the backends' range).
//!
//! No allocation and no backend code: it drives any [`Audio`] through
//! `play_voice` / `set_voice` / `stop_voice`, so every target has it.

use crate::audio::{
    Audio, Bus, Listener, PITCH_MAX, PITCH_MIN, SoundId, SoundParams, VoiceId, clamp_pitch,
    spatialize,
};
use crate::math::Vec3;

/// A sound placed in the world ([`VoicePool::play_voice_at`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SoundEmitter {
    pub sound: SoundId,
    /// Where it is and how fast it moves (world units, units per second).
    pub pos: Vec3,
    pub vel: Vec3,
    /// Heard within this distance ([`spatialize`]'s falloff).
    pub radius: f32,
    /// Its volume up close.
    pub volume: f32,
    pub looped: bool,
    /// Who keeps a voice when there are too few: higher wins.
    pub priority: u8,
    /// Its own pitch (an engine's revs), before the doppler shift.
    pub pitch: f32,
    /// Its bus (`None`: the sound's).
    pub bus: Option<Bus>,
}

impl SoundEmitter {
    /// A one-shot at `pos`, standing still, at full volume, priority 128.
    pub const fn new(sound: SoundId, pos: Vec3, radius: f32) -> Self {
        SoundEmitter {
            sound,
            pos,
            vel: Vec3::ZERO,
            radius,
            volume: 1.0,
            looped: false,
            priority: 128,
            pitch: 1.0,
            bus: None,
        }
    }
}

/// How velocities shift pitch: `(c + v_listener) / (c + v_source)` along
/// the line between them, each speed `scale`d and kept under 0.9 `c`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Doppler {
    /// The speed of sound in world units per second (343 for meters).
    pub speed_of_sound: f32,
    /// 1 = physical, more = exaggerated (arcade), 0 = off.
    pub scale: f32,
}

impl Doppler {
    pub const PHYSICAL: Doppler = Doppler {
        speed_of_sound: 343.0,
        scale: 1.0,
    };
    pub const OFF: Doppler = Doppler {
        speed_of_sound: 343.0,
        scale: 0.0,
    };

    /// The pitch factor heard by `listener` (moving at `listener_vel`) from
    /// a source at `pos` moving at `vel`: above 1 while they close in.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn pitch(&self, listener: &Listener, listener_vel: Vec3, pos: Vec3, vel: Vec3) -> f32 {
        let c = self.speed_of_sound;
        if !(self.scale > 0.0 && c > 0.0) {
            return 1.0;
        }
        let d = pos - listener.pos;
        let len2 = d.dot(d);
        if len2.is_nan() || len2 <= 1e-8 {
            return 1.0;
        }
        let n = d * (1.0 / crate::math3d::sqrt(len2));
        let limit = 0.9 * c;
        let speed = |v: f32| (v * self.scale).clamp(-limit, limit);
        // Toward the source is positive for the listener, away from the
        // listener for the source.
        let toward = speed(listener_vel.dot(n));
        let away = speed(vel.dot(n));
        clamp_pitch((c + toward) / (c + away))
    }
}

/// A handle into a [`VoicePool`]: stale once its sound ended, was stopped
/// or lost its slot to a higher priority.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PoolVoice {
    slot: u8,
    generation: u16,
}

/// What a [`VoicePool`] holds and did (budgets, the self-test, a debug HUD).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PoolStats {
    /// Slots with a backend voice now.
    pub audible: u8,
    /// Looping sounds holding a slot without a voice (out of range, or
    /// waiting for the backend).
    pub virtual_voices: u8,
    /// Sounds that lost their slot or their voice to a higher rank.
    pub stolen: u32,
    /// Sounds refused: the pool was full of higher ranks.
    pub refused: u32,
    /// One-shots never started: out of range from the start.
    pub inaudible: u32,
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u16,
    live: bool,
    emitter: SoundEmitter,
    voice: Option<VoiceId>,
    /// The volume it was last heard at (0 for a virtual voice).
    gain: f32,
}

impl Slot {
    const EMPTY: Slot = Slot {
        generation: 0,
        live: false,
        emitter: SoundEmitter::new(SoundId(0), Vec3::ZERO, 0.0),
        voice: None,
        gain: 0.0,
    };

    /// Lower ranks lose their voice first: priority, then loudness.
    fn rank(&self) -> (u8, f32) {
        (self.emitter.priority, self.gain)
    }
}

/// `a` outranks `b`.
fn above(a: (u8, f32), b: (u8, f32)) -> bool {
    a.0 > b.0 || (a.0 == b.0 && a.1 > b.1)
}

/// Up to `N` positional sounds; see the [module docs](self). Call
/// [`VoicePool::update`] once a frame, after moving the emitters.
pub struct VoicePool<const N: usize> {
    slots: [Slot; N],
    doppler: Doppler,
    /// Where the listener is and how it moves; heard only once
    /// `has_listener` (plain fields: no float `Option` on the N64).
    listener: Listener,
    listener_vel: Vec3,
    has_listener: bool,
    stats: PoolStats,
}

impl<const N: usize> Default for VoicePool<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> VoicePool<N> {
    /// An empty pool with physical doppler. `N` ≤ 255.
    pub const fn new() -> Self {
        VoicePool {
            slots: [Slot::EMPTY; N],
            doppler: Doppler::PHYSICAL,
            listener: Listener {
                pos: Vec3::ZERO,
                right: Vec3::new(1.0, 0.0, 0.0),
            },
            listener_vel: Vec3::ZERO,
            has_listener: false,
            stats: PoolStats {
                audible: 0,
                virtual_voices: 0,
                stolen: 0,
                refused: 0,
                inaudible: 0,
            },
        }
    }

    pub const fn with_doppler(mut self, doppler: Doppler) -> Self {
        self.doppler = doppler;
        self
    }

    pub fn set_doppler(&mut self, doppler: Doppler) {
        self.doppler = doppler;
    }

    /// Where the listener is and how it moves (the camera, the player's
    /// kart): what [`VoicePool::play_voice_at`] and
    /// [`VoicePool::update`] hear from. Set it every frame.
    pub fn set_listener(&mut self, listener: Listener, vel: Vec3) {
        self.listener = listener;
        self.listener_vel = vel;
        self.has_listener = true;
    }

    /// The volume and pitch `e` is heard at now.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn params(&self, e: &SoundEmitter) -> SoundParams {
        if !self.has_listener {
            return SoundParams {
                volume: 0.0,
                ..SoundParams::DEFAULT
            };
        }
        let l = &self.listener;
        let mut p = spatialize(l, e.pos, e.radius, e.volume);
        p.bus = e.bus;
        p.pitch = clamp_pitch(e.pitch * self.doppler.pitch(l, self.listener_vel, e.pos, e.vel));
        p
    }

    /// Start `e` where the listener hears it. `None` when it is refused (the
    /// pool is full of higher ranks) or a one-shot out of range. A looping
    /// sound out of range takes a slot without a voice and starts when
    /// heard.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn play_voice_at(&mut self, audio: &mut dyn Audio, e: &SoundEmitter) -> Option<PoolVoice> {
        let params = self.params(e);
        if params.volume <= 0.0 && !e.looped {
            self.stats.inaudible += 1;
            return None;
        }
        let rank = (e.priority, params.volume);
        let slot = match self.slots.iter().position(|s| !s.live) {
            Some(i) => i,
            None => {
                let i = self.lowest(false)?;
                if !above(rank, self.slots[i].rank()) {
                    self.stats.refused += 1;
                    return None;
                }
                self.release(audio, i);
                self.stats.stolen += 1;
                i
            }
        };
        let s = &mut self.slots[slot];
        s.generation = s.generation.wrapping_add(1);
        s.live = true;
        s.emitter = *e;
        s.voice = None;
        s.gain = params.volume;
        let handle = PoolVoice {
            slot: slot as u8,
            generation: s.generation,
        };
        if params.volume > 0.0 {
            self.start(audio, slot, &params);
            if self.slots[slot].voice.is_none() && !e.looped {
                // The backend had no voice and nothing ranked below it.
                self.slots[slot].live = false;
                self.stats.refused += 1;
                self.count();
                return None;
            }
        }
        self.count();
        Some(handle)
    }

    /// Give slot `i` a backend voice; when the backend has none left, take
    /// one from a lower-ranked slot (a loop goes virtual, a one-shot ends).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn start(&mut self, audio: &mut dyn Audio, i: usize, params: &SoundParams) {
        let e = self.slots[i].emitter;
        let mut voice = audio.play_voice(e.sound, params, e.looped);
        if voice.is_none() {
            let rank = (e.priority, params.volume);
            let victim = self.lowest(true);
            if let Some(v) = victim.filter(|&v| v != i && above(rank, self.slots[v].rank())) {
                let old = &mut self.slots[v];
                if let Some(id) = old.voice.take() {
                    audio.stop_voice(id);
                }
                old.gain = 0.0;
                if !old.emitter.looped {
                    old.live = false;
                }
                self.stats.stolen += 1;
                voice = audio.play_voice(e.sound, params, e.looped);
            }
        }
        self.slots[i].voice = voice;
    }

    /// The live slot of lowest rank (with a backend voice when `voiced`).
    fn lowest(&self, voiced: bool) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, s) in self.slots.iter().enumerate() {
            if !s.live || (voiced && s.voice.is_none()) {
                continue;
            }
            match best {
                Some(b) if !above(self.slots[b].rank(), s.rank()) => {}
                _ => best = Some(i),
            }
        }
        best
    }

    fn release(&mut self, audio: &mut dyn Audio, i: usize) {
        let s = &mut self.slots[i];
        if let Some(v) = s.voice.take() {
            audio.stop_voice(v);
        }
        s.live = false;
        s.gain = 0.0;
    }

    fn slot_of(&self, v: PoolVoice) -> Option<usize> {
        let i = v.slot as usize;
        let s = self.slots.get(i)?;
        (s.live && s.generation == v.generation).then_some(i)
    }

    /// The emitter behind `v`, to move it, change its pitch or volume; the
    /// next [`VoicePool::update`] applies it. `None` once `v` is stale.
    pub fn emitter_mut(&mut self, v: PoolVoice) -> Option<&mut SoundEmitter> {
        let i = self.slot_of(v)?;
        Some(&mut self.slots[i].emitter)
    }

    /// Move `v` (false once it is stale).
    pub fn move_voice(&mut self, v: PoolVoice, pos: Vec3, vel: Vec3) -> bool {
        match self.emitter_mut(v) {
            Some(e) => {
                e.pos = pos;
                e.vel = vel;
                true
            }
            None => false,
        }
    }

    /// The sound still holds its slot (heard or virtual).
    pub fn is_playing(&self, v: PoolVoice) -> bool {
        self.slot_of(v).is_some()
    }

    /// The sound has a backend voice now.
    pub fn is_audible(&self, v: PoolVoice) -> bool {
        self.slot_of(v)
            .is_some_and(|i| self.slots[i].voice.is_some())
    }

    pub fn stop(&mut self, audio: &mut dyn Audio, v: PoolVoice) {
        if let Some(i) = self.slot_of(v) {
            self.release(audio, i);
            self.count();
        }
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn stop_all(&mut self, audio: &mut dyn Audio) {
        for i in 0..N {
            if self.slots[i].live {
                self.release(audio, i);
            }
        }
        self.count();
    }

    /// Once a frame: steer every voice (volume, pan, doppler pitch), end the
    /// one-shots that finished or left the range, park loops out of range
    /// and wake the ones heard again.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn update(&mut self, audio: &mut dyn Audio) {
        for i in 0..N {
            if !self.slots[i].live {
                continue;
            }
            let e = self.slots[i].emitter;
            let params = self.params(&e);
            if let Some(v) = self.slots[i].voice
                && !audio.voice_playing(v)
            {
                // A one-shot ended (or the backend dropped the voice).
                self.slots[i].voice = None;
                if !e.looped {
                    self.slots[i].live = false;
                    continue;
                }
            }
            self.slots[i].gain = params.volume;
            match (self.slots[i].voice, params.volume > 0.0) {
                (Some(v), true) => audio.set_voice(v, &params),
                (Some(_), false) => {
                    if e.looped {
                        if let Some(v) = self.slots[i].voice.take() {
                            audio.stop_voice(v);
                        }
                    } else {
                        self.release(audio, i);
                    }
                }
                (None, true) => self.start(audio, i, &params),
                (None, false) => {}
            }
        }
        self.count();
    }

    fn count(&mut self) {
        let (mut audible, mut parked) = (0u8, 0u8);
        for s in self.slots.iter().filter(|s| s.live) {
            if s.voice.is_some() {
                audible += 1;
            } else {
                parked += 1;
            }
        }
        self.stats.audible = audible;
        self.stats.virtual_voices = parked;
    }

    pub fn stats(&self) -> PoolStats {
        self.stats
    }
}

// The pitch range the doppler shift stays within is the backends'.
const _: () = assert!(PITCH_MIN < 1.0 && PITCH_MAX > 1.0);
