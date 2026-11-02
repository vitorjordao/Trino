//! Playing an animation graph: [`GraphInstance`] (issue #65; layers and
//! inertia #112; sized by the game, tables, sync and seek #138).
//!
//! One instance holds the parameters, each machine's current state (and
//! the one it fades from), the inertialized offsets and this update's
//! events and root motion. Its capacity is the game's choice:
//!
//! - [`AnimGraphInstance`]: every layer a graph may have, an inertia slot
//!   each, an event buffer (what the editor, the tools and one hero use);
//! - [`GraphInstance<L, I, E>`]: `L` machines (the base and `L - 1`
//!   layers), `I` inertia slots (layer `l` has one when `l < I`; without,
//!   an inertialized transition crossfades over the same time) and `E` = 0
//!   or 1 event buffer. A crowd member of a graph with two layers and no
//!   events is `GraphInstance<3, 0, 0>`.
//!
//! The update itself is not generic over the capacity, nor over the
//! source: every size runs the same code over slices, reading a
//! [`GraphRef`] (a blob or its tables) — one copy in a console's RAM, where
//! code is RAM. The per-update path (advance, shape, transitions, pose) is
//! compiled for speed on the consoles, the rest for size.

use super::clip::{EventBuf, RootDelta, floor};
use super::graph::{
    Fnv, GraphLayer, GraphState, MAX_CONTRIBUTIONS, MAX_LAYERS, MAX_PARAMS, NONE16, ParamKind,
};
use super::tables::{GraphRef, GraphSource};

type Gr<'g> = GraphRef<'g>;
use crate::render::{AnimBlend, AnimRef};

/// A clip's share of the pose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contribution {
    pub clip: usize,
    /// Its time in seconds (what [`AnimRef`] samples).
    pub time: f32,
    pub weight: f32,
}

impl Contribution {
    pub const NONE: Contribution = Contribution {
        clip: 0,
        time: 0.0,
        weight: 0.0,
    };
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
    const fn new(state: usize) -> Playing {
        Playing {
            state: state as u16,
            phase: 0.0,
            elapsed: 0.0,
            cycles: 0,
            first: true,
        }
    }
}

/// One machine being played: the current state and the one it fades from
/// (`fading`; its floats are always initialized: no float behind an
/// `Option` on the N64).
#[derive(Clone, Copy, Debug, PartialEq)]
struct LayerPlay {
    cur: Playing,
    prev: Playing,
    fading: bool,
    fade: f32,
    fade_len: f32,
}

impl LayerPlay {
    const fn new(state: usize) -> LayerPlay {
        LayerPlay {
            cur: Playing::new(state),
            prev: Playing::new(state),
            fading: false,
            fade: 0.0,
            fade_len: 0.0,
        }
    }

    fn fade_k(&self) -> f32 {
        if self.fade_len > 0.0 {
            (self.fade / self.fade_len).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    fn prev(&self) -> Option<&Playing> {
        self.fading.then_some(&self.prev)
    }
}

/// A clip's share of a frozen pose (an inertia snapshot).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Snap {
    clip: u16,
    time: f32,
    weight: f32,
}

impl Snap {
    const NONE: Snap = Snap {
        clip: 0,
        time: 0.0,
        weight: 0.0,
    };
}

/// An inertialized transition (v2): where the layer's pose was when it
/// fired (`From`, at most three clips) and where its destination started
/// (`To`), frozen; the pose adds their difference, decaying over `len`.
/// `on` is false when none runs.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Inertia {
    snaps: [Snap; 6],
    from: u8,
    to: u8,
    on: bool,
    elapsed: f32,
    len: f32,
}

impl Inertia {
    const NONE: Inertia = Inertia {
        snaps: [Snap::NONE; 6],
        from: 0,
        to: 0,
        on: false,
        elapsed: 0.0,
        len: 0.0,
    };
}

/// A state's clips now: up to three (a 2D space's triangle), their
/// weights (summing to 1) and the cycle length in seconds.
#[derive(Clone, Copy, Debug)]
struct Shape {
    clip: [usize; 3],
    w: [f32; 3],
    n: usize,
    dur: f32,
}

/// No events (an instance without an event buffer).
static NO_EVENTS: EventBuf = EventBuf::new();

/// A graph being played, with room for `L` machines (the base and `L - 1`
/// layers), `I` inertia slots and `E` (0 or 1) event buffers. `Copy`,
/// fixed size, no allocation; the same updates give the same poses on
/// every target. See the [module](self) for the sizes to pick.
#[derive(Clone, Copy, Debug)]
pub struct GraphInstance<const L: usize, const I: usize, const E: usize> {
    params: [f32; MAX_PARAMS],
    root: RootDelta,
    /// Transitions taken since the instance started (a test reads it).
    pub transitions: u32,
    /// Layers over the base this instance plays.
    layers: u8,
    /// The base (0) and each layer's machine.
    play: [LayerPlay; L],
    inertia: [Inertia; I],
    events: [EventBuf; E],
}

/// Every layer a graph may have, an inertia slot each, events: the
/// instance the editor, the tools and a game's hero use.
pub type AnimGraphInstance = GraphInstance<{ MAX_LAYERS + 1 }, { MAX_LAYERS + 1 }, 1>;

/// The mutable state an update works on (one code for every size).
struct Play<'s> {
    params: &'s mut [f32; MAX_PARAMS],
    root: &'s mut RootDelta,
    transitions: &'s mut u32,
    layers: usize,
    play: &'s mut [LayerPlay],
    inertia: &'s mut [Inertia],
    events: Option<&'s mut EventBuf>,
}

/// What the pose and the digest read.
#[derive(Clone, Copy)]
struct View<'s> {
    params: &'s [f32; MAX_PARAMS],
    transitions: u32,
    layers: usize,
    play: &'s [LayerPlay],
    inertia: &'s [Inertia],
}

impl<const L: usize, const I: usize, const E: usize> GraphInstance<L, I, E> {
    const ROOM: () = assert!(L >= 1 && E <= 1, "a GraphInstance needs L >= 1 and E <= 1");

    /// An instance at the graph's start state (each layer at its own), its
    /// parameters at their defaults. A graph with more layers than the
    /// instance has room for plays the first `L - 1`.
    pub fn new<'g>(graph: impl Into<GraphRef<'g>>) -> Self {
        #[allow(clippy::let_unit_value)]
        let () = Self::ROOM;
        let mut i = GraphInstance {
            params: [0.0; MAX_PARAMS],
            root: RootDelta::ZERO,
            transitions: 0,
            layers: 0,
            play: [LayerPlay::new(0); L],
            inertia: [Inertia::NONE; I],
            events: [EventBuf::new(); E],
        };
        i.layers = init(&mut i.params, &mut i.play, graph.into()) as u8;
        i
    }

    /// An instance starting in state `state` (an index, or the start when
    /// out of range): several entities sharing one graph, each mid-action.
    pub fn new_in<'g>(graph: impl Into<GraphRef<'g>>, state: usize) -> Self {
        let graph = graph.into();
        let mut i = Self::new(graph);
        if state < graph.states_of(0).1 {
            i.play[0].cur = Playing::new(state);
        }
        i
    }

    fn view_mut(&mut self) -> Play<'_> {
        Play {
            params: &mut self.params,
            root: &mut self.root,
            transitions: &mut self.transitions,
            layers: self.layers as usize,
            play: &mut self.play,
            inertia: &mut self.inertia,
            events: self.events.first_mut(),
        }
    }

    fn view(&self) -> View<'_> {
        View {
            params: &self.params,
            transitions: self.transitions,
            layers: self.layers as usize,
            play: &self.play,
            inertia: &self.inertia,
        }
    }

    /// Set a float parameter (by index, [`GraphSource::param`]).
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
    pub fn set<'g>(&mut self, graph: impl Into<GraphRef<'g>>, name: &str, v: f32) -> bool {
        match graph.into().param_named(name) {
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

    /// The base layer's current state (the one being faded to,
    /// mid-transition).
    pub fn state(&self) -> usize {
        self.play[0].cur.state as usize
    }

    /// Layer `l`'s current state (0 = the base).
    pub fn layer_state(&self, l: usize) -> usize {
        self.play.get(l).map_or(0, |p| p.cur.state as usize)
    }

    /// Layers over the base this instance plays.
    pub fn layer_count(&self) -> usize {
        self.layers as usize
    }

    /// The base mid-transition: (from state, 0..1 progress).
    pub fn transition(&self) -> Option<(usize, f32)> {
        self.layer_transition(0)
    }

    /// Layer `l` mid-crossfade: (from state, 0..1 progress).
    pub fn layer_transition(&self, l: usize) -> Option<(usize, f32)> {
        let p = self.play.get(l)?;
        p.prev().map(|x| (x.state as usize, p.fade_k()))
    }

    /// Layer `l`'s inertialized transition: how much of its offset is left
    /// (1 → 0), `None` when none runs.
    pub fn inertia(&self, l: usize) -> Option<f32> {
        self.view().inertia(l)
    }

    /// Seconds in the current state.
    pub fn elapsed(&self) -> f32 {
        self.play[0].cur.elapsed
    }

    /// The current state's clock: a clip state's seconds, a blend space's
    /// cycle (0..1).
    pub fn clock(&self) -> f32 {
        self.play[0].cur.phase
    }

    /// Layer `l`'s clock (0 = the base), as [`clock`](Self::clock).
    pub fn layer_clock(&self, l: usize) -> f32 {
        self.play.get(l).map_or(0.0, |p| p.cur.phase)
    }

    /// Move layer `l`'s current state to `clock` (its seconds for a clip
    /// state, its cycle for a blend space): a layer a game keeps in step
    /// with something of its own (issue #138; a graph's `sync` keeps one
    /// with the base). Fires nothing; the next update fires what it
    /// crosses from there. Out of range: nothing happens.
    pub fn seek_layer(&mut self, l: usize, clock: f32) {
        if l <= self.layers as usize
            && let Some(p) = self.play.get_mut(l)
            && clock.is_finite()
        {
            p.cur.phase = clock;
            p.cur.first = false;
        }
    }

    /// The events the last update crossed, in timeline order (none for an
    /// instance without an event buffer).
    pub fn events(&self) -> &EventBuf {
        self.events.first().unwrap_or(&NO_EVENTS)
    }

    /// The root motion of the last update (model units, in the
    /// character's frame at its start): move the entity by it. Only the
    /// base layer moves the root.
    pub fn root_motion(&self) -> RootDelta {
        self.root
    }

    /// Force a state (a respawn, a cutscene), with an optional crossfade.
    pub fn play(&mut self, state: usize, blend: f32) {
        let mut v = self.view_mut();
        v.crossfade(0, state, blend);
        *v.transitions = v.transitions.wrapping_add(1);
    }

    /// One step: advance the clocks by `dt` seconds, collect events and
    /// root motion, then take the first transition whose condition holds
    /// (in the graph's order; transitions from any state skip the state
    /// they lead to), the base first, then each layer. Triggers nothing
    /// consumed are cleared.
    pub fn update<'g>(&mut self, graph: impl Into<GraphRef<'g>>, dt: f32) {
        update(&mut self.view_mut(), graph.into(), dt);
    }

    /// Each clip's share of the base layer's pose (two states of up to
    /// three clips), fading-from state first; weights sum to 1.
    pub fn contributions<'g>(
        &self,
        graph: impl Into<GraphRef<'g>>,
        out: &mut [Contribution],
    ) -> usize {
        self.view().layer_contributions(graph.into(), 0, out)
    }

    /// [`contributions`](Self::contributions) of layer `l`.
    pub fn layer_contributions<'g>(
        &self,
        graph: impl Into<GraphRef<'g>>,
        l: usize,
        out: &mut [Contribution],
    ) -> usize {
        self.view().layer_contributions(graph.into(), l, out)
    }

    /// Where the model's IK feet are in this pose (model space; the
    /// clips' feet tracks blended by their weights): where a game casts
    /// for the ground ([`crate::anim::FootIk::pose`]). `None` for a model
    /// without IK chains.
    pub fn feet<'g>(&self, graph: impl Into<GraphRef<'g>>) -> Option<[crate::math::Vec3; 2]> {
        feet(self.view(), graph.into())
    }

    /// The pose to draw as an [`AnimRef`]: up to three clips (the lightest
    /// dropped and the rest renormalized past three — only while a blend
    /// space fades into another). A graph with layers or an inertialized
    /// transition gives [`pose`](Self::pose) folded to the base N64's tier.
    pub fn anim<'g>(&self, graph: impl Into<GraphRef<'g>>) -> AnimRef {
        anim(self.view(), graph.into())
    }

    /// The whole pose (issue #112): every layer's clips with its mask,
    /// mode and weight (a layer at weight 0 is left out), inertialized
    /// offsets with their decay. Hand it to
    /// [`Renderer::push_pose`](crate::Renderer::push_pose), which folds it
    /// to the platform's tier.
    pub fn pose<'g>(&self, graph: impl Into<GraphRef<'g>>) -> crate::anim::AnimPose {
        pose(self.view(), graph.into())
    }

    /// A digest of the playback state (state, clocks quantized to 1/4096
    /// of a cycle, fade, parameters; layers and inertia when the graph has
    /// them): the self-test compares it across targets. Instances of any
    /// size that played the same give the same digest.
    pub fn digest(&self) -> u32 {
        self.view().digest()
    }
}

/// Parameters at their defaults, each machine at its start state: the
/// layers played (at most `play.len() - 1`).
#[inline(never)]
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn init(params: &mut [f32; MAX_PARAMS], play: &mut [LayerPlay], graph: Gr) -> usize {
    for (i, p) in params.iter_mut().enumerate().take(graph.param_count()) {
        let (_, kind, default) = graph.param_info(i);
        *p = if kind == ParamKind::Trigger {
            0.0
        } else {
            default
        };
    }
    let layers = graph.layer_count().min(MAX_LAYERS).min(play.len() - 1);
    for (l, p) in play.iter_mut().enumerate() {
        *p = LayerPlay::new(if l == 0 {
            graph.start()
        } else if l <= layers {
            graph.layer(l).start
        } else {
            graph.start()
        });
    }
    layers
}

/// A layer's weight now: its constant times its parameter (0..1).
fn layer_weight(params: &[f32; MAX_PARAMS], g: &GraphLayer) -> f32 {
    let p = g.weight_param.map_or(1.0, |i| params[i].clamp(0.0, 1.0));
    g.weight * p
}

/// A blend space's two clips and the weight of the second.
fn blend_pair(graph: Gr, s: &GraphState, v: f32) -> (usize, usize, f32) {
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

/// A 2D blend space's clips at `(x, y)`: the barycentric weights of the
/// baked triangle holding it, else of the nearest point on the triangles'
/// edges (outside the hull). Continuous across edges.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn blend_tri(graph: Gr, s: &GraphState, x: f32, y: f32) -> ([usize; 3], [f32; 3], usize) {
    let pt = |i: usize| graph.point2(s, i);
    if s.triangles == 0 {
        return ([pt(0).2, 0, 0], [1.0, 0.0, 0.0], 1);
    }
    let mut best = (f32::MAX, [0usize; 3], [0.0f32; 3]);
    for t in 0..s.triangles {
        let [i, j, k] = graph.triangle(s, t);
        let (a, b, c) = (pt(i), pt(j), pt(k));
        let (v0x, v0y, v1x, v1y) = (b.0 - a.0, b.1 - a.1, c.0 - a.0, c.1 - a.1);
        let (v2x, v2y) = (x - a.0, y - a.1);
        let den = v0x * v1y - v1x * v0y;
        if den.abs() > 1e-12 {
            let v = (v2x * v1y - v1x * v2y) / den;
            let w = (v0x * v2y - v2x * v0y) / den;
            let u = 1.0 - v - w;
            if u >= -1e-6 && v >= -1e-6 && w >= -1e-6 {
                let (u, v, w) = (u.max(0.0), v.max(0.0), w.max(0.0));
                let sum = u + v + w;
                return ([a.2, b.2, c.2], [u / sum, v / sum, w / sum], 3);
            }
        }
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let (ex, ey) = (q.0 - p.0, q.1 - p.1);
            let len2 = ex * ex + ey * ey;
            let k = if len2 > 0.0 {
                (((x - p.0) * ex + (y - p.1) * ey) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (dx, dy) = (x - (p.0 + ex * k), y - (p.1 + ey * k));
            let d = dx * dx + dy * dy;
            if d < best.0 {
                best = (d, [p.2, q.2, 0], [1.0 - k, k, 0.0]);
            }
        }
    }
    (best.1, best.2, 2)
}

/// A state's clips now, their weights and its cycle length in seconds.
fn shape(params: &[f32; MAX_PARAMS], graph: Gr, s: &GraphState) -> Shape {
    let dur = |c: usize| graph.clip_duration(c);
    if let (Some(px), Some(py)) = (s.param, s.param_y) {
        let (mut clip, mut w, mut n) = blend_tri(graph, s, params[px], params[py]);
        // One share per clip (two points may play the same clip).
        let mut i = 0;
        while i < n {
            let mut j = i + 1;
            while j < n {
                if clip[j] == clip[i] {
                    w[i] += w[j];
                    clip[j] = clip[n - 1];
                    w[j] = w[n - 1];
                    n -= 1;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
        let mut d = 0.0;
        for k in 0..n {
            d += dur(clip[k]) * w[k];
        }
        return Shape { clip, w, n, dur: d };
    }
    let v = s.param.map_or(0.0, |p| params[p]);
    let (a, b, w) = blend_pair(graph, s, v);
    let (da, db) = (dur(a), dur(b));
    let n = if a == b { 1 } else { 2 };
    Shape {
        clip: [a, b, b],
        w: if n == 1 {
            [1.0, 0.0, 0.0]
        } else {
            [1.0 - w, w, 0.0]
        },
        n,
        dur: da + (db - da) * w,
    }
}

fn looping(graph: Gr, s: &GraphState, clip: usize) -> bool {
    match s.looping {
        Some(l) => l,
        None => graph.clip_looping(clip),
    }
}

/// A clip's time in seconds at a state's clock.
fn clip_time(graph: Gr, s: &GraphState, clip: usize, clock: f32) -> f32 {
    let duration = graph.clip_duration(clip);
    if s.param.is_none() {
        // The seconds as played: the pose wraps a looping clip itself.
        match s.looping {
            Some(true) if !graph.clip_looping(clip) && duration > 0.0 => {
                clock - floor(clock / duration) * duration
            }
            Some(false) if graph.clip_looping(clip) => clock.min(duration),
            _ => clock,
        }
    } else if looping(graph, s, clip) {
        (clock - floor(clock)) * duration
    } else {
        clock.clamp(0.0, 1.0) * duration
    }
}

/// Where a playing state is in its cycle (0..1), for a layer synced to
/// it: `None` unless it loops over a positive length.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn cycle_of(params: &[f32; MAX_PARAMS], graph: Gr, p: &Playing) -> Option<f32> {
    let s = graph.state(p.state as usize);
    let sh = shape(params, graph, &s);
    if !looping(graph, &s, sh.clip[0]) || sh.dur <= 0.0 {
        return None;
    }
    Some(if s.param.is_none() {
        let c = p.phase / sh.dur;
        c - floor(c)
    } else {
        p.phase - floor(p.phase)
    })
}

impl Play<'_> {
    /// Advance one playing state by `dt` (or, `sync`, to that fraction of
    /// its cycle, forward); fire its events when `fire` (and there is a
    /// buffer), and return its root motion.
    #[allow(clippy::too_many_arguments)]
    fn advance(
        &mut self,
        graph: Gr,
        p: &mut Playing,
        dt: f32,
        fire: bool,
        weight: f32,
        sync: Option<f32>,
    ) -> RootDelta {
        let s = graph.state(p.state as usize);
        let sh = shape(self.params, graph, &s);
        let dur = sh.dur;
        let rate = s.speed * s.speed_param.map_or(1.0, |i| self.params[i]);
        let loops = looping(graph, &s, sh.clip[0]);
        p.elapsed += dt;
        let mut step = if dt * rate > 0.0 { dt * rate } else { 0.0 };
        // The clock: a clip state's seconds (unwrapped, like a game's own
        // `time` it replaces: the same additions give the same pose), a
        // blend space's cycles.
        let c0 = p.phase;
        if let Some(f) = sync
            && loops
            && dur > 0.0
        {
            // Forward to the base's fraction of a cycle (issue #138).
            let at = if s.param.is_none() {
                let c = c0 / dur;
                c - floor(c)
            } else {
                c0 - floor(c0)
            };
            let d = f - at;
            step = (if d < 0.0 { d + 1.0 } else { d }) * dur;
        }
        let (c1, unit) = if s.param.is_none() {
            // Not clamped even for a one-shot clip: the pose clamps it, and
            // the time stays the game's own (events and root motion clamp).
            (c0 + step, dur)
        } else {
            let mut ph = if dur > 0.0 { c0 + step / dur } else { 1.0 };
            if !loops {
                ph = ph.min(1.0);
            }
            (ph, 1.0)
        };
        let mut root = RootDelta::ZERO;
        // Each clip of the shape: its share of root motion; the heaviest
        // one fires its events (a walk and a run blended don't both step).
        let mut heavy = 0;
        for k in 1..sh.n {
            if sh.w[k] > sh.w[heavy] {
                heavy = k;
            }
        }
        for k in 0..sh.n {
            let (clip, wk) = (sh.clip[k], sh.w[k]);
            if wk <= 0.0 {
                continue;
            }
            let duration = graph.clip_duration(clip);
            let lp = looping(graph, &s, clip);
            let (t0, t1) = if s.param.is_none() {
                (c0, c1)
            } else {
                (c0 * duration, c1 * duration)
            };
            // The clip's tracks only where they are read.
            if s.root_motion
                && let Some(track) = graph.clip(clip).root
            {
                root = root.then(track.delta(t0, t1, duration, lp).scaled(wk));
            }
            if fire
                && k == heavy
                && let Some(events) = self.events.as_deref_mut()
            {
                let c = graph.clip(clip);
                let ew = weight * wk;
                c.events
                    .fire(t0, t1, p.first, c.duration, lp, c.id, ew, events);
                if s.param.is_none() {
                    s.events
                        .fire(t0, t1, p.first, c.duration, lp, c.id, ew, events);
                }
            }
        }
        // Cycles done (`Finished`).
        if unit <= 0.0 {
            p.cycles = p.cycles.max(1);
        } else if loops {
            let done = floor(c1 / unit) - floor(c0 / unit);
            p.cycles = p.cycles.saturating_add(done as u16);
        } else if c1 >= unit && c0 < unit {
            p.cycles = p.cycles.saturating_add(1);
        }
        // A blend space's clock keeps only its fraction (its clips' times
        // come from it); a clip state's stays the seconds it played.
        p.phase = if s.param.is_some() && loops {
            c1 - floor(c1)
        } else {
            c1
        };
        p.first = false;
        root
    }

    /// Advance layer `l`'s clocks; fire its events (scaled by the layer's
    /// weight `lw`); its root motion.
    fn advance_layer(
        &mut self,
        graph: Gr,
        l: usize,
        dt: f32,
        lw: f32,
        sync: Option<f32>,
    ) -> RootDelta {
        let mut lp = self.play[l];
        let k = if lp.fading {
            lp.fade += dt;
            lp.fade_k()
        } else {
            1.0
        };
        let fire = lw > 0.0;
        let mut cur = lp.cur;
        let mut root = self.advance(graph, &mut cur, dt, fire, k * lw, sync);
        lp.cur = cur;
        if lp.fading {
            let mut prev = lp.prev;
            let from = self.advance(graph, &mut prev, dt, false, 1.0 - k, None);
            root = root.scaled(k).then(from.scaled(1.0 - k));
            lp.prev = prev;
            lp.fading = k < 1.0;
        }
        self.play[l] = lp;
        if let Some(i) = self.inertia.get_mut(l)
            && i.on
        {
            i.elapsed += dt;
            if i.elapsed >= i.len {
                *i = Inertia::NONE;
            }
        }
        root
    }

    /// A plain crossfade (or a cut) of layer `l` into state `to`.
    fn crossfade(&mut self, l: usize, to: usize, blend: f32) {
        let p = &mut self.play[l];
        if blend > 0.0 {
            p.prev = p.cur;
            p.fading = true;
        } else {
            p.fading = false;
        }
        p.cur = Playing::new(to);
        p.fade = 0.0;
        p.fade_len = blend;
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn start_transition(&mut self, graph: Gr, l: usize, to: usize, blend: f32, inertial: bool) {
        if inertial && blend > 0.0 && l < self.inertia.len() {
            let mut snaps = [Snap::NONE; 6];
            let mut from = [Contribution::NONE; MAX_CONTRIBUTIONS];
            let n = self.view().layer_contributions(graph, l, &mut from);
            // The three heaviest, renormalized.
            let mut keep = n.min(3);
            let mut total = 0.0;
            for k in 0..keep {
                let mut best = k;
                for j in k + 1..n {
                    if from[j].weight > from[best].weight {
                        best = j;
                    }
                }
                from.swap(k, best);
                total += from[k].weight;
            }
            if total <= 0.0 {
                keep = 0;
            }
            for k in 0..keep {
                snaps[k] = Snap {
                    clip: from[k].clip as u16,
                    time: from[k].time,
                    weight: from[k].weight / total,
                };
            }
            let p = &mut self.play[l];
            p.fading = false;
            p.cur = Playing::new(to);
            p.fade = 0.0;
            p.fade_len = 0.0;
            let mut dest = [Contribution::NONE; MAX_CONTRIBUTIONS];
            let m = self.view().layer_contributions(graph, l, &mut dest).min(3);
            for k in 0..m {
                snaps[keep + k] = Snap {
                    clip: dest[k].clip as u16,
                    time: dest[k].time,
                    weight: dest[k].weight,
                };
            }
            self.inertia[l] = if keep > 0 && m > 0 {
                Inertia {
                    snaps,
                    from: keep as u8,
                    to: m as u8,
                    on: true,
                    elapsed: 0.0,
                    len: blend,
                }
            } else {
                Inertia::NONE
            };
        } else {
            // An inertialized transition without a slot crossfades.
            self.crossfade(l, to, blend);
        }
        *self.transitions = self.transitions.wrapping_add(1);
    }

    fn view(&self) -> View<'_> {
        View {
            params: self.params,
            transitions: *self.transitions,
            layers: self.layers,
            play: self.play,
            inertia: self.inertia,
        }
    }
}

#[inline(never)]
fn update(v: &mut Play, graph: Gr, dt: f32) {
    if let Some(e) = v.events.as_deref_mut() {
        e.clear();
    }
    let dt = if dt > 0.0 { dt } else { 0.0 };
    let layers = v.layers.min(graph.layer_count());
    *v.root = v.advance_layer(graph, 0, dt, 1.0, None);
    // The base's place in its cycle, for the layers synced to it.
    let mut base = None;
    for l in 1..=layers {
        let g = graph.layer(l);
        let sync = if g.sync {
            if base.is_none() {
                base = Some(cycle_of(v.params, graph, &v.play[0].cur));
            }
            base.flatten()
        } else {
            None
        };
        let lw = layer_weight(v.params, &g);
        v.advance_layer(graph, l, dt, lw, sync);
    }

    // Transitions, evaluated after the clocks moved.
    for l in 0..=layers {
        let cur = v.play[l].cur;
        let mut fired = None;
        let (lo, hi) = graph.transitions_of(l);
        for t in lo..hi {
            let e = graph.edge(t);
            let to = e.to as usize;
            if e.from != NONE16 {
                if e.from != cur.state {
                    continue;
                }
            } else if to == cur.state as usize {
                continue;
            }
            let mut used = 0u16;
            if graph.holds(t, v.params, cur.cycles > 0, cur.elapsed, &mut used) {
                fired = Some((to, e.blend, used, e.inertial));
                break;
            }
        }
        if let Some((to, blend, used, inertial)) = fired {
            for (i, p) in v.params.iter_mut().enumerate() {
                if used & (1 << i) != 0 {
                    *p = 0.0;
                }
            }
            v.start_transition(graph, l, to, blend, inertial);
        }
    }
    // A trigger lives one update.
    let triggers = graph.triggers();
    if triggers != 0 {
        for (i, p) in v.params.iter_mut().enumerate() {
            if triggers & (1 << i) != 0 {
                *p = 0.0;
            }
        }
    }
}

impl View<'_> {
    fn inertia(&self, l: usize) -> Option<f32> {
        let i = self.inertia.get(l).filter(|i| i.on)?;
        let k = if i.len > 0.0 { i.elapsed / i.len } else { 1.0 };
        Some(crate::anim::inertial_decay(k))
    }

    /// Whether the pose needs more than an `AnimRef`'s model: layers or an
    /// inertialized transition.
    fn layered(&self) -> bool {
        self.layers > 0 || self.inertia.iter().any(|i| i.on)
    }

    #[inline(never)]
    fn layer_contributions(&self, graph: Gr, l: usize, out: &mut [Contribution]) -> usize {
        let Some(lp) = self.play.get(l) else {
            return 0;
        };
        let mut n = 0;
        let k = if lp.fading { lp.fade_k() } else { 1.0 };
        let mut push = |p: &Playing, scale: f32| {
            let s = graph.state(p.state as usize);
            let sh = shape(self.params, graph, &s);
            for i in 0..sh.n {
                let weight = sh.w[i] * scale;
                if weight > 0.0 && n < out.len() {
                    out[n] = Contribution {
                        clip: sh.clip[i],
                        time: clip_time(graph, &s, sh.clip[i], p.phase),
                        weight,
                    };
                    n += 1;
                }
            }
        };
        if let Some(prev) = lp.prev() {
            push(prev, 1.0 - k);
        }
        push(&lp.cur, k);
        n
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    #[inline(never)]
    fn digest(&self) -> u32 {
        let mut h = Fnv::new();
        let q = |v: f32| crate::math3d::to_i32(v * 4096.0) as u32;
        let play = |h: &mut Fnv, lp: &LayerPlay| {
            h.u32(lp.cur.state as u32);
            h.u32(q(lp.cur.phase));
            h.u32(lp.cur.cycles as u32);
            match lp.prev() {
                Some(p) => {
                    h.u32(p.state as u32 + 1);
                    h.u32(q(p.phase));
                    h.u32(q(lp.fade_k()));
                }
                None => h.u32(0),
            }
        };
        play(&mut h, &self.play[0]);
        for p in self.params {
            h.u32(q(*p));
        }
        h.u32(self.transitions);
        if self.layered() {
            for lp in &self.play[1..=self.layers] {
                play(&mut h, lp);
            }
            // Every slot an `AnimGraphInstance` has (a smaller instance's
            // missing ones run nothing).
            for l in 0..=MAX_LAYERS {
                let i = self.inertia.get(l).filter(|i| i.on);
                h.u32(i.map_or(0, |i| q(i.elapsed) + 1));
            }
        }
        h.0
    }
}

#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
#[inline(never)]
fn feet(v: View, graph: Gr) -> Option<[crate::math::Vec3; 2]> {
    let mut c = [Contribution::NONE; MAX_CONTRIBUTIONS];
    let n = v.layer_contributions(graph, 0, &mut c);
    let mut out = [crate::math::Vec3::ZERO; 2];
    for x in &c[..n] {
        let clip = graph.clip(x.clip);
        let track = clip.feet?;
        // A clip state's time is its unwrapped clock.
        let t = if clip.looping && clip.duration > 0.0 {
            x.time - floor(x.time / clip.duration) * clip.duration
        } else {
            crate::math::clamp_or(x.time, 0.0, clip.duration)
        };
        let f = track.at(t);
        out[0] += f[0] * x.weight;
        out[1] += f[1] * x.weight;
    }
    (n > 0).then_some(out)
}

#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
#[inline(never)]
fn anim(v: View, graph: Gr) -> AnimRef {
    if v.layered() {
        return pose(v, graph).to_ref();
    }
    let mut c = [Contribution::NONE; MAX_CONTRIBUTIONS];
    let mut n = v.layer_contributions(graph, 0, &mut c);
    while n > 3 {
        let mut min = 0;
        for i in 1..n {
            if c[i].weight < c[min].weight {
                min = i;
            }
        }
        for i in min..n - 1 {
            c[i] = c[i + 1];
        }
        n -= 1;
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

#[inline(never)]
fn pose(v: View, graph: Gr) -> crate::anim::AnimPose {
    use crate::anim::{AnimPose, PoseLayer, SampleRole};
    let mut p = AnimPose::EMPTY;
    let layers = v.layers.min(graph.layer_count());
    for l in 0..=layers {
        let li = if l == 0 {
            0
        } else {
            let g = graph.layer(l);
            let w = layer_weight(v.params, &g);
            if w <= 0.0 {
                continue;
            }
            match p.add_layer(PoseLayer {
                mask: g.mask,
                mode: g.mode,
                weight: w.min(1.0),
                decay: 0.0,
            }) {
                Some(i) => i,
                None => break,
            }
        };
        let mut c = [Contribution::NONE; MAX_CONTRIBUTIONS];
        let n = v.layer_contributions(graph, l, &mut c);
        for x in &c[..n] {
            p.push(
                li,
                SampleRole::Pose,
                graph.clip(x.clip).id,
                x.time,
                x.weight,
            );
        }
        if let (Some(i), Some(decay)) = (v.inertia.get(l).filter(|i| i.on), v.inertia(l))
            && decay > 0.0
        {
            for (k, x) in i.snaps[..(i.from + i.to) as usize].iter().enumerate() {
                let role = if k < i.from as usize {
                    SampleRole::From
                } else {
                    SampleRole::To
                };
                p.push(li, role, graph.clip(x.clip as usize).id, x.time, x.weight);
            }
            p.layers[li as usize].decay = decay;
        }
    }
    p
}
