//! The audio contract. Backends: cpal (PC), libdragon mixer (N64), ndsp
//! (3DS).
//!
//! Beyond fire-and-forget sounds and one music track, games get:
//!
//! - **Volume and pan** per playback ([`SoundParams`]); [`spatialize`]
//!   turns a world position into them for a [`Listener`] (the camera):
//!   distance attenuation + stereo pan — positional audio that costs a
//!   dot product, on every target.
//! - **Voices**: playbacks you keep a handle to and steer every frame
//!   (ambient loops, engines). Scene sound sources use them through
//!   [`Ambience`].
//! - **Stingers**: a short musical phrase over the music (coin fanfare,
//!   discovery); the music ducks while it plays and comes back by itself.
//!
//! Every method beyond the original four has a default, so backends and
//! test doubles that only implement the basics keep working (silently).

use crate::math::Vec3;
use crate::render3d::Camera3;

/// Handle to a baked sound effect (`.wav64` on N64, PCM elsewhere).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SoundId(pub u32);

impl SoundId {
    /// Handle from a logical asset path, e.g. `SoundId::from_path("sounds/beep")`.
    pub const fn from_path(logical_path: &str) -> Self {
        SoundId(crate::asset::asset_id(logical_path))
    }
}

/// Handle to a baked music track (`.xm64` on N64, streamed elsewhere).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MusicId(pub u32);

impl MusicId {
    pub const fn from_path(logical_path: &str) -> Self {
        MusicId(crate::asset::asset_id(logical_path))
    }
}

/// Volume and stereo position of one playback.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SoundParams {
    /// 0 = silent, 1 = as baked.
    pub volume: f32,
    /// -1 = left, 0 = center, 1 = right.
    pub pan: f32,
}

impl SoundParams {
    pub const DEFAULT: SoundParams = SoundParams {
        volume: 1.0,
        pan: 0.0,
    };

    /// Left/right gains (constant-sum pan, clamped): what backends feed
    /// their per-channel volumes.
    pub fn gains(&self) -> (f32, f32) {
        let v = self.volume.max(0.0);
        let p = self.pan.clamp(-1.0, 1.0);
        (v * (1.0 - p).min(1.0), v * (1.0 + p).min(1.0))
    }
}

impl Default for SoundParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A playing sound that can be steered or stopped (see [`Audio::play_voice`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct VoiceId(pub u32);

pub trait Audio {
    /// Fire-and-forget playback of a sound effect.
    fn play_sound(&mut self, sound: SoundId);

    /// Start a music track, replacing the current one.
    fn play_music(&mut self, music: MusicId, looped: bool);

    fn stop_music(&mut self);

    /// Master volume, 0.0..=1.0.
    fn set_master_volume(&mut self, volume: f32);

    /// Fire-and-forget with volume and pan (positional one-shots).
    fn play_sound_with(&mut self, sound: SoundId, params: &SoundParams) {
        let _ = params;
        self.play_sound(sound);
    }

    /// Start a sound you can steer; `looped` repeats it (on the N64 the
    /// loop lives in the baked file: declare `loop = true` on the sound in
    /// the manifest). `None` when no voice is free or the sound is unknown.
    fn play_voice(
        &mut self,
        sound: SoundId,
        params: &SoundParams,
        looped: bool,
    ) -> Option<VoiceId> {
        let _ = (sound, params, looped);
        None
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        let _ = (voice, params);
    }

    fn stop_voice(&mut self, voice: VoiceId) {
        let _ = voice;
    }

    /// Is the voice still playing (false once a one-shot ended or after
    /// `stop_voice`)?
    fn voice_playing(&self, voice: VoiceId) -> bool {
        let _ = voice;
        false
    }

    /// Play `music` once over the current track; the track ducks while the
    /// stinger plays and restores itself after.
    fn play_stinger(&mut self, music: MusicId) {
        let _ = music;
    }

    fn stinger_playing(&self) -> bool {
        false
    }

    /// Music volume, 0.0..=1.0 (stingers duck relative to it).
    fn set_music_volume(&mut self, volume: f32) {
        let _ = volume;
    }
}

/// Who hears positional sounds: usually the camera.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Listener {
    pub pos: Vec3,
    /// Unit vector pointing to the listener's right.
    pub right: Vec3,
}

impl Listener {
    /// The camera's eye, its right as seen on screen.
    pub fn from_camera(camera: &Camera3) -> Self {
        let f = (camera.target - camera.eye).normalized();
        // Same basis as Mat34::look_at: right = up x forward.
        let right = Vec3::new(0.0, 1.0, 0.0).cross(f).normalized();
        Listener {
            pos: camera.eye,
            right,
        }
    }
}

/// Volume and pan of a sound at `source` heard by `listener`: full volume
/// up close, fading (quadratically, like real falloff but reaching 0) to
/// silence at `radius`; pan from the source's side (kept within ±0.85 so
/// nothing is fully one-eared).
pub fn spatialize(listener: &Listener, source: Vec3, radius: f32, volume: f32) -> SoundParams {
    let d = source - listener.pos;
    let dist_sq = d.dot(d);
    let r = radius.max(1e-3);
    if dist_sq >= r * r {
        return SoundParams {
            volume: 0.0,
            pan: 0.0,
        };
    }
    let dist = crate::math3d::sqrt(dist_sq);
    let t = 1.0 - dist / r;
    let pan = if dist > 1e-3 {
        (d.dot(listener.right) / dist).clamp(-1.0, 1.0) * 0.85
    } else {
        0.0
    };
    SoundParams {
        volume: volume * t * t,
        pan,
    }
}

/// A positional sound source (scene `sound` components): a looping sound
/// heard within `radius` world units.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SoundSource {
    pub sound: SoundId,
    pub radius: f32,
    pub volume: f32,
}

/// Keeps up to `N` looping voices for the sound sources nearest the
/// listener — start when one comes in range, steer while audible, stop
/// when it leaves. No allocation; call [`Ambience::update`] once a frame.
pub struct Ambience<const N: usize> {
    /// (source key, voice) per active slot.
    slots: [Option<(u32, VoiceId)>; N],
}

impl<const N: usize> Default for Ambience<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Ambience<N> {
    pub const fn new() -> Self {
        Ambience { slots: [None; N] }
    }

    /// Voices currently playing.
    pub fn active(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// `sources`: (stable key — e.g. the node index — world position,
    /// source). Sources out of range are stopped; in-range ones get a voice
    /// (while slots last) with spatialized params.
    pub fn update(
        &mut self,
        audio: &mut dyn Audio,
        listener: &Listener,
        sources: impl IntoIterator<Item = (u32, Vec3, SoundSource)>,
    ) {
        let mut seen = [false; N];
        for (key, pos, src) in sources {
            let params = spatialize(listener, pos, src.radius, src.volume);
            let slot = self
                .slots
                .iter()
                .position(|s| s.is_some_and(|(k, _)| k == key));
            if params.volume <= 0.0 {
                if let Some((_, v)) = slot.and_then(|i| self.slots[i].take()) {
                    audio.stop_voice(v);
                }
                continue;
            }
            match slot {
                Some(i) => {
                    let (_, v) = self.slots[i].expect("occupied");
                    audio.set_voice(v, &params);
                    seen[i] = true;
                }
                None => {
                    let Some(i) = self.slots.iter().position(Option::is_none) else {
                        continue; // all voices busy: the farthest stay silent
                    };
                    if let Some(v) = audio.play_voice(src.sound, &params, true) {
                        self.slots[i] = Some((key, v));
                        seen[i] = true;
                    }
                }
            }
        }
        // Sources that vanished (despawned) stop too.
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if !seen[i]
                && let Some((_, v)) = slot.take()
            {
                audio.stop_voice(v);
            }
        }
    }

    /// Stop everything (scene change).
    pub fn stop_all(&mut self, audio: &mut dyn Audio) {
        for slot in &mut self.slots {
            if let Some((_, v)) = slot.take() {
                audio.stop_voice(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spatial_falloff_and_pan() {
        let cam = Camera3 {
            eye: Vec3::ZERO,
            target: Vec3::new(0.0, 0.0, 1.0),
            fov_y: 1.0,
        };
        let l = Listener::from_camera(&cam);
        assert!((l.right.x - 1.0).abs() < 1e-5, "looking +Z, right is +X");
        let near = spatialize(&l, Vec3::new(0.0, 0.0, 1.0), 10.0, 1.0);
        let far = spatialize(&l, Vec3::new(0.0, 0.0, 8.0), 10.0, 1.0);
        assert!(near.volume > far.volume && far.volume > 0.0);
        assert_eq!(
            spatialize(&l, Vec3::new(0.0, 0.0, 12.0), 10.0, 1.0).volume,
            0.0
        );
        let right = spatialize(&l, Vec3::new(3.0, 0.0, 0.0), 10.0, 1.0);
        assert!(right.pan > 0.8);
        let (gl, gr) = right.gains();
        assert!(gr > gl);
        assert_eq!(SoundParams::DEFAULT.gains(), (1.0, 1.0));
    }

    /// Records voice traffic.
    #[derive(Default)]
    struct Recorder {
        next: u32,
        playing: std::vec::Vec<(u32, f32)>,
        stopped: u32,
    }

    impl Audio for Recorder {
        fn play_sound(&mut self, _: SoundId) {}
        fn play_music(&mut self, _: MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
        fn play_voice(&mut self, _: SoundId, p: &SoundParams, _: bool) -> Option<VoiceId> {
            self.next += 1;
            self.playing.push((self.next, p.volume));
            Some(VoiceId(self.next))
        }
        fn set_voice(&mut self, v: VoiceId, p: &SoundParams) {
            for e in &mut self.playing {
                if e.0 == v.0 {
                    e.1 = p.volume;
                }
            }
        }
        fn stop_voice(&mut self, v: VoiceId) {
            self.playing.retain(|e| e.0 != v.0);
            self.stopped += 1;
        }
    }

    #[test]
    fn ambience_starts_steers_and_stops_voices() {
        let mut a = Recorder::default();
        let mut amb: Ambience<2> = Ambience::new();
        let src = SoundSource {
            sound: SoundId(1),
            radius: 10.0,
            volume: 1.0,
        };
        let mut l = Listener {
            pos: Vec3::ZERO,
            right: Vec3::new(1.0, 0.0, 0.0),
        };
        let sources = |x: f32| {
            [
                (0u32, Vec3::new(x, 0.0, 0.0), src),
                (1u32, Vec3::new(-x, 0.0, 0.0), src),
                (2u32, Vec3::new(0.0, 0.0, x), src),
            ]
        };
        amb.update(&mut a, &l, sources(2.0));
        assert_eq!(amb.active(), 2, "capacity bounds the voices");
        let v0 = a.playing[0].1;
        l.pos = Vec3::new(1.0, 0.0, 0.0);
        amb.update(&mut a, &l, sources(2.0));
        assert!(a.playing[0].1 > v0, "closer -> louder");
        l.pos = Vec3::new(100.0, 0.0, 0.0);
        amb.update(&mut a, &l, sources(2.0));
        assert_eq!(amb.active(), 0, "out of range -> stopped");
        assert_eq!(a.stopped, 2);
    }
}
