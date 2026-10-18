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
//! - **Buses** ([`Bus`]): every playback belongs to one (music, effects,
//!   voice, ambience, UI); each has a user volume and a mute, and
//!   [`DuckRule`]s lower one bus while another sounds (dialogue over music).
//!   [`BusMixer`] is the arithmetic every backend runs; [`MixerConfig`] is
//!   the baked `assets/audio.toml` (bus defaults and duck rules), and
//!   [`BusSettings`] packs a player's volumes for a console save.
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
    /// The bus it plays on; `None` = the asset's (`bus` in the manifest,
    /// else [`Bus::Sfx`] for sounds and [`Bus::Music`] for music). Read when
    /// the playback starts: `set_voice` doesn't move a voice.
    pub bus: Option<Bus>,
}

impl SoundParams {
    pub const DEFAULT: SoundParams = SoundParams {
        volume: 1.0,
        pan: 0.0,
        bus: None,
    };

    /// These params on `bus`.
    pub const fn on(self, bus: Bus) -> Self {
        SoundParams {
            bus: Some(bus),
            ..self
        }
    }

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

/// A mixer bus. Every playback belongs to one; each has a volume (the
/// player's setting), a mute and [`DuckRule`]s. A playback's gain is
/// master × its bus × the bus's ducking (× its own volume and pan).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[repr(u8)]
pub enum Bus {
    /// Scales every other bus; nothing plays on it directly.
    Master = 0,
    Music = 1,
    Sfx = 2,
    /// Dialogue, barks.
    Voice = 3,
    /// Positional loops (scene `sound` components).
    Ambience = 4,
    /// Menu clicks.
    Ui = 5,
}

impl Bus {
    pub const COUNT: usize = 6;
    pub const ALL: [Bus; Bus::COUNT] = [
        Bus::Master,
        Bus::Music,
        Bus::Sfx,
        Bus::Voice,
        Bus::Ambience,
        Bus::Ui,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_index(i: usize) -> Option<Bus> {
        if i < Bus::COUNT {
            Some(Bus::ALL[i])
        } else {
            None
        }
    }

    /// The name in `assets/audio.toml` and the manifest's `bus`.
    pub const fn label(self) -> &'static str {
        match self {
            Bus::Master => "master",
            Bus::Music => "music",
            Bus::Sfx => "sfx",
            Bus::Voice => "voice",
            Bus::Ambience => "ambience",
            Bus::Ui => "ui",
        }
    }

    pub fn from_label(label: &str) -> Option<Bus> {
        Bus::ALL.into_iter().find(|b| b.label() == label)
    }

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of buses (one bit per [`Bus::index`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct BusSet(pub u8);

impl BusSet {
    pub const EMPTY: BusSet = BusSet(0);

    pub const fn contains(self, bus: Bus) -> bool {
        self.0 & bus.bit() != 0
    }

    pub fn insert(&mut self, bus: Bus) {
        self.0 |= bus.bit();
    }

    pub fn set(&mut self, bus: Bus, on: bool) {
        if on {
            self.0 |= bus.bit();
        } else {
            self.0 &= !bus.bit();
        }
    }
}

/// While anything plays on `trigger`, `target` falls to `level` over
/// `attack_ms`, and comes back over `release_ms` once `trigger` is quiet
/// (linear ramps; 0 ms = at once). Two rules on one target: the deeper
/// duck wins.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DuckRule {
    pub trigger: Bus,
    pub target: Bus,
    /// Target gain while ducked, 0..1 (1 removes the rule).
    pub level: f32,
    pub attack_ms: u16,
    pub release_ms: u16,
}

impl DuckRule {
    /// The issue's example: voice ducks music to 0.4 over 150 ms, back in 300 ms.
    pub const fn new(
        trigger: Bus,
        target: Bus,
        level: f32,
        attack_ms: u16,
        release_ms: u16,
    ) -> Self {
        DuckRule {
            trigger,
            target,
            level,
            attack_ms,
            release_ms,
        }
    }
}

/// Most duck rules a mixer keeps.
pub const MAX_DUCK_RULES: usize = 8;

/// Bus volumes, mutes and ducking: the arithmetic of every backend's mixer
/// (PC per output buffer, N64/3DS per frame, Web per frame). No allocation,
/// deterministic.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BusMixer {
    volumes: [f32; Bus::COUNT],
    muted: BusSet,
    rules: [Option<DuckRule>; MAX_DUCK_RULES],
    /// Current gain of each rule's ramp (1 = not ducking).
    ramps: [f32; MAX_DUCK_RULES],
}

impl Default for BusMixer {
    fn default() -> Self {
        Self::new()
    }
}

impl BusMixer {
    pub const fn new() -> Self {
        BusMixer {
            volumes: [1.0; Bus::COUNT],
            muted: BusSet::EMPTY,
            rules: [None; MAX_DUCK_RULES],
            ramps: [1.0; MAX_DUCK_RULES],
        }
    }

    pub fn set_volume(&mut self, bus: Bus, volume: f32) {
        self.volumes[bus.index()] = volume.clamp(0.0, 1.0);
    }

    pub fn volume(&self, bus: Bus) -> f32 {
        self.volumes[bus.index()]
    }

    pub fn set_muted(&mut self, bus: Bus, muted: bool) {
        self.muted.set(bus, muted);
    }

    pub fn muted(&self, bus: Bus) -> bool {
        self.muted.contains(bus)
    }

    /// Add or replace the rule for `rule.trigger` → `rule.target`; a level
    /// of 1 or more removes it. False when the rule is invalid (a bus ducking
    /// itself, the master on either side) or the table is full.
    pub fn set_duck(&mut self, rule: DuckRule) -> bool {
        if rule.trigger == rule.target || rule.trigger == Bus::Master || rule.target == Bus::Master
        {
            return false;
        }
        let same = |r: &Option<DuckRule>| {
            r.is_some_and(|r| r.trigger == rule.trigger && r.target == rule.target)
        };
        let existing = self.rules.iter().position(same);
        if rule.level >= 1.0 {
            if let Some(i) = existing {
                self.rules[i] = None;
                self.ramps[i] = 1.0;
            }
            return true;
        }
        let rule = DuckRule {
            level: rule.level.max(0.0),
            ..rule
        };
        match existing.or_else(|| self.rules.iter().position(Option::is_none)) {
            Some(i) => {
                self.rules[i] = Some(rule);
                self.ramps[i] = self.ramps[i].max(rule.level);
                true
            }
            None => false,
        }
    }

    pub fn clear_ducks(&mut self) {
        self.rules = [None; MAX_DUCK_RULES];
        self.ramps = [1.0; MAX_DUCK_RULES];
    }

    /// The rules, in table order.
    pub fn ducks(&self) -> impl Iterator<Item = DuckRule> + '_ {
        self.rules.iter().flatten().copied()
    }

    /// Advance the duck ramps by `dt` seconds; `sounding` = the buses with
    /// something playing on them.
    pub fn update(&mut self, dt: f32, sounding: BusSet) {
        let ms = dt.max(0.0) * 1000.0;
        for (rule, ramp) in self.rules.iter().zip(self.ramps.iter_mut()) {
            let Some(rule) = rule else { continue };
            let depth = 1.0 - rule.level;
            let (goal, ms_total) = if sounding.contains(rule.trigger) {
                (rule.level, rule.attack_ms)
            } else {
                (1.0, rule.release_ms)
            };
            if ms_total == 0 || depth <= 0.0 {
                *ramp = goal;
                continue;
            }
            let step = depth * ms / ms_total as f32;
            *ramp = if *ramp > goal {
                (*ramp - step).max(goal)
            } else {
                (*ramp + step).min(goal)
            };
        }
    }

    /// Ducking of `bus` right now (1 = none).
    pub fn duck(&self, bus: Bus) -> f32 {
        self.rules
            .iter()
            .zip(self.ramps)
            .filter(|(r, _)| r.is_some_and(|r| r.target == bus))
            .fold(1.0, |g, (_, ramp)| g.min(ramp))
    }

    /// Gain of a playback on `bus`: master × bus volume × ducking, 0 when
    /// the bus or the master is muted. `gain(Master)` is the master alone.
    pub fn gain(&self, bus: Bus) -> f32 {
        let master = if self.muted(Bus::Master) {
            0.0
        } else {
            self.volume(Bus::Master)
        };
        if bus == Bus::Master {
            return master;
        }
        if self.muted(bus) {
            return 0.0;
        }
        master * self.volume(bus) * self.duck(bus)
    }

    /// [`BusMixer::gain`] of every bus, by index.
    pub fn gains(&self) -> [f32; Bus::COUNT] {
        Bus::ALL.map(|b| self.gain(b))
    }

    /// Volumes and mutes (what a player sets).
    pub fn settings(&self) -> BusSettings {
        BusSettings {
            volumes: self.volumes,
            muted: self.muted,
        }
    }

    pub fn apply_settings(&mut self, s: &BusSettings) {
        for b in Bus::ALL {
            self.set_volume(b, s.volumes[b.index()]);
        }
        self.muted = s.muted;
    }
}

/// A player's bus volumes and mutes. The PC runner keeps them in
/// `<save_name>.audio`; a console game that wants them in its save stores
/// [`BusSettings::to_bytes`] (4 bits per bus volume, 6 mute bits).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BusSettings {
    pub volumes: [f32; Bus::COUNT],
    pub muted: BusSet,
}

impl Default for BusSettings {
    fn default() -> Self {
        BusSettings {
            volumes: [1.0; Bus::COUNT],
            muted: BusSet::EMPTY,
        }
    }
}

impl BusSettings {
    /// What `audio` plays with now.
    pub fn read(audio: &dyn Audio) -> Self {
        let mut s = BusSettings::default();
        for b in Bus::ALL {
            s.volumes[b.index()] = audio.bus_volume(b);
            s.muted.set(b, audio.bus_muted(b));
        }
        s
    }

    pub fn apply(&self, audio: &mut dyn Audio) {
        for b in Bus::ALL {
            audio.set_bus_volume(b, self.volumes[b.index()]);
            audio.set_bus_muted(b, self.muted.contains(b));
        }
    }

    /// Bits 0..24: a 4-bit volume per bus (0..=15 of 1.0) by index; bits
    /// 24..30: the mutes.
    pub fn pack(&self) -> u32 {
        let mut v = 0u32;
        for (i, vol) in self.volumes.iter().enumerate() {
            let q = (vol.clamp(0.0, 1.0) * 15.0 + 0.5) as u32;
            v |= q.min(15) << (i * 4);
        }
        v | (self.muted.0 as u32 & 0x3F) << 24
    }

    pub fn unpack(v: u32) -> Self {
        let mut s = BusSettings::default();
        for (i, vol) in s.volumes.iter_mut().enumerate() {
            *vol = ((v >> (i * 4)) & 15) as f32 / 15.0;
        }
        s.muted = BusSet((v >> 24) as u8 & 0x3F);
        s
    }

    pub fn to_bytes(&self) -> [u8; 4] {
        self.pack().to_le_bytes()
    }

    pub fn from_bytes(b: [u8; 4]) -> Self {
        Self::unpack(u32::from_le_bytes(b))
    }
}

/// The project's mix, baked from `assets/audio.toml` (the `audio/mixer`
/// asset, `TMIX`): each bus's starting volume and the duck rules. Backends
/// apply it at load; a player's [`BusSettings`] go on top.
///
/// `TMIX` v1, little-endian: `"TMIX"`, u16 version, u8 bus count (6), u8
/// rule count (≤ [`MAX_DUCK_RULES`]), a u16 volume per bus in thousandths,
/// then per rule u8 trigger, u8 target, u16 level in thousandths, u16
/// attack ms, u16 release ms.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MixerConfig {
    pub volumes: [f32; Bus::COUNT],
    pub ducks: [Option<DuckRule>; MAX_DUCK_RULES],
}

impl Default for MixerConfig {
    fn default() -> Self {
        MixerConfig {
            volumes: [1.0; Bus::COUNT],
            ducks: [None; MAX_DUCK_RULES],
        }
    }
}

impl MixerConfig {
    pub const MAGIC: [u8; 4] = *b"TMIX";
    pub const VERSION: u16 = 1;
    /// The logical path of the baked asset.
    pub const LOGICAL: &'static str = "audio/mixer";

    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 8 || bytes[..4] != Self::MAGIC {
            return Err("TMIX: bad magic");
        }
        if u16::from_le_bytes([bytes[4], bytes[5]]) != Self::VERSION {
            return Err("TMIX: unsupported version");
        }
        let (buses, rules) = (bytes[6] as usize, bytes[7] as usize);
        if buses != Bus::COUNT || rules > MAX_DUCK_RULES {
            return Err("TMIX: bad counts");
        }
        if bytes.len() != 8 + buses * 2 + rules * 8 {
            return Err("TMIX: bad length");
        }
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let mut cfg = MixerConfig::default();
        for (i, v) in cfg.volumes.iter_mut().enumerate() {
            *v = (u16_at(8 + i * 2).min(1000)) as f32 / 1000.0;
        }
        let base = 8 + buses * 2;
        for (i, slot) in cfg.ducks.iter_mut().take(rules).enumerate() {
            let r = &bytes[base + i * 8..base + i * 8 + 8];
            let (Some(trigger), Some(target)) = (
                Bus::from_index(r[0] as usize),
                Bus::from_index(r[1] as usize),
            ) else {
                return Err("TMIX: bad bus");
            };
            *slot = Some(DuckRule {
                trigger,
                target,
                level: u16_at(base + i * 8 + 2).min(1000) as f32 / 1000.0,
                attack_ms: u16_at(base + i * 8 + 4),
                release_ms: u16_at(base + i * 8 + 6),
            });
        }
        Ok(cfg)
    }

    /// Bytes [`MixerConfig::encode`] writes.
    pub fn encoded_len(&self) -> usize {
        8 + Bus::COUNT * 2 + self.ducks.iter().flatten().count() * 8
    }

    /// Write the `TMIX` blob into `out` (at least [`MixerConfig::encoded_len`]
    /// bytes); returns the length.
    pub fn encode(&self, out: &mut [u8]) -> usize {
        let per_mille = |v: f32| ((v.clamp(0.0, 1.0) * 1000.0 + 0.5) as u16).to_le_bytes();
        out[..4].copy_from_slice(&Self::MAGIC);
        out[4..6].copy_from_slice(&Self::VERSION.to_le_bytes());
        out[6] = Bus::COUNT as u8;
        out[7] = self.ducks.iter().flatten().count() as u8;
        let mut at = 8;
        for v in self.volumes {
            out[at..at + 2].copy_from_slice(&per_mille(v));
            at += 2;
        }
        for r in self.ducks.iter().flatten() {
            out[at] = r.trigger as u8;
            out[at + 1] = r.target as u8;
            out[at + 2..at + 4].copy_from_slice(&per_mille(r.level));
            out[at + 4..at + 6].copy_from_slice(&r.attack_ms.to_le_bytes());
            out[at + 6..at + 8].copy_from_slice(&r.release_ms.to_le_bytes());
            at += 8;
        }
        at
    }

    /// Set every bus volume and replace the duck rules.
    pub fn apply(&self, audio: &mut dyn Audio) {
        for b in Bus::ALL {
            audio.set_bus_volume(b, self.volumes[b.index()]);
        }
        audio.clear_ducks();
        for r in self.ducks.iter().flatten() {
            audio.set_duck(*r);
        }
    }
}

/// Bytes before a [`SoundBlob`]'s payload.
pub const SOUND_HEADER_LEN: usize = 20;

/// Codec of a baked PC/Web sound ([`SoundBlob`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SoundCodec {
    /// 16-bit signed little-endian mono.
    Pcm16 = 0,
    /// An Ogg Vorbis file (mono).
    Vorbis = 1,
}

/// A baked sound or track for the PC and the Web (`TSN2`, issue #53): the
/// codec, the bus it plays on by default, loop and stream flags, and the
/// encoded payload. The consoles have their own containers (wav64 on the
/// N64, the 3DS shim's header).
///
/// Little-endian: `"TSN2"`, u8 codec, u8 bus, u8 flags (bit 0 loop, bit 1
/// stream), u8 channels (1), u32 sample rate, u32 frames, u32 payload
/// bytes, payload. `TSND` (v1: f32 mono, no flags) is still read by the
/// PC loader.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SoundBlob<'a> {
    pub codec: SoundCodec,
    pub bus: Bus,
    pub looped: bool,
    pub stream: bool,
    pub sample_rate: u32,
    pub frames: u32,
    pub payload: &'a [u8],
}

impl<'a> SoundBlob<'a> {
    pub const MAGIC: [u8; 4] = *b"TSN2";
    pub const HEADER_LEN: usize = SOUND_HEADER_LEN;

    pub fn parse(bytes: &'a [u8]) -> Result<Self, &'static str> {
        if bytes.len() < Self::HEADER_LEN || bytes[..4] != Self::MAGIC {
            return Err("TSN2: bad magic");
        }
        let codec = match bytes[4] {
            0 => SoundCodec::Pcm16,
            1 => SoundCodec::Vorbis,
            _ => return Err("TSN2: unknown codec"),
        };
        let bus = Bus::from_index(bytes[5] as usize).ok_or("TSN2: bad bus")?;
        if bytes[7] != 1 {
            return Err("TSN2: only mono");
        }
        let u32_at = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let payload = &bytes[Self::HEADER_LEN..];
        if payload.len() != u32_at(16) as usize {
            return Err("TSN2: bad payload length");
        }
        let frames = u32_at(12);
        if codec == SoundCodec::Pcm16 && payload.len() != frames as usize * 2 {
            return Err("TSN2: PCM length disagrees with the frame count");
        }
        Ok(SoundBlob {
            codec,
            bus,
            looped: bytes[6] & 1 != 0,
            stream: bytes[6] & 2 != 0,
            sample_rate: u32_at(8),
            frames,
            payload,
        })
    }

    /// The header for this blob (the payload follows it).
    pub fn header(&self) -> [u8; SOUND_HEADER_LEN] {
        let mut h = [0u8; SOUND_HEADER_LEN];
        h[..4].copy_from_slice(&Self::MAGIC);
        h[4] = self.codec as u8;
        h[5] = self.bus as u8;
        h[6] = self.looped as u8 | (self.stream as u8) << 1;
        h[7] = 1;
        h[8..12].copy_from_slice(&self.sample_rate.to_le_bytes());
        h[12..16].copy_from_slice(&self.frames.to_le_bytes());
        h[16..20].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        h
    }
}

/// What a backend's audio holds and does, for budgets and self-tests.
/// Counters since the backend started; zeros on backends that don't count.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AudioStats {
    /// Playbacks sounding now (one-shots, voices, music, stingers).
    pub voices: u16,
    /// Of those, the ones streaming from storage.
    pub streams: u16,
    /// Memory held by loaded sounds (decoded or compressed, as resident).
    pub resident_bytes: u32,
    /// Memory held by streaming buffers.
    pub stream_bytes: u32,
    /// Stream buffers the output found empty (a decoder fell behind).
    pub underruns: u32,
    /// Sample frames the streams have delivered (progress).
    pub streamed_frames: u32,
}

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

    /// Music volume, 0.0..=1.0 (stingers duck relative to it). The same as
    /// `set_bus_volume(Bus::Music, ..)` on a backend with buses.
    fn set_music_volume(&mut self, volume: f32) {
        let _ = volume;
    }

    /// A bus's volume, 0.0..=1.0. The default maps the master and the music
    /// onto the original two volumes and ignores the rest.
    fn set_bus_volume(&mut self, bus: Bus, volume: f32) {
        match bus {
            Bus::Master => self.set_master_volume(volume),
            Bus::Music => self.set_music_volume(volume),
            _ => {}
        }
    }

    fn bus_volume(&self, bus: Bus) -> f32 {
        let _ = bus;
        1.0
    }

    fn set_bus_muted(&mut self, bus: Bus, muted: bool) {
        let _ = (bus, muted);
    }

    fn bus_muted(&self, bus: Bus) -> bool {
        let _ = bus;
        false
    }

    /// Add or replace a duck rule (see [`DuckRule`]; level 1 removes it).
    fn set_duck(&mut self, rule: DuckRule) {
        let _ = rule;
    }

    fn clear_ducks(&mut self) {}

    /// Current ducking gain of `bus` (1 = none): what the duck rules apply
    /// right now, for tests and meters.
    fn bus_duck(&self, bus: Bus) -> f32 {
        let _ = bus;
        1.0
    }

    /// Memory, streams and underruns (see [`AudioStats`]).
    fn audio_stats(&self) -> AudioStats {
        AudioStats::default()
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
            bus: None,
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
        bus: None,
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
    fn bus_gain_is_master_times_bus_times_duck() {
        let mut m = BusMixer::new();
        assert_eq!(m.gains(), [1.0; Bus::COUNT]);
        m.set_volume(Bus::Master, 0.5);
        m.set_volume(Bus::Music, 0.8);
        assert!((m.gain(Bus::Music) - 0.4).abs() < 1e-6);
        assert_eq!(m.gain(Bus::Master), 0.5);
        assert_eq!(m.gain(Bus::Sfx), 0.5);
        m.set_muted(Bus::Music, true);
        assert_eq!(m.gain(Bus::Music), 0.0);
        assert_eq!(m.gain(Bus::Sfx), 0.5, "a mute is per bus");
        m.set_muted(Bus::Master, true);
        assert_eq!(m.gains(), [0.0; Bus::COUNT], "the master mutes all");
        m.set_volume(Bus::Ui, 7.0);
        assert_eq!(m.volume(Bus::Ui), 1.0, "volumes clamp");
    }

    #[test]
    fn ducking_ramps_down_and_back() {
        let mut m = BusMixer::new();
        assert!(m.set_duck(DuckRule::new(Bus::Voice, Bus::Music, 0.4, 150, 300)));
        let voice = {
            let mut s = BusSet::EMPTY;
            s.insert(Bus::Voice);
            s
        };
        // 75 ms in: halfway down (1 → 0.4 over 150 ms).
        m.update(0.075, voice);
        assert!(
            (m.duck(Bus::Music) - 0.7).abs() < 1e-5,
            "{}",
            m.duck(Bus::Music)
        );
        assert_eq!(m.duck(Bus::Sfx), 1.0, "only the target ducks");
        m.update(1.0, voice);
        assert!(
            (m.gain(Bus::Music) - 0.4).abs() < 1e-6,
            "holds at the level"
        );
        // Voice ends: back up over 300 ms.
        m.update(0.15, BusSet::EMPTY);
        assert!((m.duck(Bus::Music) - 0.7).abs() < 1e-5);
        m.update(0.2, BusSet::EMPTY);
        assert_eq!(m.duck(Bus::Music), 1.0);
        // Instant rules and the deeper of two ducks.
        assert!(m.set_duck(DuckRule::new(Bus::Ui, Bus::Music, 0.2, 0, 0)));
        let mut both = voice;
        both.insert(Bus::Ui);
        m.update(0.001, both);
        assert!((m.duck(Bus::Music) - 0.2).abs() < 1e-6, "the deeper wins");
        // Level 1 removes a rule; invalid rules are refused.
        assert!(m.set_duck(DuckRule::new(Bus::Ui, Bus::Music, 1.0, 0, 0)));
        assert_eq!(m.ducks().count(), 1);
        assert!(!m.set_duck(DuckRule::new(Bus::Music, Bus::Music, 0.5, 0, 0)));
        assert!(!m.set_duck(DuckRule::new(Bus::Master, Bus::Music, 0.5, 0, 0)));
        assert!(!m.set_duck(DuckRule::new(Bus::Voice, Bus::Master, 0.5, 0, 0)));
        m.clear_ducks();
        assert_eq!(m.duck(Bus::Music), 1.0);
    }

    #[test]
    fn duck_table_fills_and_replaces_in_place() {
        let mut m = BusMixer::new();
        let pairs = [
            (Bus::Voice, Bus::Music),
            (Bus::Voice, Bus::Sfx),
            (Bus::Voice, Bus::Ambience),
            (Bus::Voice, Bus::Ui),
            (Bus::Ui, Bus::Music),
            (Bus::Ui, Bus::Sfx),
            (Bus::Ui, Bus::Ambience),
            (Bus::Sfx, Bus::Ambience),
        ];
        for (t, g) in pairs {
            assert!(m.set_duck(DuckRule::new(t, g, 0.5, 10, 10)));
        }
        assert!(
            !m.set_duck(DuckRule::new(Bus::Sfx, Bus::Music, 0.5, 10, 10)),
            "full"
        );
        assert!(
            m.set_duck(DuckRule::new(Bus::Voice, Bus::Music, 0.3, 10, 10)),
            "replace"
        );
        assert_eq!(m.ducks().count(), MAX_DUCK_RULES);
        assert_eq!(m.ducks().next().unwrap().level, 0.3);
    }

    #[test]
    fn settings_pack_into_four_bytes() {
        let mut s = BusSettings {
            volumes: [1.0, 0.8, 0.0, 0.4, 1.0 / 15.0, 0.5],
            ..BusSettings::default()
        };
        s.muted.insert(Bus::Ambience);
        let back = BusSettings::from_bytes(s.to_bytes());
        for (a, b) in s.volumes.iter().zip(back.volumes) {
            assert!((a - b).abs() <= 0.5 / 15.0 + 1e-6, "{a} vs {b}");
        }
        assert_eq!(back.muted, s.muted);
        assert_eq!(
            BusSettings::unpack(BusSettings::default().pack()),
            BusSettings::default()
        );
        assert_eq!(BusSettings::default().pack(), 0x00FF_FFFF);
    }

    #[test]
    fn mixer_config_round_trips_and_rejects_damage() {
        let mut cfg = MixerConfig::default();
        cfg.volumes[Bus::Music.index()] = 0.8;
        cfg.ducks[0] = Some(DuckRule::new(Bus::Voice, Bus::Music, 0.4, 150, 300));
        cfg.ducks[1] = Some(DuckRule::new(Bus::Ui, Bus::Ambience, 0.25, 0, 50));
        let mut buf = [0u8; 64];
        let n = cfg.encode(&mut buf);
        assert_eq!(n, cfg.encoded_len());
        assert_eq!(n, 8 + 12 + 16);
        assert_eq!(MixerConfig::parse(&buf[..n]).unwrap(), cfg);
        assert!(MixerConfig::parse(&buf[..n - 1]).is_err(), "length");
        let mut bad = buf;
        bad[0] = b'X';
        assert!(MixerConfig::parse(&bad[..n]).is_err(), "magic");
        let mut bad = buf;
        bad[4] = 2;
        assert!(MixerConfig::parse(&bad[..n]).is_err(), "version");
        let mut bad = buf;
        bad[8 + 12] = 9;
        assert!(MixerConfig::parse(&bad[..n]).is_err(), "bus index");
    }

    #[test]
    fn sound_blobs_round_trip_and_reject_damage() {
        let payload = [1u8, 0, 2, 0, 3, 0];
        let blob = SoundBlob {
            codec: SoundCodec::Pcm16,
            bus: Bus::Ambience,
            looped: true,
            stream: false,
            sample_rate: 22050,
            frames: 3,
            payload: &payload,
        };
        let mut bytes = blob.header().to_vec();
        bytes.extend_from_slice(&payload);
        assert_eq!(SoundBlob::parse(&bytes).unwrap(), blob);
        let mut v = bytes.clone();
        v[12] = 4;
        assert!(SoundBlob::parse(&v).is_err(), "frames vs PCM length");
        let mut v = bytes.clone();
        v[4] = 7;
        assert!(SoundBlob::parse(&v).is_err(), "codec");
        assert!(
            SoundBlob::parse(&bytes[..bytes.len() - 1]).is_err(),
            "payload"
        );
        let ogg = SoundBlob {
            codec: SoundCodec::Vorbis,
            stream: true,
            looped: false,
            frames: 1000,
            ..blob
        };
        let mut bytes = ogg.header().to_vec();
        bytes.extend_from_slice(&payload);
        let back = SoundBlob::parse(&bytes).unwrap();
        assert!(back.stream && !back.looped && back.codec == SoundCodec::Vorbis);
    }

    #[test]
    fn bus_labels_round_trip() {
        for b in Bus::ALL {
            assert_eq!(Bus::from_label(b.label()), Some(b));
            assert_eq!(Bus::from_index(b.index()), Some(b));
        }
        assert_eq!(Bus::from_label("drums"), None);
        assert_eq!(Bus::from_index(Bus::COUNT), None);
        assert_eq!(SoundParams::DEFAULT.on(Bus::Ui).bus, Some(Bus::Ui));
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
