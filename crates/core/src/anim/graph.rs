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
//! Playback reads a blob byte by byte, or the tables it decodes to once
//! ([`GraphTables`](crate::anim::GraphTables), issue #138): see
//! [`crate::anim::tables`].
//!
//! ## `TANG` v3 (little-endian; offsets from the blob's start)
//!
//! v2 (issue #112) is v1 plus layers, 2D blend spaces and inertialized
//! transitions; a v1 blob is a v2 blob with none of them (its reserved
//! header bytes are 0) and plays exactly as before. v3 (issue #138) is v2
//! plus a flags byte per layer (bit 0: synced to the base's cycle) and the
//! binds after the layer records (other models the graph serves, each with
//! its own masks); the bake writes v3 only for a graph that uses them.
//!
//! ```text
//! header (32 B): "TANG" u32 version, u32 model id, u16 params, u16 clips,
//!                u16 states, u16 transitions, u16 start state,
//!                u16 layers (v2: over the base; 0 in v1), u32 layers offset,
//!                u16 base states, u16 base transitions (v2: the base layer's
//!                share, first in their tables; 0 = all of them)
//! params      (12 B each): u32 name id, u8 kind (0 float, 1 bool, 2 trigger), u8 0 *3, f32 default
//! clips       (24 B each): u32 clip id, f32 duration, u8 loops, u8 0 *3,
//!                          u32 events offset, u32 root track offset, u32 feet offset
//!                          (NONE = 0xFFFF_FFFF)
//! states      (32 B each): u32 name id, u8 kind (0 clip, 1 blend 1D, 2 blend 2D), u8 flags
//!                          (bit 0 root motion, bit 1 loop override, bit 2 its value),
//!                          u16 points, f32 speed, u16 speed param, u16 blend param
//!                          (NONE = 0xFFFF), f32 wrap (0 = none), u32 points offset,
//!                          u32 events offset, u32 triangles offset (2D)
//!                          (2D: the wrap field is u16 y param, u16 triangles)
//! transitions (12 B each): u16 from (0xFFFF = any state of its layer), u16 to
//!                          (bit 15: inertialized), f32 blend seconds,
//!                          u32 condition offset
//! layers      (28 B each, at the layers offset): u32 name id, u32 joint mask,
//!                          u8 mode (0 override, 1 additive), u8 flags (v3;
//!                          0 before), u16 weight
//!                          param (NONE = none), f32 weight, u16 first state,
//!                          u16 states, u16 first transition, u16 transitions,
//!                          u16 start state, u16 0
//! binds       (v3, right after the layer records): u16 count, u16 0, then
//!                          per model u32 model id and a u32 mask per layer
//! data: points (8 B: f32 value, u16 clip, u16 0; 2D: 12 B, f32 x, f32 y,
//!       u16 clip, u16 0), triangles (3 × u8 point), event tracks
//!       ([`EventTrack`]: u32 n + records), root tracks ([`RootTrack`]),
//!       feet tracks ([`FeetTrack`]: f32 fps, u32 frames, frames × 2 × f32*3,
//!       the IK chains' ankles in model space, left then right),
//!       conditions (prefix code: u8 op + operands, see [`Cond`]).
//! ```
//!
//! Names are hashes ([`ParamId`]/[`StateId`] via `from_name`); the RON keeps
//! them for tools.

use super::clip::{EventTrack, RootTrack};
use super::{rd_f32, rd_u8, rd_u16, rd_u32};
use crate::render::ClipId;

pub use super::instance::{AnimGraphInstance, Contribution, GraphInstance};

pub const TANG_MAGIC: &[u8; 4] = b"TANG";
pub const TANG_VERSION: u32 = 3;
/// Layer record flags (v3, at +9).
pub const LAYER_SYNC: u8 = 1;
/// Parameters one graph may declare (the instance keeps them inline).
pub const MAX_PARAMS: usize = 16;
/// Points of one blend space (eight directions and the middle, and more).
pub const MAX_POINTS: usize = 16;
/// Triangles of one 2D blend space.
pub const MAX_TRIANGLES: usize = 24;
/// Layers one graph may declare over its base.
pub const MAX_LAYERS: usize = crate::anim::MAX_POSE_LAYERS - 1;
/// Clip shares one layer's pose has at most (a 2D space fading into another).
pub const MAX_CONTRIBUTIONS: usize = 6;
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
pub const POINT2_BYTES: usize = 12;
pub const LAYER_BYTES: usize = 28;
/// A transition's `to` bit: inertialized (no crossfade, a decaying offset).
pub const INERTIAL: u16 = 0x8000;

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
    layers: usize,
    layers_at: usize,
    base_states: usize,
    base_transitions: usize,
    /// v3: other models the graph serves, each with its own layer masks.
    binds: usize,
    /// The model this view's masks are for: `NONE16` = the header's, else
    /// a bind (`for_model`).
    bind: u16,
}

/// One layer over a graph's base (v2): its own state machine, playing on
/// the joints of its mask, by its weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphLayer {
    pub id: u32,
    pub mask: crate::anim::JointMask,
    pub mode: crate::anim::LayerMode,
    /// A float parameter (clamped 0..1) the weight is multiplied by.
    pub weight_param: Option<usize>,
    pub weight: f32,
    pub states: (usize, usize),
    pub transitions: (usize, usize),
    pub start: usize,
    /// Its clock follows the base's cycle (v3, issue #138): a looping
    /// state of the layer lands where the base's looping state is, as a
    /// fraction of its cycle, every update.
    pub sync: bool,
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

    /// No track (a graph table's clip without IK feet).
    pub const EMPTY: FeetTrack<'static> = FeetTrack {
        fps: 0.0,
        frames: 0,
        records: &[],
    };

    /// A track [`parse`](Self::parse) validated before, from its parts (the
    /// static tables `cargo xtask anim export --rust` writes, issue #138).
    pub const fn from_parts(fps: f32, frames: usize, records: &'a [u8]) -> FeetTrack<'a> {
        FeetTrack {
            fps,
            frames,
            records,
        }
    }

    pub fn fps(&self) -> f32 {
        self.fps
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Its records (24 bytes a frame), for the static export.
    pub fn records(&self) -> &'a [u8] {
        self.records
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
    /// A blend space over `param` (`Some`; 2D: the x axis), or one clip.
    pub param: Option<usize>,
    /// A 2D blend space's y parameter.
    pub param_y: Option<usize>,
    /// Triangles of a 2D blend space.
    pub triangles: usize,
    /// Where its triangles start: a byte offset in a blob, an index in a
    /// [`GraphTables`](crate::anim::GraphTables)' triangle table.
    pub tris_at: usize,
    pub points: usize,
    pub speed: f32,
    pub speed_param: Option<usize>,
    pub wrap: f32,
    pub root_motion: bool,
    /// The graph's own loop setting for a clip state (`None`: the clip's).
    pub looping: Option<bool>,
    /// Where its points start (as [`tris_at`](Self::tris_at)).
    pub points_at: usize,
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
        if !(1..=TANG_VERSION).contains(&version) {
            return Err(GraphError::Version(version));
        }
        let states = rd_u16(bytes, 16) as usize;
        let transitions = rd_u16(bytes, 18) as usize;
        let (layers, layers_at, bs, bt) = if version >= 2 {
            (
                rd_u16(bytes, 22) as usize,
                rd_u32(bytes, 24) as usize,
                rd_u16(bytes, 28) as usize,
                rd_u16(bytes, 30) as usize,
            )
        } else {
            (0, 0, 0, 0)
        };
        let g = AnimGraph {
            bytes,
            params: rd_u16(bytes, 12) as usize,
            clips: rd_u16(bytes, 14) as usize,
            states,
            transitions,
            start: rd_u16(bytes, 20) as usize,
            layers,
            layers_at,
            base_states: if bs == 0 { states } else { bs },
            base_transitions: if bs == 0 && bt == 0 { transitions } else { bt },
            binds: 0,
            bind: NONE16,
        };
        if g.params > MAX_PARAMS || g.states > MAX_STATES || g.layers > MAX_LAYERS {
            return Err(GraphError::TooBig);
        }
        if g.states == 0
            || g.start >= g.base_states
            || g.base_states > g.states
            || g.base_transitions > g.transitions
        {
            return Err(GraphError::Corrupt);
        }
        if bytes.len() < g.transitions_at() + g.transitions * TRANSITION_BYTES
            || (g.layers > 0 && bytes.len() < g.layers_at + g.layers * LAYER_BYTES)
        {
            return Err(GraphError::Truncated);
        }
        // v3: the binds after the layer records (`u16 count, u16 0`, then
        // per model `u32 id` and a mask per layer).
        let mut g = g;
        if version >= 3 && g.layers > 0 {
            let at = g.layers_at + g.layers * LAYER_BYTES;
            if bytes.len() < at + 4 {
                return Err(GraphError::Truncated);
            }
            g.binds = rd_u16(bytes, at) as usize;
            if bytes.len() < at + 4 + g.binds * g.bind_bytes() {
                return Err(GraphError::Truncated);
            }
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
            g.check_state(s)?;
        }
        // Each layer's states and transitions: a range after the base's,
        // in order; its transitions stay inside it.
        let (mut next_s, mut next_t) = (g.base_states, g.base_transitions);
        for l in 0..g.layers {
            let at = g.layers_at + l * LAYER_BYTES;
            let wp = rd_u16(bytes, at + 10);
            let (s0, sn) = (
                rd_u16(bytes, at + 16) as usize,
                rd_u16(bytes, at + 18) as usize,
            );
            let (t0, tn) = (
                rd_u16(bytes, at + 20) as usize,
                rd_u16(bytes, at + 22) as usize,
            );
            let start = rd_u16(bytes, at + 24) as usize;
            // The flags byte: 0 before v3 (ignored there).
            if (version >= 3 && rd_u8(bytes, at + 9) & !LAYER_SYNC != 0)
                || rd_u8(bytes, at + 8) > 1
                || (wp != NONE16 && wp as usize >= g.params)
                || !(0.0..=1.0).contains(&rd_f32(bytes, at + 12))
                || s0 != next_s
                || sn == 0
                || t0 != next_t
                || !(s0..s0 + sn).contains(&start)
            {
                return Err(GraphError::Corrupt);
            }
            next_s = s0 + sn;
            next_t = t0 + tn;
        }
        if next_s != g.states || next_t != g.transitions {
            return Err(GraphError::Corrupt);
        }
        for t in 0..g.transitions {
            let at = g.transitions_at() + t * TRANSITION_BYTES;
            let (from, to) = (
                rd_u16(bytes, at),
                (rd_u16(bytes, at + 2) & !INERTIAL) as usize,
            );
            let (lo, hi) = g.states_of(g.layer_of_transition(t));
            if (from != NONE16 && !(lo..hi).contains(&(from as usize))) || !(lo..hi).contains(&to) {
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

    /// Validate state `s` (its kind, points, floats, triangles, events).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn check_state(&self, s: usize) -> Result<(), GraphError> {
        let bytes = self.bytes;
        let at = self.states_at() + s * STATE_BYTES;
        let kind = rd_u8(bytes, at + 4);
        let points = rd_u16(bytes, at + 6) as usize;
        let speed_param = rd_u16(bytes, at + 12);
        let param = rd_u16(bytes, at + 14);
        let points_at = rd_u32(bytes, at + 20) as usize;
        if kind > 2 || points == 0 || points > MAX_POINTS || (kind == 0 && points != 1) {
            return Err(if points > MAX_POINTS {
                GraphError::TooBig
            } else {
                GraphError::Corrupt
            });
        }
        let sane = |v: f32| (0.0..1.0e6).contains(&v);
        if !sane(rd_f32(bytes, at + 8)) || (kind != 2 && !sane(rd_f32(bytes, at + 16))) {
            return Err(GraphError::Corrupt);
        }
        let stride = if kind == 2 { POINT2_BYTES } else { POINT_BYTES };
        if (kind >= 1 && param as usize >= self.params)
            || (speed_param != NONE16 && speed_param as usize >= self.params)
            || bytes.len() < points_at + points * stride
        {
            return Err(GraphError::Corrupt);
        }
        let mut last = f32::MIN;
        for p in 0..points {
            let v = rd_f32(bytes, points_at + p * stride);
            let clip = rd_u16(bytes, points_at + p * stride + stride - 4) as usize;
            if clip >= self.clips || !v.is_finite() || (kind != 2 && v < last) {
                return Err(GraphError::Corrupt);
            }
            if kind == 2 && !rd_f32(bytes, points_at + p * stride + 4).is_finite() {
                return Err(GraphError::Corrupt);
            }
            last = v;
        }
        if kind == 2 {
            let y = rd_u16(bytes, at + 16) as usize;
            let tris = rd_u16(bytes, at + 18) as usize;
            let tris_at = rd_u32(bytes, at + 28) as usize;
            if y >= self.params
                || tris > MAX_TRIANGLES
                || (points >= 3) != (tris > 0)
                || bytes.len() < tris_at + tris * 3
                || bytes[tris_at..tris_at + tris * 3]
                    .iter()
                    .any(|&k| k as usize >= points)
            {
                return Err(GraphError::Corrupt);
            }
        }
        let events = rd_u32(bytes, at + 24);
        if events != NONE32 {
            bytes
                .get(events as usize..)
                .and_then(EventTrack::parse)
                .ok_or(GraphError::Corrupt)?;
        }
        Ok(())
    }

    /// The layer a transition belongs to (0 = the base).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn layer_of_transition(&self, t: usize) -> usize {
        if t < self.base_transitions {
            return 0;
        }
        (0..self.layers)
            .find(|&l| {
                let at = self.layers_at + l * LAYER_BYTES;
                let (t0, tn) = (
                    rd_u16(self.bytes, at + 20) as usize,
                    rd_u16(self.bytes, at + 22) as usize,
                );
                (t0..t0 + tn).contains(&t)
            })
            .map_or(0, |l| l + 1)
    }

    /// States `lo..hi` of layer `l` (0 = the base).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn states_of(&self, l: usize) -> (usize, usize) {
        if l == 0 {
            (0, self.base_states)
        } else {
            let at = self.layers_at + (l - 1) * LAYER_BYTES;
            let s0 = rd_u16(self.bytes, at + 16) as usize;
            (s0, s0 + rd_u16(self.bytes, at + 18) as usize)
        }
    }

    /// Transitions `lo..hi` of layer `l` (0 = the base).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn transitions_of(&self, l: usize) -> (usize, usize) {
        if l == 0 {
            (0, self.base_transitions)
        } else {
            let at = self.layers_at + (l - 1) * LAYER_BYTES;
            let t0 = rd_u16(self.bytes, at + 20) as usize;
            (t0, t0 + rd_u16(self.bytes, at + 22) as usize)
        }
    }

    /// Layers over the base (0 for a v1 graph).
    pub fn layer_count(&self) -> usize {
        self.layers
    }

    /// Layer `l` over the base (`1..=layer_count()`).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn layer(&self, l: usize) -> GraphLayer {
        let at = self.layers_at + (l.max(1) - 1) * LAYER_BYTES;
        let b = self.bytes;
        let wp = rd_u16(b, at + 10);
        let mask = if self.bind == NONE16 {
            rd_u32(b, at + 4)
        } else {
            rd_u32(
                b,
                self.binds_at() + self.bind as usize * self.bind_bytes() + 4 * l.max(1),
            )
        };
        GraphLayer {
            id: rd_u32(b, at),
            mask: crate::anim::JointMask(mask),
            mode: if rd_u8(b, at + 8) == 1 {
                crate::anim::LayerMode::Additive
            } else {
                crate::anim::LayerMode::Override
            },
            weight_param: (wp != NONE16).then_some(wp as usize),
            weight: rd_f32(b, at + 12),
            states: self.states_of(l),
            transitions: self.transitions_of(l),
            start: rd_u16(b, at + 24) as usize,
            sync: self.version() >= 3 && rd_u8(b, at + 9) & LAYER_SYNC != 0,
        }
    }

    /// The blob's `TANG` version (1, 2 or 3).
    pub fn version(&self) -> u32 {
        rd_u32(self.bytes, 4)
    }

    /// Where the binds start (v3: past their 4-byte count).
    fn binds_at(&self) -> usize {
        self.layers_at + self.layers * LAYER_BYTES + 4
    }

    /// One bind's bytes: its model id and a mask per layer.
    fn bind_bytes(&self) -> usize {
        4 + 4 * self.layers
    }

    /// Models besides [`model`](Self::model)'s own the graph serves (v3,
    /// issue #138): rigs that differ by joints a mask doesn't name (a prop,
    /// a tool), each with its layer masks resolved by joint name at the
    /// bake.
    pub fn bind_count(&self) -> usize {
        self.binds
    }

    /// Bind `i`'s model id.
    pub fn bind_model(&self, i: usize) -> u32 {
        rd_u32(self.bytes, self.binds_at() + i * self.bind_bytes())
    }

    /// This graph for `model` (`ModelId(id)`): the same graph whose layer
    /// masks are that model's, `None` when the graph doesn't serve it.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn for_model(&self, model: u32) -> Option<AnimGraph<'a>> {
        if rd_u32(self.bytes, 8) == model {
            return Some(AnimGraph {
                bind: NONE16,
                ..*self
            });
        }
        (0..self.binds)
            .find(|&i| self.bind_model(i) == model)
            .map(|i| AnimGraph {
                bind: i as u16,
                ..*self
            })
    }

    /// The layer named `name` (`1..=layer_count()`), for a game that reads
    /// a layer as data (its mask, its clips) and lays it over its own base
    /// (Solid Eleven's kicks, Plan & Party's carrying: issue #136).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn layer_named(&self, name: &str) -> Option<usize> {
        let id = crate::asset::asset_id(name);
        (1..=self.layers).find(|&l| self.layer(l).id == id)
    }

    /// The clip a clip state plays (`None`: a blend space, or no state
    /// `i`).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn state_clip(&self, i: usize) -> Option<ClipId> {
        if i >= self.states {
            return None;
        }
        let s = self.state(i);
        if s.param.is_some() || s.points == 0 {
            return None;
        }
        Some(self.clip(self.point(&s, 0).1).id)
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

    /// The model the graph animates (`ModelId(id)`): the one
    /// [`for_model`](Self::for_model) chose, else the graph's own.
    pub fn model(&self) -> u32 {
        if self.bind == NONE16 {
            rd_u32(self.bytes, 8)
        } else {
            self.bind_model(self.bind as usize)
        }
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

    /// Clip `i`'s seconds.
    pub fn clip_duration(&self, i: usize) -> f32 {
        rd_f32(self.bytes, self.clips_at() + i * CLIP_BYTES + 4)
    }

    /// Whether clip `i` loops.
    pub fn clip_looping(&self, i: usize) -> bool {
        rd_u8(self.bytes, self.clips_at() + i * CLIP_BYTES + 8) != 0
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
        let kind = rd_u8(b, at + 4);
        GraphState {
            id: StateId(rd_u32(b, at)),
            param: (kind >= 1).then(|| rd_u16(b, at + 14) as usize),
            param_y: (kind == 2).then(|| rd_u16(b, at + 16) as usize),
            triangles: if kind == 2 {
                rd_u16(b, at + 18) as usize
            } else {
                0
            },
            tris_at: rd_u32(b, at + 28) as usize,
            points: rd_u16(b, at + 6) as usize,
            speed: rd_f32(b, at + 8),
            speed_param: (speed_param != NONE16).then_some(speed_param as usize),
            wrap: if kind == 2 { 0.0 } else { rd_f32(b, at + 16) },
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

    /// Point `p` of a 2D state: (x, y, clip index).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn point2(&self, s: &GraphState, p: usize) -> (f32, f32, usize) {
        let at = s.points_at + p * POINT2_BYTES;
        (
            rd_f32(self.bytes, at),
            rd_f32(self.bytes, at + 4),
            rd_u16(self.bytes, at + 8) as usize,
        )
    }

    /// Triangle `t` of a 2D state: three point indices.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn triangle(&self, s: &GraphState, t: usize) -> [usize; 3] {
        let at = s.tris_at + t * 3;
        [0, 1, 2].map(|k| rd_u8(self.bytes, at + k) as usize)
    }

    /// Transition `t`: (from state or `None` = any, to, blend seconds,
    /// condition offset).
    pub fn transition(&self, t: usize) -> (Option<usize>, usize, f32, usize) {
        let at = self.transitions_at() + t * TRANSITION_BYTES;
        let from = rd_u16(self.bytes, at);
        (
            (from != NONE16).then_some(from as usize),
            (rd_u16(self.bytes, at + 2) & !INERTIAL) as usize,
            rd_f32(self.bytes, at + 4),
            rd_u32(self.bytes, at + 8) as usize,
        )
    }

    /// Whether transition `t` is inertialized (v2).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn transition_inertial(&self, t: usize) -> bool {
        let at = self.transitions_at() + t * TRANSITION_BYTES;
        rd_u16(self.bytes, at + 2) & INERTIAL != 0
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

pub(crate) struct EvalCtx<'p> {
    pub(crate) params: &'p [f32; MAX_PARAMS],
    pub(crate) finished: bool,
    pub(crate) elapsed: f32,
}

/// One condition node's code, read from the blob (the decoder's view).
pub(crate) struct CondCode {
    pub(crate) op: u8,
    pub(crate) param: u16,
    pub(crate) children: u8,
    pub(crate) value: f32,
    /// Bytes of its own code (its children follow).
    pub(crate) len: usize,
}

impl AnimGraph<'_> {
    /// [`eval`](Self::eval) of the condition at byte `at`.
    pub(crate) fn eval_at(
        &self,
        at: usize,
        params: &[f32; MAX_PARAMS],
        finished: bool,
        elapsed: f32,
        used: &mut u16,
    ) -> bool {
        let ctx = EvalCtx {
            params,
            finished,
            elapsed,
        };
        self.eval(at, &ctx, used).0
    }

    /// The node at byte `at` (validated by `parse`).
    pub(crate) fn cond_code(&self, at: usize) -> CondCode {
        let b = self.bytes;
        let op = rd_u8(b, at);
        let (param, children, value, len) = match op {
            op::TRIGGER | op::PARAM => (rd_u16(b, at + 1), 0, 0.0, 3),
            op::GREATER | op::LESS => (rd_u16(b, at + 1), 0, rd_f32(b, at + 3), 7),
            op::AFTER => (0, 0, rd_f32(b, at + 1), 5),
            op::NOT => (0, 1, 0.0, 1),
            op::AND | op::OR => (0, rd_u8(b, at + 1), 0.0, 2),
            _ => (0, 0, 0.0, 1),
        };
        CondCode {
            op,
            param,
            children,
            value,
            len,
        }
    }

    /// The whole length of the condition at byte `at`, its children in.
    pub(crate) fn cond_len(&self, at: usize) -> usize {
        self.check_cond(at, 0).unwrap_or(1)
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
