//! An animation graph decoded once into native tables (issue #138).
//!
//! [`AnimGraph`] reads its `TANG` blob byte by byte: every update decodes
//! the playing states, the clips' durations and tracks, and walks each
//! transition's condition code, ~1 000 bounds-checked reads a crowd member
//! pays on every frame (~400 µs on the N64). [`GraphTables`] holds the same
//! graph as plain records: states, points, triangles, clips, transitions,
//! layers and the conditions flattened into nodes that `And`/`Or` can skip.
//! The playback ([`GraphInstance`](crate::anim::GraphInstance)) runs over
//! either through [`GraphSource`], with the same arithmetic in the same
//! order, so both give the same poses, events and digests.
//!
//! Tables come from:
//! - [`GraphTables::decode_in`]: at run time, from a parsed blob, into an
//!   [`Arena`](crate::mem::Arena) ([`GraphTables::bytes_for`] bytes: about
//!   2 KB for Solid Eleven's player graph);
//! - `cargo xtask anim export <name> --rust <out.rs>`: static Rust data a
//!   game `include!`s. A console build that only has those never links the
//!   blob parser ([`AnimGraph::parse`]) nor the decoder.

use super::clip::{EventTrack, RootTrack};
use super::graph::{
    AnimGraph, FeetTrack, GraphClip, GraphLayer, GraphState, MAX_PARAMS, NONE16, ParamId,
    ParamKind, StateId, op,
};
use crate::render::ClipId;

/// A transition of a graph, decoded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// The state it leaves (`NONE16`: any state of its layer).
    pub from: u16,
    pub to: u16,
    /// Inertialized (no crossfade, a decaying offset).
    pub inertial: bool,
    /// Crossfade (or decay) seconds.
    pub blend: f32,
    /// Its condition: a node of [`GraphTables::conds`].
    pub cond: u16,
}

/// One node of a flattened condition (prefix order: a node's children
/// follow it). `end` is the index past its subtree, so an `And` that
/// failed or an `Or` that held jumps over the rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CondNode {
    /// An [`op`] code.
    pub op: u8,
    /// The parameter `TRIGGER`/`PARAM`/`GREATER`/`LESS` read.
    pub param: u8,
    /// Children of `AND`/`OR` (one for `NOT`).
    pub children: u8,
    /// Its subtree reads a trigger outside a `Not`: an `Or` that already
    /// holds still evaluates it, to consume what it reads (a trigger is
    /// consumed by the transition that fires on it).
    pub trigger: bool,
    pub end: u16,
    /// `GREATER`/`LESS`'s value, `AFTER`'s seconds.
    pub value: f32,
}

/// One point of a blend space (`y` = 0 in 1D).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub clip: u16,
}

/// One clip a graph plays. The tracks are empty (no frames, no records)
/// where the clip has none: no float behind an `Option` on the N64 (an
/// uninitialized payload can be a denormal, which traps).
#[derive(Clone, Copy, Debug)]
pub struct ClipRec<'a> {
    pub id: ClipId,
    pub duration: f32,
    pub looping: bool,
    pub events: EventTrack<'a>,
    pub root: RootTrack<'a>,
    pub feet: FeetTrack<'a>,
}

impl<'a> ClipRec<'a> {
    fn clip(&self) -> GraphClip<'a> {
        GraphClip {
            id: self.id,
            duration: self.duration,
            looping: self.looping,
            events: self.events,
            root: (self.root.frames() > 0).then_some(self.root),
            feet: (self.feet.frames() > 0).then_some(self.feet),
        }
    }
}

/// What playback reads from a graph: its blob ([`AnimGraph`]) or its
/// decoded tables ([`GraphTables`]). Indices are the blob's (states,
/// transitions, clips, layers 1..=`layer_count`).
pub trait GraphSource {
    /// The model whose joints the layer masks count (`ModelId(id)`).
    fn model(&self) -> u32;
    fn param_count(&self) -> usize;
    /// Parameter `i`: (id, kind, default).
    fn param_info(&self, i: usize) -> (ParamId, ParamKind, f32);
    fn state_count(&self) -> usize;
    fn clip_count(&self) -> usize;
    /// The base's start state.
    fn start(&self) -> usize;
    /// Layers over the base.
    fn layer_count(&self) -> usize;
    /// Layer `l` (`1..=layer_count()`).
    fn layer(&self, l: usize) -> GraphLayer;
    /// States `lo..hi` of layer `l` (0 = the base).
    fn states_of(&self, l: usize) -> (usize, usize);
    /// Transitions `lo..hi` of layer `l`.
    fn transitions_of(&self, l: usize) -> (usize, usize);
    fn state(&self, i: usize) -> GraphState<'_>;
    fn clip(&self, i: usize) -> GraphClip<'_>;
    /// Clip `i`'s seconds ([`clip`](Self::clip)`(i).duration`, read alone).
    fn clip_duration(&self, i: usize) -> f32 {
        self.clip(i).duration
    }
    /// Whether clip `i` loops (`clip(i).looping`).
    fn clip_looping(&self, i: usize) -> bool {
        self.clip(i).looping
    }
    /// Point `p` of a 1D (or clip) state: (value, clip index).
    fn point(&self, s: &GraphState, p: usize) -> (f32, usize);
    /// Point `p` of a 2D state: (x, y, clip index).
    fn point2(&self, s: &GraphState, p: usize) -> (f32, f32, usize);
    /// Triangle `t` of a 2D state.
    fn triangle(&self, s: &GraphState, t: usize) -> [usize; 3];
    /// Transition `t`.
    fn edge(&self, t: usize) -> Edge;
    /// Whether transition `t`'s condition holds; `used` collects the
    /// triggers it read (to consume them when it fires).
    #[doc(hidden)]
    fn holds(
        &self,
        t: usize,
        params: &[f32; MAX_PARAMS],
        finished: bool,
        elapsed: f32,
        used: &mut u16,
    ) -> bool;

    /// The trigger parameters (bit `i` = parameter `i`).
    fn triggers(&self) -> u16 {
        let mut m = 0;
        for i in 0..self.param_count().min(MAX_PARAMS) {
            if self.param_info(i).1 == ParamKind::Trigger {
                m |= 1 << i;
            }
        }
        m
    }

    /// The index of a parameter (cache it).
    fn param(&self, id: ParamId) -> Option<usize> {
        (0..self.param_count()).find(|&i| self.param_info(i).0 == id)
    }

    fn param_named(&self, name: &str) -> Option<usize> {
        self.param(ParamId::from_name(name))
    }

    /// The index of a state.
    fn state_index(&self, id: StateId) -> Option<usize> {
        (0..self.state_count()).find(|&i| self.state(i).id == id)
    }

    /// The layer named `name` (`1..=layer_count()`).
    fn layer_named(&self, name: &str) -> Option<usize> {
        let id = crate::asset::asset_id(name);
        (1..=self.layer_count()).find(|&l| self.layer(l).id == id)
    }

    /// The clip a clip state plays (`None`: a blend space, or no state `i`).
    fn state_clip(&self, i: usize) -> Option<ClipId> {
        if i >= self.state_count() {
            return None;
        }
        let s = self.state(i);
        if s.param.is_some() || s.points == 0 {
            return None;
        }
        Some(self.clip(self.point(&s, 0).1).id)
    }
}

impl GraphSource for AnimGraph<'_> {
    fn model(&self) -> u32 {
        AnimGraph::model(self)
    }
    fn param_count(&self) -> usize {
        AnimGraph::param_count(self)
    }
    fn param_info(&self, i: usize) -> (ParamId, ParamKind, f32) {
        AnimGraph::param_info(self, i)
    }
    fn state_count(&self) -> usize {
        AnimGraph::state_count(self)
    }
    fn clip_count(&self) -> usize {
        AnimGraph::clip_count(self)
    }
    fn start(&self) -> usize {
        AnimGraph::start(self)
    }
    fn layer_count(&self) -> usize {
        AnimGraph::layer_count(self)
    }
    fn layer(&self, l: usize) -> GraphLayer {
        AnimGraph::layer(self, l)
    }
    fn states_of(&self, l: usize) -> (usize, usize) {
        AnimGraph::states_of(self, l)
    }
    fn transitions_of(&self, l: usize) -> (usize, usize) {
        AnimGraph::transitions_of(self, l)
    }
    fn state(&self, i: usize) -> GraphState<'_> {
        AnimGraph::state(self, i)
    }
    fn clip(&self, i: usize) -> GraphClip<'_> {
        AnimGraph::clip(self, i)
    }
    fn clip_duration(&self, i: usize) -> f32 {
        AnimGraph::clip_duration(self, i)
    }
    fn clip_looping(&self, i: usize) -> bool {
        AnimGraph::clip_looping(self, i)
    }
    fn point(&self, s: &GraphState, p: usize) -> (f32, usize) {
        AnimGraph::point(self, s, p)
    }
    fn point2(&self, s: &GraphState, p: usize) -> (f32, f32, usize) {
        AnimGraph::point2(self, s, p)
    }
    fn triangle(&self, s: &GraphState, t: usize) -> [usize; 3] {
        AnimGraph::triangle(self, s, t)
    }
    fn edge(&self, t: usize) -> Edge {
        let (from, to, blend, cond) = AnimGraph::transition(self, t);
        Edge {
            from: from.map_or(NONE16, |f| f as u16),
            to: to as u16,
            inertial: AnimGraph::transition_inertial(self, t),
            blend,
            // A byte offset here (`holds` reads it again); only tables
            // index nodes.
            cond: cond as u16,
        }
    }
    fn holds(
        &self,
        t: usize,
        params: &[f32; MAX_PARAMS],
        finished: bool,
        elapsed: f32,
        used: &mut u16,
    ) -> bool {
        let cond = AnimGraph::transition(self, t).3;
        self.eval_at(cond, params, finished, elapsed, used)
    }
}

/// A graph as native tables: what [`GraphTables::decode_in`] fills and
/// `cargo xtask anim export --rust` writes as static data. The fields are
/// public for that export; build them only from one of those two.
#[derive(Clone, Copy, Debug)]
pub struct GraphTables<'a> {
    pub model: u32,
    pub params: &'a [(ParamId, ParamKind, f32)],
    pub clips: &'a [ClipRec<'a>],
    /// Every state, the base's first then each layer's. A state's
    /// `points_at`/`tris_at` index [`points`](Self::points) and
    /// [`triangles`](Self::triangles).
    pub states: &'a [GraphState<'a>],
    pub points: &'a [Point],
    pub triangles: &'a [[u8; 3]],
    pub edges: &'a [Edge],
    pub conds: &'a [CondNode],
    /// Layers 1..=n over the base (index 0 is layer 1).
    pub layers: &'a [GraphLayer],
    pub start: u16,
    pub base_states: u16,
    pub base_transitions: u16,
    /// The trigger parameters (bit `i` = parameter `i`).
    pub triggers: u16,
}

impl GraphTables<'_> {
    /// Whether node `i` holds (the blob's evaluation, with `And`/`Or`
    /// skipping what can't change the result or the triggers consumed).
    /// Leaves under an `And`/`Or` are read in its loop, not by a call.
    fn eval(&self, i: usize, ctx: &Ctx, used: &mut u16) -> bool {
        let Some(n) = self.conds.get(i) else {
            return false;
        };
        match n.op {
            op::NOT => {
                // A trigger under a Not is not consumed by it.
                let mut ignore = 0;
                !self.eval(i + 1, ctx, &mut ignore)
            }
            op::AND => {
                let mut mine = 0u16;
                let mut c = i + 1;
                for _ in 0..n.children {
                    let Some(x) = self.conds.get(c) else {
                        return false;
                    };
                    // A false child: the And is false and consumes nothing.
                    let v = if is_leaf(x.op) {
                        leaf(x, ctx, &mut mine)
                    } else {
                        self.eval(c, ctx, &mut mine)
                    };
                    if !v {
                        return false;
                    }
                    c = x.end as usize;
                }
                *used |= mine;
                true
            }
            op::OR => {
                let mut mine = 0u16;
                let mut acc = false;
                let mut c = i + 1;
                for _ in 0..n.children {
                    let Some(x) = self.conds.get(c) else {
                        break;
                    };
                    // Once it holds, only children that read a trigger
                    // still matter (each one on is consumed).
                    if !acc || x.trigger {
                        acc |= if is_leaf(x.op) {
                            leaf(x, ctx, &mut mine)
                        } else {
                            self.eval(c, ctx, &mut mine)
                        };
                    }
                    c = x.end as usize;
                }
                if acc {
                    *used |= mine;
                }
                acc
            }
            _ => leaf(n, ctx, used),
        }
    }

    /// The bytes these tables take.
    pub fn bytes(&self) -> usize {
        use core::mem::size_of_val;
        size_of_val(self.params)
            + size_of_val(self.clips)
            + size_of_val(self.states)
            + size_of_val(self.points)
            + size_of_val(self.triangles)
            + size_of_val(self.edges)
            + size_of_val(self.conds)
            + size_of_val(self.layers)
    }
}

/// Whether `op` reads no child.
#[inline(always)]
fn is_leaf(op: u8) -> bool {
    !matches!(op, op::AND | op::OR | op::NOT)
}

/// A leaf's value (`TRIGGER` adds itself to `used` when on).
#[inline(always)]
fn leaf(n: &CondNode, ctx: &Ctx, used: &mut u16) -> bool {
    let p = n.param as usize & (MAX_PARAMS - 1);
    match n.op {
        op::TRUE => true,
        op::FINISHED => ctx.finished,
        op::TRIGGER => {
            let on = ctx.params[p] != 0.0;
            if on {
                *used |= 1 << p;
            }
            on
        }
        op::PARAM => ctx.params[p] != 0.0,
        op::GREATER => ctx.params[p] > n.value,
        op::LESS => ctx.params[p] < n.value,
        op::AFTER => ctx.elapsed >= n.value,
        _ => false,
    }
}

struct Ctx<'p> {
    params: &'p [f32; MAX_PARAMS],
    finished: bool,
    elapsed: f32,
}

impl GraphSource for GraphTables<'_> {
    fn model(&self) -> u32 {
        self.model
    }
    fn param_count(&self) -> usize {
        self.params.len()
    }
    fn param_info(&self, i: usize) -> (ParamId, ParamKind, f32) {
        self.params
            .get(i)
            .copied()
            .unwrap_or((ParamId(0), ParamKind::Float, 0.0))
    }
    fn state_count(&self) -> usize {
        self.states.len()
    }
    fn clip_count(&self) -> usize {
        self.clips.len()
    }
    fn start(&self) -> usize {
        self.start as usize
    }
    fn layer_count(&self) -> usize {
        self.layers.len()
    }
    fn layer(&self, l: usize) -> GraphLayer {
        self.layers[l.max(1) - 1]
    }
    fn states_of(&self, l: usize) -> (usize, usize) {
        if l == 0 {
            (0, self.base_states as usize)
        } else {
            self.layers[l - 1].states
        }
    }
    fn transitions_of(&self, l: usize) -> (usize, usize) {
        if l == 0 {
            (0, self.base_transitions as usize)
        } else {
            self.layers[l - 1].transitions
        }
    }
    fn state(&self, i: usize) -> GraphState<'_> {
        self.states[i]
    }
    fn clip(&self, i: usize) -> GraphClip<'_> {
        self.clips[i].clip()
    }
    fn clip_duration(&self, i: usize) -> f32 {
        self.clips[i].duration
    }
    fn clip_looping(&self, i: usize) -> bool {
        self.clips[i].looping
    }
    fn point(&self, s: &GraphState, p: usize) -> (f32, usize) {
        let x = self.points[s.points_at + p];
        (x.x, x.clip as usize)
    }
    fn point2(&self, s: &GraphState, p: usize) -> (f32, f32, usize) {
        let x = self.points[s.points_at + p];
        (x.x, x.y, x.clip as usize)
    }
    fn triangle(&self, s: &GraphState, t: usize) -> [usize; 3] {
        self.triangles[s.tris_at + t].map(|k| k as usize)
    }
    fn edge(&self, t: usize) -> Edge {
        self.edges[t]
    }
    fn holds(
        &self,
        t: usize,
        params: &[f32; MAX_PARAMS],
        finished: bool,
        elapsed: f32,
        used: &mut u16,
    ) -> bool {
        let ctx = Ctx {
            params,
            finished,
            elapsed,
        };
        self.eval(self.edges[t].cond as usize, &ctx, used)
    }
    fn triggers(&self) -> u16 {
        self.triggers
    }
}

const NO_STATE: GraphState<'static> = GraphState {
    id: StateId(0),
    param: None,
    param_y: None,
    triangles: 0,
    tris_at: 0,
    points: 0,
    speed: 0.0,
    speed_param: None,
    wrap: 0.0,
    root_motion: false,
    looping: None,
    points_at: 0,
    events: EventTrack::EMPTY,
};

const NO_CLIP: ClipRec<'static> = ClipRec {
    id: ClipId(0),
    duration: 0.0,
    looping: false,
    events: EventTrack::EMPTY,
    root: RootTrack::EMPTY,
    feet: FeetTrack::EMPTY,
};

const NO_LAYER: GraphLayer = GraphLayer {
    id: 0,
    mask: crate::anim::JointMask(0),
    mode: crate::anim::LayerMode::Override,
    weight_param: None,
    weight: 0.0,
    states: (0, 0),
    transitions: (0, 0),
    start: 0,
    sync: false,
};

const NO_COND: CondNode = CondNode {
    op: op::TRUE,
    param: 0,
    children: 0,
    trigger: false,
    end: 0,
    value: 0.0,
};

const NO_EDGE: Edge = Edge {
    from: NONE16,
    to: 0,
    inertial: false,
    blend: 0.0,
    cond: 0,
};

const NO_POINT: Point = Point {
    x: 0.0,
    y: 0.0,
    clip: 0,
};

/// The table lengths a graph decodes to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableSizes {
    pub points: usize,
    pub triangles: usize,
    pub conds: usize,
}

impl<'s> GraphTables<'s> {
    /// What [`decode_in`](Self::decode_in) takes of an arena for `g`, its
    /// alignment slack included.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn bytes_for(g: &AnimGraph) -> usize {
        use core::mem::size_of;
        let n = Self::sizes(g);
        let slices = [
            g.param_count() * size_of::<(ParamId, ParamKind, f32)>(),
            g.clip_count() * size_of::<ClipRec>(),
            g.state_count() * size_of::<GraphState>(),
            n.points * size_of::<Point>(),
            n.triangles * 3,
            g.transition_count() * size_of::<Edge>(),
            n.conds * size_of::<CondNode>(),
            g.layer_count() * size_of::<GraphLayer>(),
        ];
        // Each slice may start up to an alignment past the last one's end.
        slices.iter().map(|b| b + 8).sum()
    }

    /// The point, triangle and condition-node counts of `g`.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn sizes(g: &AnimGraph) -> TableSizes {
        let mut n = TableSizes::default();
        for i in 0..g.state_count() {
            let s = g.state(i);
            n.points += s.points;
            n.triangles += s.triangles;
        }
        for t in 0..g.transition_count() {
            n.conds += count_nodes(g, g.transition(t).3, 0);
        }
        n
    }

    /// Decode `g` (parsed, so valid) into tables in `arena`:
    /// [`bytes_for`](Self::bytes_for) bytes of it. `None` when the arena
    /// is too small. The tables borrow the arena (and the blob, for the
    /// clips' tracks).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn decode_in<'b: 's>(
        g: &AnimGraph<'b>,
        arena: &'s crate::mem::Arena<'_>,
    ) -> Option<GraphTables<'s>> {
        let n = Self::sizes(g);
        let params = arena.alloc_slice(g.param_count(), (ParamId(0), ParamKind::Float, 0.0))?;
        let clips = arena.alloc_slice(g.clip_count(), NO_CLIP)?;
        let states = arena.alloc_slice(g.state_count(), NO_STATE)?;
        let points = arena.alloc_slice(n.points, NO_POINT)?;
        let triangles = arena.alloc_slice(n.triangles, [0u8; 3])?;
        let edges = arena.alloc_slice(g.transition_count(), NO_EDGE)?;
        let conds = arena.alloc_slice(n.conds, NO_COND)?;
        let layers = arena.alloc_slice(g.layer_count(), NO_LAYER)?;
        for (i, p) in params.iter_mut().enumerate() {
            *p = g.param_info(i);
        }
        for (i, r) in clips.iter_mut().enumerate() {
            let c = g.clip(i);
            *r = ClipRec {
                id: c.id,
                duration: c.duration,
                looping: c.looping,
                events: c.events,
                root: c.root.unwrap_or(RootTrack::EMPTY),
                feet: c.feet.unwrap_or(FeetTrack::EMPTY),
            };
        }
        let (mut np, mut nt) = (0usize, 0usize);
        for (i, out) in states.iter_mut().enumerate() {
            let mut s = g.state(i);
            for p in 0..s.points {
                let (x, y, clip) = if s.param_y.is_some() {
                    g.point2(&s, p)
                } else {
                    let (x, clip) = g.point(&s, p);
                    (x, 0.0, clip)
                };
                points[np + p] = Point {
                    x,
                    y,
                    clip: clip as u16,
                };
            }
            for t in 0..s.triangles {
                triangles[nt + t] = g.triangle(&s, t).map(|k| k as u8);
            }
            s.points_at = np;
            s.tris_at = nt;
            np += s.points;
            nt += s.triangles;
            *out = s;
        }
        let mut nc = 0usize;
        for (t, e) in edges.iter_mut().enumerate() {
            let (from, to, blend, cond) = g.transition(t);
            let at = nc;
            nc = flatten(g, cond, conds, nc).0;
            *e = Edge {
                from: from.map_or(NONE16, |f| f as u16),
                to: to as u16,
                inertial: g.transition_inertial(t),
                blend,
                cond: at as u16,
            };
        }
        for (l, out) in layers.iter_mut().enumerate() {
            *out = g.layer(l + 1);
        }
        Some(GraphTables {
            model: g.model(),
            params,
            clips,
            states,
            points,
            triangles,
            edges,
            conds,
            layers,
            start: g.start() as u16,
            base_states: g.states_of(0).1 as u16,
            base_transitions: g.transitions_of(0).1 as u16,
            triggers: GraphSource::triggers(g),
        })
    }
}

/// Nodes the condition at byte `at` flattens to.
fn count_nodes(g: &AnimGraph, at: usize, depth: usize) -> usize {
    let c = g.cond_code(at);
    if depth > super::graph::MAX_COND_DEPTH {
        return 1;
    }
    let mut n = 1;
    let mut child = at + c.len;
    for _ in 0..c.children {
        n += count_nodes(g, child, depth + 1);
        child += g.cond_len(child);
    }
    n
}

/// Flatten the condition at byte `at` into `nodes` from `i`: (the next
/// free node, whether it reads a trigger outside a `Not`).
fn flatten(g: &AnimGraph, at: usize, nodes: &mut [CondNode], i: usize) -> (usize, bool) {
    if i >= nodes.len() {
        return (i, false);
    }
    let c = g.cond_code(at);
    let mut node = CondNode {
        op: c.op,
        param: c.param as u8,
        children: c.children,
        trigger: c.op == op::TRIGGER,
        end: 0,
        value: c.value,
    };
    let mut next = i + 1;
    let mut child = at + c.len;
    for _ in 0..c.children {
        let (n, trig) = flatten(g, child, nodes, next);
        // Under a Not a trigger is never consumed.
        if c.op != op::NOT {
            node.trigger |= trig;
        }
        child += g.cond_len(child);
        next = n;
    }
    node.end = next as u16;
    nodes[i] = node;
    (next, node.trigger)
}

/// The graph an instance plays: a blob or its tables. Instances take
/// `impl Into<GraphRef>` (`&AnimGraph` and `&GraphTables` both convert),
/// so the playback is one copy of code (on a console, code is RAM), with a
/// branch per read instead of a monomorphized copy per source and crate.
#[derive(Clone, Copy, Debug)]
pub enum GraphRef<'g> {
    Blob(&'g AnimGraph<'g>),
    Tables(&'g GraphTables<'g>),
}

impl<'g, 'b: 'g> From<&'g AnimGraph<'b>> for GraphRef<'g> {
    fn from(g: &'g AnimGraph<'b>) -> Self {
        GraphRef::Blob(g)
    }
}

impl<'g, 'b: 'g> From<&'g GraphTables<'b>> for GraphRef<'g> {
    fn from(t: &'g GraphTables<'b>) -> Self {
        GraphRef::Tables(t)
    }
}

macro_rules! either {
    ($self:ident, $g:ident => $e:expr) => {
        match $self {
            GraphRef::Blob($g) => $e,
            GraphRef::Tables($g) => $e,
        }
    };
}

impl GraphSource for GraphRef<'_> {
    #[inline]
    fn model(&self) -> u32 {
        either!(self, g => g.model())
    }
    #[inline]
    fn param_count(&self) -> usize {
        either!(self, g => GraphSource::param_count(*g))
    }
    #[inline]
    fn param_info(&self, i: usize) -> (ParamId, ParamKind, f32) {
        either!(self, g => GraphSource::param_info(*g, i))
    }
    #[inline]
    fn state_count(&self) -> usize {
        either!(self, g => GraphSource::state_count(*g))
    }
    #[inline]
    fn clip_count(&self) -> usize {
        either!(self, g => GraphSource::clip_count(*g))
    }
    #[inline]
    fn start(&self) -> usize {
        either!(self, g => GraphSource::start(*g))
    }
    #[inline]
    fn layer_count(&self) -> usize {
        either!(self, g => GraphSource::layer_count(*g))
    }
    #[inline]
    fn layer(&self, l: usize) -> GraphLayer {
        either!(self, g => GraphSource::layer(*g, l))
    }
    #[inline]
    fn states_of(&self, l: usize) -> (usize, usize) {
        either!(self, g => GraphSource::states_of(*g, l))
    }
    #[inline]
    fn transitions_of(&self, l: usize) -> (usize, usize) {
        either!(self, g => GraphSource::transitions_of(*g, l))
    }
    #[inline]
    fn state(&self, i: usize) -> GraphState<'_> {
        either!(self, g => GraphSource::state(*g, i))
    }
    #[inline]
    fn clip(&self, i: usize) -> GraphClip<'_> {
        either!(self, g => GraphSource::clip(*g, i))
    }
    #[inline]
    fn clip_duration(&self, i: usize) -> f32 {
        either!(self, g => GraphSource::clip_duration(*g, i))
    }
    #[inline]
    fn clip_looping(&self, i: usize) -> bool {
        either!(self, g => GraphSource::clip_looping(*g, i))
    }
    #[inline]
    fn point(&self, s: &GraphState, p: usize) -> (f32, usize) {
        either!(self, g => GraphSource::point(*g, s, p))
    }
    #[inline]
    fn point2(&self, s: &GraphState, p: usize) -> (f32, f32, usize) {
        either!(self, g => GraphSource::point2(*g, s, p))
    }
    #[inline]
    fn triangle(&self, s: &GraphState, t: usize) -> [usize; 3] {
        either!(self, g => GraphSource::triangle(*g, s, t))
    }
    #[inline]
    fn edge(&self, t: usize) -> Edge {
        either!(self, g => GraphSource::edge(*g, t))
    }
    #[inline]
    fn holds(
        &self,
        t: usize,
        params: &[f32; MAX_PARAMS],
        finished: bool,
        elapsed: f32,
        used: &mut u16,
    ) -> bool {
        either!(self, g => GraphSource::holds(*g, t, params, finished, elapsed, used))
    }
    #[inline]
    fn triggers(&self) -> u16 {
        either!(self, g => GraphSource::triggers(*g))
    }
}
