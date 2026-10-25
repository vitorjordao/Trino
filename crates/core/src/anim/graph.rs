//! Animation graphs: state machines and blend spaces as data (`TANG`
//! blobs baked from `assets/anim/<name>.anim.ron`).
//!
//! A graph names a model's clips, parameters the game sets (floats, bools,
//! triggers), states (one clip, or a 1D blend space over a parameter) and
//! transitions between them (conditions over the parameters and the state's
//! clock, a crossfade time). [`AnimGraphInstance`] plays one: every update
//! advances the clocks, fires the events the playhead crossed, adds up the
//! root motion and takes the first transition whose condition holds. Its
//! pose is an [`AnimRef`] (up to three clips), which every backend already
//! draws.
//!
//! ## `TANG` v1 (little-endian; offsets from the blob's start)
//!
//! ```text
//! header (32 B): "TANG" u32 version, u32 model id, u16 params, u16 clips,
//!                u16 states, u16 transitions, u16 start state, u16 0, u32 0 u32 0
//! params      (12 B each): u32 name id, u8 kind (0 float, 1 bool, 2 trigger), u8 0 *3, f32 default
//! clips       (24 B each): u32 clip id, f32 duration, u8 loops, u8 0 *3,
//!                          u32 events offset, u32 root track offset, u32 feet offset
//!                          (NONE = 0xFFFF_FFFF)
//! states      (32 B each): u32 name id, u8 kind (0 clip, 1 blend 1D), u8 flags
//!                          (bit 0 root motion, bit 1 loop override, bit 2 its value),
//!                          u16 points, f32 speed, u16 speed param, u16 blend param
//!                          (NONE = 0xFFFF), f32 wrap (0 = none), u32 points offset,
//!                          u32 events offset, u32 0
//! transitions (12 B each): u16 from (0xFFFF = any state), u16 to, f32 blend seconds,
//!                          u32 condition offset
//! data: points (8 B: f32 value, u16 clip, u16 0), event tracks
//!       ([`EventTrack`]: u32 n + records), root tracks ([`RootTrack`]),
//!       feet tracks ([`FeetTrack`]: f32 fps, u32 frames, frames × 2 × f32*3,
//!       the IK chains' ankles in model space, left then right),
//!       conditions (prefix code: u8 op + operands, see [`Cond`]).
//! ```
//!
//! Names are hashes ([`ParamId`]/[`StateId`] via `from_name`); the RON keeps
//! them for tools.

use super::clip::{EventBuf, EventTrack, RootDelta, RootTrack, floor};
use super::{rd_f32, rd_u8, rd_u16, rd_u32};
use crate::render::{AnimBlend, AnimRef, ClipId};

pub const TANG_MAGIC: &[u8; 4] = b"TANG";
pub const TANG_VERSION: u32 = 1;
/// Parameters one graph may declare (the instance keeps them inline).
pub const MAX_PARAMS: usize = 16;
/// Points of one blend space.
pub const MAX_POINTS: usize = 8;
/// States of one graph.
pub const MAX_STATES: usize = 64;
/// Deepest condition tree (`Not(And([Or(..)]))` is 3).
pub const MAX_COND_DEPTH: usize = 8;
/// No index / no offset.
pub const NONE16: u16 = 0xFFFF;
pub const NONE32: u32 = 0xFFFF_FFFF;

pub const HEADER_BYTES: usize = 32;
pub const PARAM_BYTES: usize = 12;
pub const CLIP_BYTES: usize = 24;
pub const STATE_BYTES: usize = 32;
pub const TRANSITION_BYTES: usize = 12;
pub const POINT_BYTES: usize = 8;

/// State flags.
pub const STATE_ROOT_MOTION: u8 = 1;
pub const STATE_LOOP_SET: u8 = 2;
pub const STATE_LOOP_ON: u8 = 4;

/// Condition opcodes (the prefix code at a transition's condition offset).
pub mod op {
    /// Always.
    pub const TRUE: u8 = 0;
    /// `u16 param`: the trigger is set (consumed when its transition fires).
    pub const TRIGGER: u8 = 1;
    /// The state's clip reached its end (one-shot) or looped once.
    pub const FINISHED: u8 = 2;
    /// `u16 param`: a bool is true / a float is not 0.
    pub const PARAM: u8 = 3;
    /// One child.
    pub const NOT: u8 = 4;
    /// `u8 n` children.
    pub const AND: u8 = 5;
    pub const OR: u8 = 6;
    /// `u16 param, f32 value`.
    pub const GREATER: u8 = 7;
    pub const LESS: u8 = 8;
    /// `f32 seconds` in the state.
    pub const AFTER: u8 = 9;
}

/// Why a graph blob was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GraphError {
    BadMagic,
    /// A version this build does not read.
    Version(u32),
    Truncated,
    /// An index or offset out of range, or a malformed condition.
    Corrupt,
    /// More than [`MAX_PARAMS`] / [`MAX_STATES`] / [`MAX_POINTS`].
    TooBig,
}

/// A parameter's identity (`ParamId::from_name("speed")`, FNV-1a).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ParamId(pub u32);

impl ParamId {
    pub const fn from_name(name: &str) -> Self {
        ParamId(crate::asset::asset_id(name))
    }
}

/// A state's identity (`StateId::from_name("locomotion")`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StateId(pub u32);

impl StateId {
    pub const fn from_name(name: &str) -> Self {
        StateId(crate::asset::asset_id(name))
    }
}

/// What a parameter holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamKind {
    Float,
    Bool,
    /// Set for one update: a transition on it consumes it, and an update
    /// clears what nothing consumed.
    Trigger,
}

/// A parsed (borrowed) graph blob.
#[derive(Clone, Copy, Debug)]
pub struct AnimGraph<'a> {
    bytes: &'a [u8],
    params: usize,
    clips: usize,
    states: usize,
    transitions: usize,
    start: usize,
}

/// One clip a graph plays, as the bake copied it from the model.
#[derive(Clone, Copy, Debug)]
pub struct GraphClip<'a> {
    pub id: ClipId,
    pub duration: f32,
    pub looping: bool,
    pub events: EventTrack<'a>,
    pub root: Option<RootTrack<'a>>,
    /// Where the model's IK feet are over the clip (when it has chains).
    pub feet: Option<FeetTrack<'a>>,
}

/// The ankles of a model's two IK legs over a clip, per baked frame (model
/// space): where a game looks for the ground under each foot.
#[derive(Clone, Copy, Debug)]
pub struct FeetTrack<'a> {
    fps: f32,
    frames: usize,
    records: &'a [u8],
}

impl<'a> FeetTrack<'a> {
    /// `f32 fps, u32 frames` then `frames × 24` bytes.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn parse(bytes: &'a [u8]) -> Option<FeetTrack<'a>> {
        if bytes.len() < 8 {
            return None;
        }
        let fps = rd_f32(bytes, 0);
        let frames = rd_u32(bytes, 4) as usize;
        let len = 8 + frames.checked_mul(24)?;
        if frames == 0 || bytes.len() < len || !fps.is_finite() || fps < 0.0 {
            return None;
        }
        let records = &bytes[8..len];
        if (0..frames * 6).any(|i| !rd_f32(records, i * 4).is_finite()) {
            return None;
        }
        Some(FeetTrack {
            fps,
            frames,
            records,
        })
    }

    /// A track [`parse`](Self::parse) validated before: the header only.
    fn view(bytes: &'a [u8]) -> Option<FeetTrack<'a>> {
        let frames = rd_u32(bytes, 4) as usize;
        let records = bytes.get(8..8 + frames * 24)?;
        (frames > 0).then_some(FeetTrack {
            fps: rd_f32(bytes, 0),
            frames,
            records,
        })
    }

    fn frame(&self, f: usize) -> [crate::math::Vec3; 2] {
        let v = |at: usize| {
            crate::math::Vec3::new(
                rd_f32(self.records, at),
                rd_f32(self.records, at + 4),
                rd_f32(self.records, at + 8),
            )
        };
        [v(f * 24), v(f * 24 + 12)]
    }

    /// Both ankles at `time` seconds into the clip (clamped).
    pub fn at(&self, time: f32) -> [crate::math::Vec3; 2] {
        let last = (self.frames - 1) as f32;
        let f = (time * self.fps).clamp(0.0, last);
        let f0 = f as usize;
        let f1 = (f0 + 1).min(self.frames - 1);
        let k = f - f0 as f32;
        let (a, b) = (self.frame(f0), self.frame(f1));
        [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k]
    }
}

/// One state of a graph.
#[derive(Clone, Copy, Debug)]
pub struct GraphState<'a> {
    pub id: StateId,
    /// A blend space over `param` (`Some`), or one clip.
    pub param: Option<usize>,
    pub points: usize,
    pub speed: f32,
    pub speed_param: Option<usize>,
    pub wrap: f32,
    pub root_motion: bool,
    /// The graph's own loop setting for a clip state (`None`: the clip's).
    pub looping: Option<bool>,
    points_at: usize,
    pub events: EventTrack<'a>,
}

impl<'a> AnimGraph<'a> {
    /// Parse and validate a `TANG` blob (every index and offset is checked
    /// once, so playback never panics on a bad blob).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn parse(bytes: &'a [u8]) -> Result<AnimGraph<'a>, GraphError> {
        if bytes.len() < HEADER_BYTES {
            return Err(if bytes.len() >= 4 && &bytes[..4] != TANG_MAGIC {
                GraphError::BadMagic
            } else {
                GraphError::Truncated
            });
        }
        if &bytes[..4] != TANG_MAGIC {
            return Err(GraphError::BadMagic);
        }
        let version = rd_u32(bytes, 4);
        if version != TANG_VERSION {
            return Err(GraphError::Version(version));
        }
        let g = AnimGraph {
            bytes,
            params: rd_u16(bytes, 12) as usize,
            clips: rd_u16(bytes, 14) as usize,
            states: rd_u16(bytes, 16) as usize,
            transitions: rd_u16(bytes, 18) as usize,
            start: rd_u16(bytes, 20) as usize,
        };
        if g.params > MAX_PARAMS || g.states > MAX_STATES {
            return Err(GraphError::TooBig);
        }
        if g.states == 0 || g.start >= g.states {
            return Err(GraphError::Corrupt);
        }
        if bytes.len() < g.transitions_at() + g.transitions * TRANSITION_BYTES {
            return Err(GraphError::Truncated);
        }
        let track = |off: u32| -> Result<(), GraphError> {
            if off != NONE32 {
                bytes
                    .get(off as usize..)
                    .and_then(EventTrack::parse)
                    .ok_or(GraphError::Corrupt)?;
            }
            Ok(())
        };
        // Every float the playback divides by or clamps with is finite and
        // in range, so a corrupt blob can't make it panic or spin.
        let sane = |v: f32| (0.0..1.0e6).contains(&v);
        for p in 0..g.params {
            if !rd_f32(bytes, g.params_at() + p * PARAM_BYTES + 8).is_finite() {
                return Err(GraphError::Corrupt);
            }
        }
        for c in 0..g.clips {
            let at = g.clips_at() + c * CLIP_BYTES;
            if !sane(rd_f32(bytes, at + 4)) {
                return Err(GraphError::Corrupt);
            }
            track(rd_u32(bytes, at + 12))?;
            let root = rd_u32(bytes, at + 16);
            if root != NONE32 {
                bytes
                    .get(root as usize..)
                    .and_then(RootTrack::parse)
                    .ok_or(GraphError::Corrupt)?;
            }
            let feet = rd_u32(bytes, at + 20);
            if feet != NONE32 {
                bytes
                    .get(feet as usize..)
                    .and_then(FeetTrack::parse)
                    .ok_or(GraphError::Corrupt)?;
            }
        }
        for s in 0..g.states {
            let at = g.states_at() + s * STATE_BYTES;
            let kind = rd_u8(bytes, at + 4);
            let points = rd_u16(bytes, at + 6) as usize;
            let speed_param = rd_u16(bytes, at + 12);
            let param = rd_u16(bytes, at + 14);
            let points_at = rd_u32(bytes, at + 20) as usize;
            if kind > 1 || points == 0 || points > MAX_POINTS || (kind == 0 && points != 1) {
                return Err(if points > MAX_POINTS {
                    GraphError::TooBig
                } else {
                    GraphError::Corrupt
                });
            }
            if !sane(rd_f32(bytes, at + 8)) || !sane(rd_f32(bytes, at + 16)) {
                return Err(GraphError::Corrupt);
            }
            if (kind == 1 && param as usize >= g.params)
                || (speed_param != NONE16 && speed_param as usize >= g.params)
                || bytes.len() < points_at + points * POINT_BYTES
            {
                return Err(GraphError::Corrupt);
            }
            let mut last = f32::MIN;
            for p in 0..points {
                let v = rd_f32(bytes, points_at + p * POINT_BYTES);
                let clip = rd_u16(bytes, points_at + p * POINT_BYTES + 4) as usize;
                if clip >= g.clips || !v.is_finite() || v < last {
                    return Err(GraphError::Corrupt);
                }
                last = v;
            }
            track(rd_u32(bytes, at + 24))?;
        }
        for t in 0..g.transitions {
            let at = g.transitions_at() + t * TRANSITION_BYTES;
            let (from, to) = (rd_u16(bytes, at), rd_u16(bytes, at + 2) as usize);
            if (from != NONE16 && from as usize >= g.states) || to >= g.states {
                return Err(GraphError::Corrupt);
            }
            if !sane(rd_f32(bytes, at + 4)) {
                return Err(GraphError::Corrupt);
            }
            let cond = rd_u32(bytes, at + 8) as usize;
            g.check_cond(cond, 0)?;
        }
        Ok(g)
    }

    fn params_at(&self) -> usize {
        HEADER_BYTES
    }
    fn clips_at(&self) -> usize {
        self.params_at() + self.params * PARAM_BYTES
    }
    fn states_at(&self) -> usize {
        self.clips_at() + self.clips * CLIP_BYTES
    }
    fn transitions_at(&self) -> usize {
        self.states_at() + self.states * STATE_BYTES
    }

    /// Length of the condition at `at`, validating it.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn check_cond(&self, at: usize, depth: usize) -> Result<usize, GraphError> {
        let b = self.bytes;
        if depth > MAX_COND_DEPTH || at >= b.len() {
            return Err(GraphError::Corrupt);
        }
        let need = |n: usize| {
            if b.len() < at + n {
                Err(GraphError::Corrupt)
            } else {
                Ok(n)
            }
        };
        let param = |n: usize| -> Result<(), GraphError> {
            need(n)?;
            if rd_u16(b, at + 1) as usize >= self.params {
                Err(GraphError::Corrupt)
            } else {
                Ok(())
            }
        };
        match rd_u8(b, at) {
            op::TRUE | op::FINISHED => Ok(1),
            op::TRIGGER | op::PARAM => param(3).map(|_| 3),
            op::GREATER | op::LESS => param(7).map(|_| 7),
            op::AFTER => need(5),
            op::NOT => Ok(1 + self.check_cond(at + 1, depth + 1)?),
            op::AND | op::OR => {
                need(2)?;
                let n = rd_u8(b, at + 1) as usize;
                let mut len = 2;
                for _ in 0..n {
                    len += self.check_cond(at + len, depth + 1)?;
                }
                Ok(len)
            }
            _ => Err(GraphError::Corrupt),
        }
    }

    /// The model the graph animates (`ModelId(id)`).
    pub fn model(&self) -> u32 {
        rd_u32(self.bytes, 8)
    }

    pub fn param_count(&self) -> usize {
        self.params
    }

    pub fn state_count(&self) -> usize {
        self.states
    }

    pub fn transition_count(&self) -> usize {
        self.transitions
    }

    pub fn clip_count(&self) -> usize {
        self.clips
    }

    /// The state an instance starts in.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Parameter `i`: (id, kind, default).
    pub fn param_info(&self, i: usize) -> (ParamId, ParamKind, f32) {
        let at = self.params_at() + i * PARAM_BYTES;
        let kind = match rd_u8(self.bytes, at + 4) {
            1 => ParamKind::Bool,
            2 => ParamKind::Trigger,
            _ => ParamKind::Float,
        };
        (
            ParamId(rd_u32(self.bytes, at)),
            kind,
            rd_f32(self.bytes, at + 8),
        )
    }

    /// The index of a parameter (cache it: lookups hash nothing but walk
    /// the table).
    pub fn param(&self, id: ParamId) -> Option<usize> {
        (0..self.params).find(|&i| self.param_info(i).0 == id)
    }

    /// The index of a parameter by name.
    pub fn param_named(&self, name: &str) -> Option<usize> {
        self.param(ParamId::from_name(name))
    }

    /// Clip `i` (`i < clip_count()`): views over tracks `parse` validated.
    #[inline(never)]
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn clip(&self, i: usize) -> GraphClip<'a> {
        let at = self.clips_at() + i * CLIP_BYTES;
        let b = self.bytes;
        let track = |off: u32| (off != NONE32).then(|| b.get(off as usize..)).flatten();
        GraphClip {
            id: ClipId(rd_u32(b, at)),
            duration: rd_f32(b, at + 4),
            looping: rd_u8(b, at + 8) != 0,
            events: track(rd_u32(b, at + 12)).map_or_else(EventTrack::default, EventTrack::view),
            root: track(rd_u32(b, at + 16)).and_then(RootTrack::view),
            feet: track(rd_u32(b, at + 20)).and_then(FeetTrack::view),
        }
    }

    /// State `i` (`i < state_count()`).
    #[inline(never)]
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn state(&self, i: usize) -> GraphState<'a> {
        let at = self.states_at() + i * STATE_BYTES;
        let b = self.bytes;
        let flags = rd_u8(b, at + 5);
        let speed_param = rd_u16(b, at + 12);
        let events = rd_u32(b, at + 24);
        GraphState {
            id: StateId(rd_u32(b, at)),
            param: (rd_u8(b, at + 4) == 1).then(|| rd_u16(b, at + 14) as usize),
            points: rd_u16(b, at + 6) as usize,
            speed: rd_f32(b, at + 8),
            speed_param: (speed_param != NONE16).then_some(speed_param as usize),
            wrap: rd_f32(b, at + 16),
            root_motion: flags & STATE_ROOT_MOTION != 0,
            looping: (flags & STATE_LOOP_SET != 0).then_some(flags & STATE_LOOP_ON != 0),
            points_at: rd_u32(b, at + 20) as usize,
            events: if events == NONE32 {
                EventTrack::default()
            } else {
                b.get(events as usize..)
                    .map_or_else(EventTrack::default, EventTrack::view)
            },
        }
    }

    /// The index of a state.
    pub fn state_index(&self, id: StateId) -> Option<usize> {
        (0..self.states).find(|&i| self.state(i).id == id)
    }

    /// Point `p` of state `s`: (value, clip index).
    pub fn point(&self, s: &GraphState, p: usize) -> (f32, usize) {
        let at = s.points_at + p * POINT_BYTES;
        (rd_f32(self.bytes, at), rd_u16(self.bytes, at + 4) as usize)
    }

    /// Transition `t`: (from state or `None` = any, to, blend seconds,
    /// condition offset).
    pub fn transition(&self, t: usize) -> (Option<usize>, usize, f32, usize) {
        let at = self.transitions_at() + t * TRANSITION_BYTES;
        let from = rd_u16(self.bytes, at);
        (
            (from != NONE16).then_some(from as usize),
            rd_u16(self.bytes, at + 2) as usize,
            rd_f32(self.bytes, at + 4),
            rd_u32(self.bytes, at + 8) as usize,
        )
    }

    /// Whether the condition at `at` holds, and the length of its code.
    /// `used` collects the triggers it read (to consume them).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn eval(&self, at: usize, ctx: &EvalCtx, used: &mut u16) -> (bool, usize) {
        let b = self.bytes;
        match rd_u8(b, at) {
            op::TRUE => (true, 1),
            op::FINISHED => (ctx.finished, 1),
            op::TRIGGER => {
                let p = rd_u16(b, at + 1) as usize;
                let on = ctx.params[p] != 0.0;
                if on {
                    *used |= 1 << p;
                }
                (on, 3)
            }
            op::PARAM => (ctx.params[rd_u16(b, at + 1) as usize] != 0.0, 3),
            op::GREATER | op::LESS => {
                let v = ctx.params[rd_u16(b, at + 1) as usize];
                let k = rd_f32(b, at + 3);
                (
                    if rd_u8(b, at) == op::GREATER {
                        v > k
                    } else {
                        v < k
                    },
                    7,
                )
            }
            op::AFTER => (ctx.elapsed >= rd_f32(b, at + 1), 5),
            op::NOT => {
                // A trigger under a Not is not consumed by it.
                let mut ignore = 0;
                let (v, n) = self.eval(at + 1, ctx, &mut ignore);
                (!v, 1 + n)
            }
            op::AND | op::OR => {
                let n = rd_u8(b, at + 1) as usize;
                let and = rd_u8(b, at) == op::AND;
                let mut len = 2;
                let mut acc = and;
                let mut mine = 0u16;
                for _ in 0..n {
                    let (v, l) = self.eval(at + len, ctx, &mut mine);
                    len += l;
                    acc = if and { acc && v } else { acc || v };
                }
                if acc {
                    *used |= mine;
                }
                (acc, len)
            }
            _ => (false, 1),
        }
    }
}

struct EvalCtx<'p> {
    params: &'p [f32; MAX_PARAMS],
    finished: bool,
    elapsed: f32,
}

/// One state being played: its index, its clock (a clip state's seconds,
/// unwrapped; a blend space's cycles of its effective duration, whole
/// cycles dropped) and how long it has been on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Playing {
    state: u16,
    phase: f32,
    elapsed: f32,
    /// Completed cycles (saturating), for `Finished` on a loop.
    cycles: u16,
    /// The next update starts the state (events at time 0 fire).
    first: bool,
}

impl Playing {
    fn new(state: usize) -> Playing {
        Playing {
            state: state as u16,
            phase: 0.0,
            elapsed: 0.0,
            cycles: 0,
            first: true,
        }
    }
}

/// A clip's share of the pose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contribution {
    pub clip: usize,
    /// Its time in seconds (what [`AnimRef`] samples).
    pub time: f32,
    pub weight: f32,
}

/// A graph being played: parameters, the current state (and the one it is
/// fading from), this update's events and root motion. `Copy`, fixed size,
/// no allocation; the same updates give the same poses on every target.
#[derive(Clone, Copy, Debug)]
pub struct AnimGraphInstance {
    params: [f32; MAX_PARAMS],
    cur: Playing,
    prev: Option<Playing>,
    fade: f32,
    fade_len: f32,
    events: EventBuf,
    root: RootDelta,
    /// Transitions taken since the instance started (a test reads it).
    pub transitions: u32,
}

impl AnimGraphInstance {
    /// An instance at the graph's start state, its parameters at their
    /// defaults.
    pub fn new(graph: &AnimGraph) -> AnimGraphInstance {
        let mut params = [0.0; MAX_PARAMS];
        for (i, p) in params.iter_mut().enumerate().take(graph.param_count()) {
            let (_, kind, default) = graph.param_info(i);
            *p = if kind == ParamKind::Trigger {
                0.0
            } else {
                default
            };
        }
        AnimGraphInstance {
            params,
            cur: Playing::new(graph.start()),
            prev: None,
            fade: 0.0,
            fade_len: 0.0,
            events: EventBuf::new(),
            root: RootDelta::ZERO,
            transitions: 0,
        }
    }

    /// An instance starting in state `state` (an index, or the start when
    /// out of range): several entities sharing one graph, each mid-action.
    pub fn new_in(graph: &AnimGraph, state: usize) -> AnimGraphInstance {
        let mut i = AnimGraphInstance::new(graph);
        if state < graph.state_count() {
            i.cur = Playing::new(state);
        }
        i
    }

    /// Set a float parameter (by index, [`AnimGraph::param`]).
    pub fn set_float(&mut self, param: usize, v: f32) {
        if let Some(p) = self.params.get_mut(param) {
            *p = v;
        }
    }

    pub fn set_bool(&mut self, param: usize, v: bool) {
        self.set_float(param, if v { 1.0 } else { 0.0 });
    }

    /// Fire a trigger for the next update.
    pub fn set_trigger(&mut self, param: usize) {
        self.set_float(param, 1.0);
    }

    /// Set a parameter by name (`inst.set(&graph, "speed", 2.0)`; bools and
    /// triggers take 0 / 1). `false` when the graph has no such parameter.
    pub fn set(&mut self, graph: &AnimGraph, name: &str, v: f32) -> bool {
        match graph.param_named(name) {
            Some(i) => {
                self.set_float(i, v);
                true
            }
            None => false,
        }
    }

    pub fn param(&self, param: usize) -> f32 {
        self.params.get(param).copied().unwrap_or(0.0)
    }

    /// The current state's index (the one being faded to, mid-transition).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn state(&self) -> usize {
        self.cur.state as usize
    }

    /// Mid-transition: (from state, 0..1 progress).
    pub fn transition(&self) -> Option<(usize, f32)> {
        self.prev.map(|p| (p.state as usize, self.fade_k()))
    }

    /// Seconds in the current state.
    pub fn elapsed(&self) -> f32 {
        self.cur.elapsed
    }

    /// The current state's clock: a clip state's seconds, a blend space's
    /// cycle (0..1).
    pub fn clock(&self) -> f32 {
        self.cur.phase
    }

    /// The events the last update crossed, in timeline order.
    pub fn events(&self) -> &EventBuf {
        &self.events
    }

    /// The root motion of the last update (model units, in the
    /// character's frame at its start): move the entity by it.
    pub fn root_motion(&self) -> RootDelta {
        self.root
    }

    /// Force a state (a respawn, a cutscene), with an optional crossfade.
    pub fn play(&mut self, state: usize, blend: f32) {
        self.start_transition(state, blend);
    }

    fn fade_k(&self) -> f32 {
        if self.fade_len > 0.0 {
            (self.fade / self.fade_len).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    fn start_transition(&mut self, to: usize, blend: f32) {
        self.prev = (blend > 0.0).then_some(self.cur);
        self.cur = Playing::new(to);
        self.fade = 0.0;
        self.fade_len = blend;
        self.transitions = self.transitions.wrapping_add(1);
    }

    /// A blend space's two clips and the weight of the second.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn blend_pair(graph: &AnimGraph, s: &GraphState, v: f32) -> (usize, usize, f32) {
        let first = graph.point(s, 0);
        if s.points == 1 {
            return (first.1, first.1, 0.0);
        }
        let last = graph.point(s, s.points - 1);
        let mut v = v;
        if s.wrap > 0.0 {
            // Circular (an angle): between the last point and the first
            // across the seam.
            v -= floor((v - first.0) / s.wrap) * s.wrap;
            if v > last.0 {
                let span = first.0 + s.wrap - last.0;
                let w = if span > 0.0 { (v - last.0) / span } else { 0.0 };
                return (last.1, first.1, w);
            }
        }
        if v <= first.0 {
            return (first.1, first.1, 0.0);
        }
        for p in 0..s.points - 1 {
            let (a, b) = (graph.point(s, p), graph.point(s, p + 1));
            if v <= b.0 {
                let span = b.0 - a.0;
                let w = if span > 0.0 { (v - a.0) / span } else { 1.0 };
                return (a.1, b.1, w);
            }
        }
        (last.1, last.1, 0.0)
    }

    /// The two clips of a state now, the second's weight, and its cycle
    /// length in seconds.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn shape(&self, graph: &AnimGraph, s: &GraphState) -> (usize, usize, f32, f32) {
        let v = s.param.map_or(0.0, |p| self.params[p]);
        let (a, b, w) = Self::blend_pair(graph, s, v);
        let (da, db) = (graph.clip(a).duration, graph.clip(b).duration);
        (a, b, w, da + (db - da) * w)
    }

    fn looping(graph: &AnimGraph, s: &GraphState, clip: usize) -> bool {
        s.looping.unwrap_or(graph.clip(clip).looping)
    }

    /// A clip's time in seconds at a state's clock.
    fn clip_time(graph: &AnimGraph, s: &GraphState, clip: usize, clock: f32) -> f32 {
        let c = graph.clip(clip);
        if s.param.is_none() {
            // The seconds as played: the pose wraps a looping clip itself.
            match s.looping {
                Some(true) if !c.looping && c.duration > 0.0 => {
                    clock - floor(clock / c.duration) * c.duration
                }
                Some(false) if c.looping => clock.min(c.duration),
                _ => clock,
            }
        } else if Self::looping(graph, s, clip) {
            (clock - floor(clock)) * c.duration
        } else {
            clock.clamp(0.0, 1.0) * c.duration
        }
    }

    /// Advance one playing state by `dt`; fire its events into
    /// `self.events` when `fire`, and return its root motion.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn advance(
        &mut self,
        graph: &AnimGraph,
        p: &mut Playing,
        dt: f32,
        fire: bool,
        weight: f32,
    ) -> RootDelta {
        let s = graph.state(p.state as usize);
        let (a, b, w, dur) = self.shape(graph, &s);
        let rate = s.speed * s.speed_param.map_or(1.0, |i| self.params[i]);
        let looping = Self::looping(graph, &s, a);
        p.elapsed += dt;
        let step = if dt * rate > 0.0 { dt * rate } else { 0.0 };
        // The clock: a clip state's seconds (unwrapped, like a game's own
        // `time` it replaces: the same additions give the same pose), a
        // blend space's cycles.
        let c0 = p.phase;
        let (c1, unit) = if s.param.is_none() {
            // Not clamped even for a one-shot clip: the pose clamps it, and
            // the time stays the game's own (events and root motion clamp).
            (c0 + step, dur)
        } else {
            let mut ph = if dur > 0.0 { c0 + step / dur } else { 1.0 };
            if !looping {
                ph = ph.min(1.0);
            }
            (ph, 1.0)
        };
        let mut root = RootDelta::ZERO;
        // Each clip of the shape: its share of root motion; the heavier one
        // fires its events (a walk and a run blended don't both step).
        let (heavy, heavy_w) = if w > 0.5 { (b, w) } else { (a, 1.0 - w) };
        let pair = [(a, 1.0 - w), (b, w)];
        let clips = if a == b { &pair[..1] } else { &pair[..] };
        for &(clip, k) in clips {
            let k = if a == b { 1.0 } else { k };
            if k <= 0.0 {
                continue;
            }
            let c = graph.clip(clip);
            let lp = Self::looping(graph, &s, clip);
            let (t0, t1) = if s.param.is_none() {
                (c0, c1)
            } else {
                (c0 * c.duration, c1 * c.duration)
            };
            if s.root_motion
                && let Some(track) = c.root
            {
                root = root.then(track.delta(t0, t1, c.duration, lp).scaled(k));
            }
            if fire && clip == heavy {
                let ew = weight * if a == b { 1.0 } else { heavy_w };
                c.events
                    .fire(t0, t1, p.first, c.duration, lp, c.id, ew, &mut self.events);
                if s.param.is_none() {
                    s.events
                        .fire(t0, t1, p.first, c.duration, lp, c.id, ew, &mut self.events);
                }
            }
        }
        // Cycles done (`Finished`).
        if unit <= 0.0 {
            p.cycles = p.cycles.max(1);
        } else if looping {
            let done = floor(c1 / unit) - floor(c0 / unit);
            p.cycles = p.cycles.saturating_add(done as u16);
        } else if c1 >= unit && c0 < unit {
            p.cycles = p.cycles.saturating_add(1);
        }
        // A blend space's clock keeps only its fraction (its clips' times
        // come from it); a clip state's stays the seconds it played.
        p.phase = if s.param.is_some() && looping {
            c1 - floor(c1)
        } else {
            c1
        };
        p.first = false;
        root
    }

    /// One step: advance the clocks by `dt` seconds, collect events and
    /// root motion, then take the first transition whose condition holds
    /// (in the graph's order; transitions from any state skip the state
    /// they lead to). Triggers nothing consumed are cleared.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn update(&mut self, graph: &AnimGraph, dt: f32) {
        self.events.clear();
        let dt = if dt > 0.0 { dt } else { 0.0 };
        let k = if self.prev.is_some() {
            self.fade += dt;
            self.fade_k()
        } else {
            1.0
        };
        let mut cur = self.cur;
        let mut root = self.advance(graph, &mut cur, dt, true, k);
        self.cur = cur;
        if let Some(mut prev) = self.prev {
            let from = self.advance(graph, &mut prev, dt, false, 1.0 - k);
            root = root.scaled(k).then(from.scaled(1.0 - k));
            self.prev = if k >= 1.0 { None } else { Some(prev) };
        }
        self.root = root;

        // Transitions, evaluated after the clocks moved.
        let ctx_finished = self.cur.cycles > 0;
        let mut fired = None;
        {
            let ctx = EvalCtx {
                params: &self.params,
                finished: ctx_finished,
                elapsed: self.cur.elapsed,
            };
            for t in 0..graph.transition_count() {
                let (from, to, blend, cond) = graph.transition(t);
                match from {
                    Some(f) if f != self.cur.state as usize => continue,
                    None if to == self.cur.state as usize => continue,
                    _ => {}
                }
                let mut used = 0u16;
                if graph.eval(cond, &ctx, &mut used).0 {
                    fired = Some((to, blend, used));
                    break;
                }
            }
        }
        if let Some((to, blend, used)) = fired {
            for (i, p) in self.params.iter_mut().enumerate() {
                if used & (1 << i) != 0 {
                    *p = 0.0;
                }
            }
            self.start_transition(to, blend);
        }
        // A trigger lives one update.
        for i in 0..graph.param_count() {
            if graph.param_info(i).1 == ParamKind::Trigger {
                self.params[i] = 0.0;
            }
        }
    }

    /// Each clip's share of the pose, at most four (two states of two
    /// clips), fading-from state first; weights sum to 1.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn contributions(&self, graph: &AnimGraph, out: &mut [Contribution; 4]) -> usize {
        let mut n = 0;
        let k = if self.prev.is_some() {
            self.fade_k()
        } else {
            1.0
        };
        let mut push = |p: &Playing, scale: f32| {
            let s = graph.state(p.state as usize);
            let (a, b, w, _) = self.shape(graph, &s);
            for (clip, cw) in [(a, 1.0 - w), (b, w)] {
                let weight = cw * scale;
                if weight > 0.0 && n < 4 {
                    out[n] = Contribution {
                        clip,
                        time: Self::clip_time(graph, &s, clip, p.phase),
                        weight,
                    };
                    n += 1;
                }
                if a == b {
                    break;
                }
            }
        };
        if let Some(prev) = &self.prev {
            push(prev, 1.0 - k);
        }
        push(&self.cur, k);
        n
    }

    /// Where the model's IK feet are in this pose (model space; the
    /// clips' feet tracks blended by their weights): where a game casts
    /// for the ground ([`crate::anim::FootIk::pose`]). `None` for a model
    /// without IK chains.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn feet(&self, graph: &AnimGraph) -> Option<[crate::math::Vec3; 2]> {
        let mut c = [Contribution {
            clip: 0,
            time: 0.0,
            weight: 0.0,
        }; 4];
        let n = self.contributions(graph, &mut c);
        let mut out = [crate::math::Vec3::ZERO; 2];
        for x in &c[..n] {
            let clip = graph.clip(x.clip);
            let track = clip.feet?;
            // A clip state's time is its unwrapped clock.
            let t = if clip.looping && clip.duration > 0.0 {
                x.time - floor(x.time / clip.duration) * clip.duration
            } else {
                x.time.clamp(0.0, clip.duration)
            };
            let f = track.at(t);
            out[0] += f[0] * x.weight;
            out[1] += f[1] * x.weight;
        }
        (n > 0).then_some(out)
    }

    /// The pose to draw: up to three clips as an [`AnimRef`] (a fourth,
    /// lightest one is dropped and the rest renormalized — only while a
    /// blend space fades into another).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn anim(&self, graph: &AnimGraph) -> AnimRef {
        let mut c = [Contribution {
            clip: 0,
            time: 0.0,
            weight: 0.0,
        }; 4];
        let mut n = self.contributions(graph, &mut c);
        if n == 4 {
            let mut min = 0;
            for i in 1..4 {
                if c[i].weight < c[min].weight {
                    min = i;
                }
            }
            for i in min..3 {
                c[i] = c[i + 1];
            }
            n = 3;
        }
        let total: f32 = c[..n].iter().map(|x| x.weight).sum();
        let id = |x: &Contribution| graph.clip(x.clip).id;
        let mut a = AnimRef::new(id(&c[0]), c[0].time);
        if n >= 2 && total > 0.0 {
            let w01 = c[0].weight + c[1].weight;
            let k1 = if w01 > 0.0 { c[1].weight / w01 } else { 0.0 };
            a = a.blend_to(id(&c[1]), c[1].time, k1);
            if n == 3 {
                a.over = AnimBlend {
                    clip: id(&c[2]),
                    time: c[2].time,
                    weight: c[2].weight / total,
                };
            }
        }
        a
    }

    /// A digest of the playback state (state, clocks quantized to 1/4096
    /// of a cycle, fade, parameters): the self-test compares it across
    /// targets.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn digest(&self) -> u32 {
        let mut h = Fnv::new();
        let q = |v: f32| crate::math3d::to_i32(v * 4096.0) as u32;
        h.u32(self.cur.state as u32);
        h.u32(q(self.cur.phase));
        h.u32(self.cur.cycles as u32);
        match self.prev {
            Some(p) => {
                h.u32(p.state as u32 + 1);
                h.u32(q(p.phase));
                h.u32(q(self.fade_k()));
            }
            None => h.u32(0),
        }
        for p in &self.params {
            h.u32(q(*p));
        }
        h.u32(self.transitions);
        h.0
    }
}

/// FNV-1a over words (digests).
pub(crate) struct Fnv(pub u32);

impl Fnv {
    pub(crate) fn new() -> Fnv {
        Fnv(0x811c_9dc5)
    }

    pub(crate) fn u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.0 ^= b as u32;
            self.0 = self.0.wrapping_mul(0x0100_0193);
        }
    }
}

/// A graph baked into a blob (`anim/<name>`), re-parsed when the blob
/// changes (live reload). Fetch it in `update_with`, where the platform is.
pub struct GraphBlob {
    id: crate::platform::BlobId,
    bytes: Option<&'static [u8]>,
    graph: Option<AnimGraph<'static>>,
    /// Why the last bytes did not parse.
    pub error: Option<GraphError>,
}

impl GraphBlob {
    pub const fn from_id(id: crate::platform::BlobId) -> GraphBlob {
        GraphBlob {
            id,
            bytes: None,
            graph: None,
            error: None,
        }
    }

    /// The blob of `anim/<name>`: `GraphBlob::new("strider")`.
    pub fn new(name: &str) -> GraphBlob {
        let mut path = crate::text::TextBuf::<64>::new();
        let _ = core::fmt::Write::write_fmt(&mut path, format_args!("anim/{name}"));
        GraphBlob::from_id(crate::platform::BlobId::from_path(path.as_str()))
    }

    /// The graph, re-parsed when the blob changed; `None` while it is
    /// missing or broken ([`error`](Self::error)). An instance made from an
    /// older graph keeps playing if its indices still fit; restart it on a
    /// change that removed states ([`changed`](Self::get)'s second value).
    pub fn get(
        &mut self,
        platform: &dyn crate::platform::Platform,
    ) -> (Option<AnimGraph<'static>>, bool) {
        let bytes = platform.blob(self.id);
        let changed = bytes.map(<[u8]>::as_ptr) != self.bytes.map(<[u8]>::as_ptr);
        if changed {
            self.bytes = bytes;
            self.graph = None;
            self.error = None;
            if let Some(b) = bytes {
                match AnimGraph::parse(b) {
                    Ok(g) => self.graph = Some(g),
                    Err(e) => self.error = Some(e),
                }
            }
        }
        (self.graph, changed)
    }

    /// The graph as last fetched.
    pub fn graph(&self) -> Option<AnimGraph<'static>> {
        self.graph
    }
}
