//! Clip metadata: timeline events and root motion tracks.

use super::{rd_f32, rd_u32};
use crate::audio::SoundId;
use crate::math::Vec3;
use crate::math3d::{cos, sin};
use crate::render::ClipId;

/// Bytes of one event record: `f32 time, u32 id, u32 sound (0 = none)`.
pub const EVENT_BYTES: usize = 12;
/// Bytes of one root-motion frame: `f32 x, f32 z, f32 yaw`.
pub const ROOT_BYTES: usize = 12;
/// Events one update can report ([`EventBuf`]); more are counted in
/// [`EventBuf::dropped`].
pub const MAX_EVENTS: usize = 8;

/// An event name's identity (`EventId::from_name("kick_contact")`), the same
/// FNV-1a as clip and asset handles. Names stay in the text files; the blobs
/// carry the hash.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct EventId(pub u32);

impl EventId {
    pub const fn from_name(name: &str) -> Self {
        EventId(crate::asset::asset_id(name))
    }
}

/// An event a clip's timeline crossed during an update.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AnimEvent {
    pub id: EventId,
    /// An event named `sfx:<name>` carries the sound `sounds/<name>`: the
    /// game plays it (on its bus, with its caption) without a table.
    pub sound: Option<SoundId>,
    /// The clip whose timeline it is on.
    pub clip: ClipId,
    /// Its time on that clip (seconds).
    pub time: f32,
    /// The clip's weight in the pose when it fired (a crossfade's fading
    /// clip still fires, at a low weight: filter on it if that matters).
    pub weight: f32,
}

impl AnimEvent {
    const NONE: AnimEvent = AnimEvent {
        id: EventId(0),
        sound: None,
        clip: ClipId(0),
        time: 0.0,
        weight: 0.0,
    };
}

/// The events of one update, in timeline order. Fixed capacity: no
/// allocation on any target.
#[derive(Clone, Copy, Debug)]
pub struct EventBuf {
    events: [AnimEvent; MAX_EVENTS],
    len: usize,
    /// Events past [`MAX_EVENTS`] in this update (not reported).
    pub dropped: u32,
}

impl Default for EventBuf {
    fn default() -> Self {
        EventBuf::new()
    }
}

impl EventBuf {
    pub const fn new() -> Self {
        EventBuf {
            events: [AnimEvent::NONE; MAX_EVENTS],
            len: 0,
            dropped: 0,
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
    }

    pub fn push(&mut self, e: AnimEvent) {
        if self.len < MAX_EVENTS {
            self.events[self.len] = e;
            self.len += 1;
        } else {
            self.dropped += 1;
        }
    }

    pub fn as_slice(&self) -> &[AnimEvent] {
        &self.events[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether an event with this id fired.
    pub fn contains(&self, id: EventId) -> bool {
        self.as_slice().iter().any(|e| e.id == id)
    }

    pub fn iter(&self) -> core::slice::Iter<'_, AnimEvent> {
        self.as_slice().iter()
    }
}

/// A clip's events, sorted by time (a view over `u32 count` + records).
#[derive(Clone, Copy, Debug, Default)]
pub struct EventTrack<'a> {
    records: &'a [u8],
}

impl<'a> EventTrack<'a> {
    pub const EMPTY: EventTrack<'static> = EventTrack { records: &[] };

    /// `bytes` = `u32 count` then the records. `None` when truncated.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn parse(bytes: &'a [u8]) -> Option<(EventTrack<'a>, usize)> {
        if bytes.len() < 4 {
            return None;
        }
        let n = rd_u32(bytes, 0) as usize;
        let len = 4 + n.checked_mul(EVENT_BYTES)?;
        if bytes.len() < len {
            return None;
        }
        let records = &bytes[4..len];
        // Times are finite (a corrupt blob can't make `fire` spin).
        if (0..n).any(|i| !rd_f32(records, i * EVENT_BYTES).is_finite()) {
            return None;
        }
        Some((EventTrack { records }, len))
    }

    /// A track [`parse`](Self::parse) validated before (a graph's, at
    /// `AnimGraph::parse`): only the count is read.
    pub(crate) fn view(bytes: &'a [u8]) -> EventTrack<'a> {
        let n = rd_u32(bytes, 0) as usize;
        EventTrack {
            records: bytes.get(4..4 + n * EVENT_BYTES).unwrap_or(&[]),
        }
    }

    pub fn len(&self) -> usize {
        self.records.len() / EVENT_BYTES
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Event `i`: (time, id, sound).
    pub fn get(&self, i: usize) -> (f32, EventId, Option<SoundId>) {
        let at = i * EVENT_BYTES;
        let sound = rd_u32(self.records, at + 8);
        (
            rd_f32(self.records, at),
            EventId(rd_u32(self.records, at + 4)),
            (sound != 0).then_some(SoundId(sound)),
        )
    }

    /// Push every event the clip's playhead crossed going from `from` to
    /// `to` seconds (`to >= from`, unwrapped: a looping clip's time keeps
    /// growing past its duration). An event fires when `from < t <= to`;
    /// `first` (the update that starts the clip) also fires one at exactly
    /// `from`, so an event at 0 fires when the clip starts and on every
    /// wrap, never twice. A one-shot clip stops at its duration.
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn fire(
        &self,
        from: f32,
        to: f32,
        first: bool,
        duration: f32,
        looping: bool,
        clip: ClipId,
        weight: f32,
        out: &mut EventBuf,
    ) {
        if self.is_empty() || to < from {
            return;
        }
        let mut emit = |t: f32, local: f32, i: usize| {
            if (from < t || (first && t == from)) && t <= to {
                let (_, id, sound) = self.get(i);
                out.push(AnimEvent {
                    id,
                    sound,
                    clip,
                    time: local,
                    weight,
                });
            }
        };
        if looping && duration > 0.0 {
            // At most a few cycles per update (a long hitch).
            let c0 = floor(from / duration);
            let c1 = floor(to / duration).min(c0 + 4.0);
            // Counted in integers: past 2^24 cycles `c + 1.0 == c`, and a
            // tiny duration from damaged data looped forever (issue #68).
            // A NaN span (an infinite cycle count) visits one cycle.
            let cycles = if c1 >= c0 { (c1 - c0) as u32 } else { 0 };
            for k in 0..=cycles.min(4) {
                let base = (c0 + k as f32) * duration;
                for i in 0..self.len() {
                    let local = self.get(i).0;
                    emit(base + local, local, i);
                }
            }
        } else {
            let (from_c, to_c) = (from.clamp(0.0, duration), to.clamp(0.0, duration));
            if to_c == from_c && !first {
                return;
            }
            for i in 0..self.len() {
                let local = self.get(i).0;
                if (from_c < local || (first && local == from_c)) && local <= to_c {
                    let (_, id, sound) = self.get(i);
                    out.push(AnimEvent {
                        id,
                        sound,
                        clip,
                        time: local,
                        weight,
                    });
                }
            }
        }
    }
}

/// `floor` without `std` (finite inputs).
pub(crate) fn floor(x: f32) -> f32 {
    let t = crate::math3d::to_i32(x) as f32;
    if t > x { t - 1.0 } else { t }
}

/// `(sin, cos)` of a yaw, exact for no turn (the core's trig is an
/// approximation: a straight walk must not lose distance to it). Out of
/// line: inlined at every use it was most of the root-motion code.
#[inline(never)]
fn sin_cos(yaw: f32) -> (f32, f32) {
    if yaw == 0.0 {
        (0.0, 1.0)
    } else {
        (sin(yaw), cos(yaw))
    }
}

/// The root's ground pose at one time: where it stood and how far it had
/// turned, relative to the clip's first frame (model space, Y up).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct RootPose {
    pub x: f32,
    pub z: f32,
    /// Radians about +Y, the engine's yaw (`Mat34::from_rotation_scale_translation`).
    pub yaw: f32,
}

impl RootPose {
    pub const IDENTITY: RootPose = RootPose {
        x: 0.0,
        z: 0.0,
        yaw: 0.0,
    };

    /// `self` then `b` (b expressed in self's frame).
    pub fn then(self, b: RootPose) -> RootPose {
        let (s, c) = sin_cos(self.yaw);
        RootPose {
            x: self.x + b.x * c + b.z * s,
            z: self.z - b.x * s + b.z * c,
            yaw: self.yaw + b.yaw,
        }
    }

    pub fn inverse(self) -> RootPose {
        let (s, c) = sin_cos(-self.yaw);
        RootPose {
            x: -(self.x * c + self.z * s),
            z: -(-self.x * s + self.z * c),
            yaw: -self.yaw,
        }
    }
}

/// How far the root moved during an update, in the character's own frame at
/// the start of the update (x right, z forward, model units) and how far it
/// turned. The game (or a character controller, #66) moves the entity by it.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct RootDelta {
    pub translation: Vec3,
    pub yaw: f32,
}

impl RootDelta {
    pub const ZERO: RootDelta = RootDelta {
        translation: Vec3::ZERO,
        yaw: 0.0,
    };

    fn from_pose(p: RootPose) -> RootDelta {
        RootDelta {
            translation: Vec3::new(p.x, 0.0, p.z),
            yaw: p.yaw,
        }
    }

    fn pose(self) -> RootPose {
        RootPose {
            x: self.translation.x,
            z: self.translation.z,
            yaw: self.yaw,
        }
    }

    /// `self` then `b`.
    pub fn then(self, b: RootDelta) -> RootDelta {
        RootDelta::from_pose(self.pose().then(b.pose()))
    }

    /// `self` scaled by `k` (a crossfade's weight): translation and turn.
    pub fn scaled(self, k: f32) -> RootDelta {
        RootDelta {
            translation: self.translation * k,
            yaw: self.yaw * k,
        }
    }

    /// Move an entity at `position` facing `yaw` (drawn at `scale` model
    /// units per world unit): the new position and yaw.
    pub fn apply(self, position: Vec3, yaw: f32, scale: f32) -> (Vec3, f32) {
        let (s, c) = sin_cos(yaw);
        let t = self.translation * scale;
        (
            Vec3::new(
                position.x + t.x * c + t.z * s,
                position.y + t.y,
                position.z - t.x * s + t.z * c,
            ),
            yaw + self.yaw,
        )
    }
}

/// A clip's root-motion track: the root's ground pose per baked frame,
/// cumulative from frame 0 (a view over `f32 fps, u32 frames` + records).
#[derive(Clone, Copy, Debug)]
pub struct RootTrack<'a> {
    fps: f32,
    frames: usize,
    records: &'a [u8],
}

impl<'a> RootTrack<'a> {
    /// `bytes` = `f32 fps, u32 frames` then the records. `None` when
    /// truncated or empty.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn parse(bytes: &'a [u8]) -> Option<(RootTrack<'a>, usize)> {
        if bytes.len() < 8 {
            return None;
        }
        let fps = rd_f32(bytes, 0);
        let frames = rd_u32(bytes, 4) as usize;
        let len = 8 + frames.checked_mul(ROOT_BYTES)?;
        if frames == 0 || bytes.len() < len || !fps.is_finite() || fps < 0.0 {
            return None;
        }
        let records = &bytes[8..len];
        if (0..frames * 3).any(|i| !rd_f32(records, i * 4).is_finite()) {
            return None;
        }
        Some((
            RootTrack {
                fps,
                frames,
                records: &bytes[8..len],
            },
            len,
        ))
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// A track [`parse`](Self::parse) validated before: the header only.
    pub(crate) fn view(bytes: &'a [u8]) -> Option<RootTrack<'a>> {
        let frames = rd_u32(bytes, 4) as usize;
        let records = bytes.get(8..8 + frames * ROOT_BYTES)?;
        (frames > 0).then_some(RootTrack {
            fps: rd_f32(bytes, 0),
            frames,
            records,
        })
    }

    fn frame(&self, f: usize) -> RootPose {
        let at = f * ROOT_BYTES;
        RootPose {
            x: rd_f32(self.records, at),
            z: rd_f32(self.records, at + 4),
            yaw: rd_f32(self.records, at + 8),
        }
    }

    /// The root's pose at `time` seconds into one cycle (clamped to the
    /// clip), interpolated like the joints are.
    pub fn at(&self, time: f32) -> RootPose {
        let last = (self.frames - 1) as f32;
        let f = if self.fps > 0.0 { time * self.fps } else { 0.0 };
        let f = f.clamp(0.0, last);
        let f0 = f as usize;
        let f1 = (f0 + 1).min(self.frames - 1);
        let k = f - f0 as f32;
        let (a, b) = (self.frame(f0), self.frame(f1));
        RootPose {
            x: a.x + (b.x - a.x) * k,
            z: a.z + (b.z - a.z) * k,
            yaw: a.yaw + (b.yaw - a.yaw) * k,
        }
    }

    /// Ground pose at an unwrapped time: whole cycles compose the cycle's
    /// end pose (a looping walk keeps walking).
    fn unwrapped(&self, t: f32, duration: f32, looping: bool) -> (f32, RootPose) {
        if !looping || duration <= 0.0 {
            return (0.0, self.at(t.clamp(0.0, duration.max(0.0))));
        }
        let c = floor(t / duration);
        (c, self.at(t - c * duration))
    }

    /// The root's move from `from` to `to` seconds (unwrapped times,
    /// `to >= from`), in the frame it had at `from`.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn delta(&self, from: f32, to: f32, duration: f32, looping: bool) -> RootDelta {
        if to <= from {
            return RootDelta::ZERO;
        }
        let (c0, p0) = self.unwrapped(from, duration, looping);
        let (c1, p1) = self.unwrapped(to, duration, looping);
        let mut d = p0.inverse();
        if c1 > c0 {
            // To the end of the first cycle, whole cycles, then into the last.
            let cycle = self.at(duration);
            d = d.then(cycle);
            // Whole cycles between, at most three (a long hitch), counted in
            // integers: near 2^24 `k + 1.0 == k` looped forever (issue #68).
            let whole = (c1 - c0 - 1.0).clamp(0.0, 3.0) as u32;
            for _ in 0..whole {
                d = d.then(cycle);
            }
        }
        RootDelta::from_pose(d.then(p1))
    }
}

/// A list of `u32 clip id` + record entries (the `ROOT` and `AEVT` layout,
/// also used inside a graph blob).
#[derive(Clone, Copy, Debug, Default)]
pub struct ClipTable<'a> {
    body: &'a [u8],
    count: usize,
    kind: TableKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum TableKind {
    #[default]
    Events,
    Roots,
}

impl<'a> ClipTable<'a> {
    fn entry_len(kind: TableKind, at: &'a [u8]) -> Option<usize> {
        match kind {
            TableKind::Events => EventTrack::parse(at).map(|(_, n)| n),
            TableKind::Roots => RootTrack::parse(at).map(|(_, n)| n),
        }
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn parse(body: &'a [u8], kind: TableKind) -> Option<ClipTable<'a>> {
        if body.len() < 4 {
            return None;
        }
        let count = rd_u32(body, 0) as usize;
        let mut at = 4;
        for _ in 0..count {
            let entry = body.get(at + 4..)?;
            at += 4 + Self::entry_len(kind, entry)?;
        }
        Some(ClipTable {
            body: &body[..at],
            count,
            kind,
        })
    }

    /// An `AEVT`-shaped table.
    pub fn events(body: &'a [u8]) -> Option<ClipTable<'a>> {
        Self::parse(body, TableKind::Events)
    }

    /// A `ROOT`-shaped table.
    pub fn roots(body: &'a [u8]) -> Option<ClipTable<'a>> {
        Self::parse(body, TableKind::Roots)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The bytes of entry `n` after its clip id, and the id.
    fn entry(&self, n: usize) -> Option<(ClipId, &'a [u8])> {
        let mut at = 4;
        for k in 0..self.count {
            let id = ClipId(rd_u32(self.body, at));
            let rest = &self.body[at + 4..];
            if k == n {
                return Some((id, rest));
            }
            at += 4 + Self::entry_len(self.kind, rest)?;
        }
        None
    }

    /// The clip ids, in order.
    pub fn clip(&self, n: usize) -> Option<ClipId> {
        self.entry(n).map(|(id, _)| id)
    }

    fn find(&self, clip: ClipId) -> Option<&'a [u8]> {
        (0..self.count)
            .filter_map(|n| self.entry(n))
            .find(|(id, _)| *id == clip)
            .map(|(_, b)| b)
    }

    /// The events of `clip` (empty when it has none, or on a roots table).
    pub fn events_of(&self, clip: ClipId) -> EventTrack<'a> {
        if self.kind != TableKind::Events {
            return EventTrack::default();
        }
        self.find(clip)
            .and_then(EventTrack::parse)
            .map_or_else(EventTrack::default, |(t, _)| t)
    }

    /// The root track of `clip` (`None` when the bake left its root in
    /// place, or on an events table).
    pub fn root_of(&self, clip: ClipId) -> Option<RootTrack<'a>> {
        if self.kind != TableKind::Roots {
            return None;
        }
        self.find(clip).and_then(RootTrack::parse).map(|(t, _)| t)
    }
}

/// A baked model's clip metadata (`ROOT` + `AEVT`), for tools: the editor's
/// Animation tab, `model inspect --anim` and the graph bake, which copies
/// it into the graph blob games read.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClipMeta<'a> {
    pub roots: ClipTable<'a>,
    pub events: ClipTable<'a>,
}

impl<'a> ClipMeta<'a> {
    /// `None` when a section is malformed; a model without them gives an
    /// empty table.
    pub fn from_tmdl(tmdl: &'a [u8]) -> Option<ClipMeta<'a>> {
        use crate::render3d::tmdl_section;
        let roots = match tmdl_section(tmdl, b"ROOT") {
            Some(b) => ClipTable::roots(b)?,
            None => ClipTable {
                kind: TableKind::Roots,
                ..ClipTable::default()
            },
        };
        let events = match tmdl_section(tmdl, b"AEVT") {
            Some(b) => ClipTable::events(b)?,
            None => ClipTable::default(),
        };
        Some(ClipMeta { roots, events })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(list: &[(f32, &str)]) -> Vec<u8> {
        let mut b = (list.len() as u32).to_le_bytes().to_vec();
        for (t, n) in list {
            b.extend_from_slice(&t.to_le_bytes());
            b.extend_from_slice(&EventId::from_name(n).0.to_le_bytes());
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        b
    }

    fn fired(track: &EventTrack, from: f32, to: f32, first: bool, looping: bool) -> Vec<f32> {
        let mut out = EventBuf::new();
        track.fire(from, to, first, 1.0, looping, ClipId(1), 1.0, &mut out);
        out.iter().map(|e| e.time).collect()
    }

    #[test]
    fn events_fire_once_per_crossing_and_on_every_wrap() {
        let bytes = events(&[(0.0, "start"), (0.5, "mid")]);
        let (t, used) = EventTrack::parse(&bytes).unwrap();
        assert_eq!(used, bytes.len());
        // The first update fires the event at 0, later ones don't repeat it.
        assert_eq!(fired(&t, 0.0, 0.1, true, true), [0.0]);
        assert_eq!(fired(&t, 0.0, 0.1, false, true), Vec::<f32>::new());
        // Exactly on the event: (from, to] takes it once.
        assert_eq!(fired(&t, 0.4, 0.5, false, true), [0.5]);
        assert_eq!(fired(&t, 0.5, 0.6, false, true), Vec::<f32>::new());
        // A wrap fires the event at 0 of the next cycle.
        assert_eq!(fired(&t, 0.9, 1.1, false, true), [0.0]);
        // A hitch longer than the clip: every crossing, in order.
        assert_eq!(fired(&t, 0.2, 2.6, false, true), [0.5, 0.0, 0.5, 0.0, 0.5]);
        // One-shot: stops at the end, an event at the end fires once.
        let end = events(&[(1.0, "end")]);
        let (e, _) = EventTrack::parse(&end).unwrap();
        assert_eq!(fired(&e, 0.9, 1.3, false, false), [1.0]);
        assert_eq!(fired(&e, 1.3, 1.6, false, false), Vec::<f32>::new());
    }

    #[test]
    fn the_buffer_counts_what_it_drops() {
        let many: Vec<(f32, &str)> = (0..10).map(|i| (i as f32 * 0.05, "e")).collect();
        let bytes = events(&many);
        let (t, _) = EventTrack::parse(&bytes).unwrap();
        let mut out = EventBuf::new();
        t.fire(0.0, 0.9, true, 1.0, true, ClipId(1), 1.0, &mut out);
        assert_eq!(out.len(), MAX_EVENTS);
        assert_eq!(out.dropped, 2);
    }

    fn track(frames: &[(f32, f32, f32)]) -> Vec<u8> {
        let mut b = 2.0f32.to_le_bytes().to_vec();
        b.extend_from_slice(&(frames.len() as u32).to_le_bytes());
        for (x, z, y) in frames {
            for v in [x, z, y] {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        b
    }

    #[test]
    fn root_deltas_add_up_over_wraps() {
        // 2 fps, 3 frames: 1 s, 0.5 m then 1.5 m forward.
        let bytes = track(&[(0.0, 0.0, 0.0), (0.0, 0.5, 0.0), (0.0, 2.0, 0.0)]);
        let (t, _) = RootTrack::parse(&bytes).unwrap();
        let d = |a: f32, b: f32| t.delta(a, b, 1.0, true).translation.z;
        assert!((d(0.0, 0.5) - 0.5).abs() < 1e-6);
        assert!((d(0.5, 1.0) - 1.5).abs() < 1e-6);
        // Split anywhere, the sum is the same: no step lost at the seam.
        let whole = d(0.25, 2.75);
        let parts = d(0.25, 0.9) + d(0.9, 1.6) + d(1.6, 2.75);
        assert!((whole - parts).abs() < 1e-5, "{whole} vs {parts}");
        assert!((d(0.0, 3.0) - 6.0).abs() < 1e-5);
        // One-shot: nothing past the end.
        assert_eq!(t.delta(1.0, 2.0, 1.0, false), RootDelta::ZERO);
    }

    #[test]
    fn a_turning_root_moves_in_its_own_frame() {
        // Turn a quarter while stepping 1 m forward.
        let q = core::f32::consts::FRAC_PI_2;
        let bytes = track(&[(0.0, 0.0, 0.0), (0.0, 0.5, q * 0.5), (0.0, 1.0, q)]);
        let (t, _) = RootTrack::parse(&bytes).unwrap();
        let d = t.delta(0.0, 1.0, 1.0, true);
        assert!((d.yaw - q).abs() < 1e-6);
        // Applied to an entity facing +X (yaw 90°): the step goes along +X.
        let (p, yaw) = d.apply(Vec3::ZERO, q, 1.0);
        assert!((p.x - 1.0).abs() < 1e-3 && p.z.abs() < 1e-3, "{p:?}");
        assert!((yaw - 2.0 * q).abs() < 1e-6);
        // Composition and inverse agree.
        let p = RootPose {
            x: 1.0,
            z: 2.0,
            yaw: 0.7,
        };
        let id = p.then(p.inverse());
        // (The core's trig is an approximation: a few mm over 2 m.)
        assert!(
            id.x.abs() < 1e-2 && id.z.abs() < 1e-2 && id.yaw.abs() < 1e-6,
            "{id:?}"
        );
    }

    #[test]
    fn clip_tables_find_their_clips() {
        let mut ev = 2u32.to_le_bytes().to_vec();
        for (clip, list) in [(7u32, &[(0.1f32, "a")][..]), (9, &[(0.2, "b"), (0.3, "c")])] {
            ev.extend_from_slice(&clip.to_le_bytes());
            ev.extend_from_slice(&events(list));
        }
        let t = ClipTable::events(&ev).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t.clip(1), Some(ClipId(9)));
        assert_eq!(t.events_of(ClipId(9)).len(), 2);
        assert!(t.events_of(ClipId(8)).is_empty());
        assert!(t.root_of(ClipId(9)).is_none());
        // Truncated: refused.
        assert!(ClipTable::events(&ev[..ev.len() - 1]).is_none());
        let mut roots = 1u32.to_le_bytes().to_vec();
        roots.extend_from_slice(&7u32.to_le_bytes());
        roots.extend_from_slice(&track(&[(0.0, 0.0, 0.0), (0.0, 1.0, 0.0)]));
        let r = ClipTable::roots(&roots).unwrap();
        assert_eq!(r.root_of(ClipId(7)).unwrap().frames(), 2);
    }

    #[test]
    fn floor_rounds_down() {
        assert_eq!(floor(1.5), 1.0);
        assert_eq!(floor(-0.5), -1.0);
        assert_eq!(floor(2.0), 2.0);
    }
}
