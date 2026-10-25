//! The engine's character controller (issue #66 phase 2): a body that walks,
//! jumps, climbs steps and slopes, rides moving platforms, passes up
//! through one-way platforms and slides down steep ground — castle64's
//! player, generalised. Generic over [`Scalar`] (`f32` or [`Fx`](super::Fx)).
//!
//! The body is a box (`half` extents; [`ControllerConfig::capsule`] maps a
//! capsule to its box) on every target: the N64-era model, deterministic,
//! identical on PC, N64, 3DS and the Web. It moves one axis at a time (X,
//! Z, then Y) with the same contact skin as [`move_aabb`](super::move_aabb),
//! and with every option off over plain boxes it **is** `move_aabb`: same
//! operations in the same order, the same bits (tested), so castle64's
//! trajectories, goldens and bot runs did not move when it adopted it.
//!
//! Geometry is a slice of [`Surface`]s (boxes, one-way boxes, ramps),
//! visited in index order like `move_aabb`'s solids, plus triangle meshes
//! through [`MeshQuery`] trait objects (`&[&mesh]`, or `&[]` without any:
//! one compiled controller whatever the mesh type, which the N64's RAM
//! notices). [`CharacterController::step_world`] gathers the surfaces from a
//! [`World`](super::World) grid first.

use super::scalar::{Box3, MeshQuery, Scalar, V3};
use super::{SKIN, Solid, World};
use crate::math3d;

/// The side a ramp's top rises toward.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum RampDir {
    PosX,
    NegX,
    PosZ,
    NegZ,
}

impl RampDir {
    /// (axis index, rises toward +axis).
    #[inline]
    fn axis(self) -> (usize, bool) {
        match self {
            RampDir::PosX => (0, true),
            RampDir::NegX => (0, false),
            RampDir::PosZ => (2, true),
            RampDir::NegZ => (2, false),
        }
    }

    pub const fn code(self) -> u8 {
        match self {
            RampDir::PosX => 0,
            RampDir::NegX => 1,
            RampDir::PosZ => 2,
            RampDir::NegZ => 3,
        }
    }

    pub const fn from_code(c: u8) -> RampDir {
        match c & 3 {
            0 => RampDir::PosX,
            1 => RampDir::NegX,
            2 => RampDir::PosZ,
            _ => RampDir::NegZ,
        }
    }
}

/// What a surface does to a body.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SurfaceKind {
    /// A box, solid from every side.
    Solid,
    /// Solid only from above: bodies jump up through it and land on it.
    OneWay,
    /// A box whose top slopes from `min.y` (the low side) to `max.y` toward
    /// `RampDir`. Walkable when the slope is within the body's
    /// `max_slope_tan`, a wall (and a slide when landed on) otherwise.
    Ramp(RampDir),
}

/// Collision geometry for the controller: a box with a [`SurfaceKind`] on a
/// layer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Surface<S> {
    pub aabb: Box3<S>,
    pub layer: u16,
    pub kind: SurfaceKind,
}

impl<S: Scalar> Surface<S> {
    pub fn solid(aabb: Box3<S>) -> Self {
        Surface {
            aabb,
            layer: 1,
            kind: SurfaceKind::Solid,
        }
    }

    pub fn one_way(aabb: Box3<S>) -> Self {
        Surface {
            aabb,
            layer: 1,
            kind: SurfaceKind::OneWay,
        }
    }

    pub fn ramp(aabb: Box3<S>, dir: RampDir) -> Self {
        Surface {
            aabb,
            layer: 1,
            kind: SurfaceKind::Ramp(dir),
        }
    }

    /// Height of the top under the XZ footprint of `foot`: the box's top,
    /// or a ramp's surface at the footprint's highest point (within the
    /// ramp).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn top_at(&self, foot: &Box3<S>) -> S {
        let b = &self.aabb;
        match self.kind {
            SurfaceKind::Ramp(dir) => {
                let (a, up) = dir.axis();
                let (lo, hi) = (b.min.get(a), b.max.get(a));
                let run = hi - lo;
                if run <= S::ZERO {
                    return b.max.y;
                }
                let u = if up {
                    foot.max.get(a).clamp(lo, hi) - lo
                } else {
                    hi - foot.min.get(a).clamp(lo, hi)
                };
                b.min.y + u * (b.max.y - b.min.y) / run
            }
            _ => b.max.y,
        }
    }

    /// Is the top walkable for a body allowed `max_slope_tan` (rise/run)?
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn walkable(&self, max_slope_tan: S) -> bool {
        match self.kind {
            SurfaceKind::Ramp(dir) => {
                let (a, _) = dir.axis();
                let run = self.aabb.max.get(a) - self.aabb.min.get(a);
                self.aabb.max.y - self.aabb.min.y <= run * max_slope_tan
            }
            _ => true,
        }
    }

    /// The top's unit normal (up for boxes).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn normal(&self) -> V3<S> {
        match self.kind {
            SurfaceKind::Ramp(dir) => {
                let (a, up) = dir.axis();
                let run = self.aabb.max.get(a) - self.aabb.min.get(a);
                let rise = self.aabb.max.y - self.aabb.min.y;
                let len = (run * run + rise * rise).sqrt();
                let mut n = V3::new(S::ZERO, run / len, S::ZERO);
                let side = rise / len;
                n.set(a, if up { -side } else { side });
                n
            }
            _ => V3::new(S::ZERO, S::ONE, S::ZERO),
        }
    }

    #[inline]
    fn takes(&self, mask: u16) -> bool {
        self.layer & mask != 0
    }
}

impl From<&Solid> for Surface<f32> {
    fn from(s: &Solid) -> Self {
        Surface {
            aabb: Box3::from_aabb(&s.aabb),
            layer: s.layer,
            kind: s.kind,
        }
    }
}

/// How a body moves. Every option is off (zero) in [`ControllerConfig::new`]
/// except the slope limit, which only matters to ramps and meshes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ControllerConfig<S> {
    /// Half extents of the body's box.
    pub half: V3<S>,
    pub gravity: S,
    /// Fastest fall (positive).
    pub max_fall: S,
    /// Substeps per second: a frame takes `trunc(dt * hz) + 1` of them, at
    /// most `max_substeps` (no tunneling through thin solids).
    pub substep_hz: S,
    pub max_substeps: i32,
    /// Ledges up to this high are climbed while walking (0 = off).
    pub step_height: S,
    /// While grounded, follow the ground down by up to this much per
    /// substep instead of leaving it (slopes, stairs down; 0 = off).
    pub snap_down: S,
    /// Steepest walkable ramp, as rise / run (tan of the angle).
    pub max_slope_tan: S,
    /// The same limit for triangle meshes, as the normal's minimum Y (cos).
    pub min_normal_y: f32,
    /// A jump still works this long after walking off a ledge.
    pub coyote_time: S,
    /// A jump pressed this long before landing happens on landing.
    pub jump_buffer: S,
    /// Layers the body collides with.
    pub mask: u16,
}

impl<S: Scalar> ControllerConfig<S> {
    /// A body of `half` extents with gravity and a fall cap; 120 substeps
    /// per second (at most 12), steps, snapping, coyote time and jump
    /// buffering off, slopes up to 45°.
    pub fn new(half: V3<S>, gravity: S, max_fall: S) -> Self {
        ControllerConfig {
            half,
            gravity,
            max_fall,
            substep_hz: S::from_int(120),
            max_substeps: 12,
            step_height: S::ZERO,
            snap_down: S::ZERO,
            max_slope_tan: S::ONE,
            min_normal_y: core::f32::consts::FRAC_1_SQRT_2,
            coyote_time: S::ZERO,
            jump_buffer: S::ZERO,
            mask: super::LAYER_ALL,
        }
    }

    /// A capsule of `radius` and total `height`, as the box the controller
    /// collides with (the same on every target).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn capsule(radius: S, height: S, gravity: S, max_fall: S) -> Self {
        let two = S::ONE + S::ONE;
        Self::new(V3::new(radius, height / two, radius), gravity, max_fall)
    }

    /// Set the steepest walkable slope in degrees (ramps and meshes).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn with_max_slope_deg(mut self, deg: f32) -> Self {
        let r = deg.clamp(0.0, 89.0) * (core::f32::consts::PI / 180.0);
        let (s, c) = (math3d::sin(r), math3d::cos(r));
        self.max_slope_tan = S::from_f32(s / c);
        self.min_normal_y = c;
        self
    }
}

/// A scene's character component (`character: (radius, height,
/// max_slope_deg, step_height)`): the body of a player or an NPC.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Character {
    pub radius: f32,
    /// Total height.
    pub height: f32,
    pub max_slope_deg: f32,
    pub step_height: f32,
}

impl Character {
    pub const DEFAULT: Character = Character {
        radius: 0.3,
        height: 1.1,
        max_slope_deg: 45.0,
        step_height: 0.0,
    };

    /// Its controller settings, with the game's gravity and fall cap.
    /// Steps also snap down as far, so stairs work both ways.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn config(&self, gravity: f32, max_fall: f32) -> ControllerConfig<f32> {
        let mut c = ControllerConfig::capsule(self.radius, self.height, gravity, max_fall)
            .with_max_slope_deg(self.max_slope_deg);
        c.step_height = self.step_height;
        c.snap_down = self.step_height;
        c
    }
}

/// One move of a body ([`move_body`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BodyMove<S> {
    pub pos: V3<S>,
    pub vel: V3<S>,
    pub on_ground: bool,
    pub hit_ceiling: bool,
    pub hit_wall: bool,
    /// Climbed a ledge (step height).
    pub stepped: bool,
    /// Resting on ground too steep to stand on.
    pub sliding: bool,
    /// The surface under the body (index into the surfaces).
    pub standing_on: Option<usize>,
    /// The mesh (index) and triangle under the body.
    pub mesh_ground: Option<(usize, u32)>,
    pub ground_normal: V3<S>,
}

/// The per-move options of [`move_body`] (from a [`ControllerConfig`]).
#[derive(Clone, Copy, Debug)]
pub struct MoveOpts<S> {
    pub step_height: S,
    pub max_slope_tan: S,
    pub min_normal_y: f32,
    pub mask: u16,
    /// The body stood on the ground before this move (steps need it).
    pub was_grounded: bool,
}

impl<S: Scalar> MoveOpts<S> {
    /// `move_aabb`'s behaviour: no steps, every layer.
    pub fn plain() -> Self {
        MoveOpts {
            step_height: S::ZERO,
            max_slope_tan: S::ONE,
            min_normal_y: core::f32::consts::FRAC_1_SQRT_2,
            mask: super::LAYER_ALL,
            was_grounded: false,
        }
    }
}

#[inline]
fn up<S: Scalar>() -> V3<S> {
    V3::new(S::ZERO, S::ONE, S::ZERO)
}

/// Move a box centered at `pos` by `vel * dt` against `surfaces` and
/// `meshes`, one axis at a time. Over [`SurfaceKind::Solid`] boxes with
/// [`MoveOpts::plain`] this is [`move_aabb`](super::move_aabb), bit for bit.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn move_body<S: Scalar>(
    pos: V3<S>,
    half: V3<S>,
    vel: V3<S>,
    dt: S,
    surfaces: &[Surface<S>],
    meshes: &[&dyn MeshQuery<S>],
    o: &MoveOpts<S>,
) -> BodyMove<S> {
    let skin = S::from_f32(SKIN);
    let mut out = BodyMove {
        pos,
        vel,
        on_ground: false,
        hit_ceiling: false,
        hit_wall: false,
        stepped: false,
        sliding: false,
        standing_on: None,
        mesh_ground: None,
        ground_normal: up(),
    };
    // How far a walk may lift the body this move (a slope's rise or a
    // step), plus the skin.
    let horiz = (vel.x * dt).abs() + (vel.z * dt).abs();
    let allowance = o.step_height.max(o.max_slope_tan * horiz) + skin;

    for axis in [0usize, 2, 1] {
        let dv = vel.get(axis) * dt;
        if dv == S::ZERO && axis != 1 {
            continue;
        }
        let before = out.pos;
        let new = out.pos.get(axis) + dv;
        out.pos.set(axis, new);
        let sk = if axis == 1 {
            V3::new(skin, S::ZERO, skin)
        } else {
            V3::new(S::ZERO, skin, S::ZERO)
        };
        let me = Box3::new(out.pos - half + sk, out.pos + half - sk);
        for (i, s) in surfaces.iter().enumerate() {
            if !s.takes(o.mask) {
                continue;
            }
            match s.kind {
                SurfaceKind::Solid => {
                    if !me.overlaps(&s.aabb) {
                        continue;
                    }
                    if axis == 1 {
                        if dv <= S::ZERO {
                            out.pos.y = s.aabb.max.y + half.y;
                            out.vel.y = S::ZERO;
                            out.on_ground = true;
                            out.standing_on = Some(i);
                            out.ground_normal = up();
                        } else {
                            out.pos.y = s.aabb.min.y - half.y;
                            out.vel.y = S::ZERO;
                            out.hit_ceiling = true;
                        }
                    } else {
                        if o.step_height > S::ZERO
                            && o.was_grounded
                            && try_step(&mut out, s, half, skin, surfaces, meshes, o)
                        {
                            continue;
                        }
                        wall(&mut out, axis, dv, &s.aabb, half);
                    }
                }
                SurfaceKind::OneWay => {
                    let feet_before = before.y - half.y;
                    if axis == 1
                        && dv <= S::ZERO
                        && feet_before >= s.aabb.max.y - skin
                        && me.overlaps(&s.aabb)
                    {
                        out.pos.y = s.aabb.max.y + half.y;
                        out.vel.y = S::ZERO;
                        out.on_ground = true;
                        out.standing_on = Some(i);
                        out.ground_normal = up();
                    }
                }
                SurfaceKind::Ramp(_) => {
                    // The slope's height under the whole footprint (both
                    // passes agree, or landing would read as a wall).
                    let foot = Box3::new(out.pos - half, out.pos + half);
                    let top = s.top_at(&foot);
                    if !(me.overlaps_xz(&s.aabb) && me.min.y < top && me.max.y > s.aabb.min.y) {
                        continue;
                    }
                    let walkable = s.walkable(o.max_slope_tan);
                    if axis != 1 {
                        let feet = out.pos.y - half.y;
                        if !(walkable && top - feet <= allowance) {
                            // Back out of the wedge along this axis.
                            out.pos.set(axis, before.get(axis));
                            out.hit_wall = true;
                            out.vel.set(axis, S::ZERO);
                        }
                        continue;
                    }
                    let feet_before = before.y - half.y;
                    if feet_before + allowance >= top || dv <= S::ZERO {
                        out.pos.y = top + half.y;
                        if dv <= S::ZERO {
                            out.vel.y = S::ZERO;
                            out.standing_on = Some(i);
                            out.ground_normal = s.normal();
                            if walkable {
                                out.on_ground = true;
                            } else {
                                out.sliding = true;
                            }
                        }
                    } else {
                        out.pos.y = s.aabb.min.y - half.y;
                        out.vel.y = S::ZERO;
                        out.hit_ceiling = true;
                    }
                }
            }
        }
        for (mi, m) in meshes.iter().enumerate() {
            if axis != 1 {
                let me = Box3::new(out.pos - half + sk, out.pos + half - sk);
                if m.blocks(&me, o.min_normal_y) {
                    out.pos.set(axis, before.get(axis));
                    out.hit_wall = true;
                    out.vel.set(axis, S::ZERO);
                }
                continue;
            }
            let me = Box3::new(out.pos - half + sk, out.pos + half - sk);
            let feet_before = before.y - half.y;
            if let Some(g) = m.support(&me, feet_before + allowance, o.min_normal_y)
                && me.min.y < g.height
            {
                out.pos.y = g.height + half.y;
                if dv <= S::ZERO {
                    out.vel.y = S::ZERO;
                    out.on_ground = true;
                    out.standing_on = None;
                    out.mesh_ground = Some((mi, g.tri));
                    out.ground_normal = g.normal;
                }
            }
        }
    }
    out
}

/// Stop at a box's face along `axis` (`move_aabb`'s wall response).
#[inline]
fn wall<S: Scalar>(out: &mut BodyMove<S>, axis: usize, dv: S, b: &Box3<S>, half: V3<S>) {
    if dv > S::ZERO {
        out.pos.set(axis, b.min.get(axis) - half.get(axis));
    } else if dv < S::ZERO {
        out.pos.set(axis, b.max.get(axis) + half.get(axis));
    }
    out.hit_wall = true;
    out.vel.set(axis, S::ZERO);
}

/// Climb onto `s` when its top is within the step height and the body fits
/// there.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn try_step<S: Scalar>(
    out: &mut BodyMove<S>,
    s: &Surface<S>,
    half: V3<S>,
    skin: S,
    surfaces: &[Surface<S>],
    meshes: &[&dyn MeshQuery<S>],
    o: &MoveOpts<S>,
) -> bool {
    let feet = out.pos.y - half.y;
    let rise = s.aabb.max.y - feet;
    if rise <= S::ZERO || rise > o.step_height {
        return false;
    }
    let mut c = out.pos;
    c.y = s.aabb.max.y + half.y;
    let k = V3::new(skin, skin, skin);
    let probe = Box3::new(c - half + k, c + half - k);
    let blocked = surfaces.iter().any(|t| {
        t.takes(o.mask)
            && match t.kind {
                SurfaceKind::Solid => probe.overlaps(&t.aabb),
                SurfaceKind::Ramp(_) => {
                    probe.overlaps_xz(&t.aabb)
                        && probe.min.y < t.top_at(&probe)
                        && probe.max.y > t.aabb.min.y
                }
                SurfaceKind::OneWay => false,
            }
    }) || meshes.iter().any(|m| m.blocks(&probe, o.min_normal_y));
    if blocked {
        return false;
    }
    out.pos.y = c.y;
    out.stepped = true;
    true
}

/// Ground found under a body: (height, surface, mesh triangle, normal,
/// walkable).
type Ground<S> = (S, Option<usize>, Option<(usize, u32)>, V3<S>, bool);

/// The highest ground within `snap` below the feet of a box at `pos`.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn snap_target<S: Scalar>(
    pos: V3<S>,
    half: V3<S>,
    snap: S,
    surfaces: &[Surface<S>],
    meshes: &[&dyn MeshQuery<S>],
    o: &MoveOpts<S>,
) -> Option<Ground<S>> {
    let skin = S::from_f32(SKIN);
    let feet = pos.y - half.y;
    let k = V3::new(skin, S::ZERO, skin);
    let foot = Box3::new(pos - half + k, pos + half - k);
    let full = Box3::new(pos - half, pos + half);
    let mut best: Option<Ground<S>> = None;
    let mut consider = |h: S, s: Option<usize>, m: Option<(usize, u32)>, n: V3<S>, walk: bool| {
        if h <= feet + skin && h >= feet - snap && best.is_none_or(|b| h > b.0) {
            best = Some((h, s, m, n, walk));
        }
    };
    for (i, s) in surfaces.iter().enumerate() {
        if !s.takes(o.mask) || !foot.overlaps_xz(&s.aabb) {
            continue;
        }
        let walk = s.walkable(o.max_slope_tan);
        consider(s.top_at(&full), Some(i), None, s.normal(), walk);
    }
    for (mi, m) in meshes.iter().enumerate() {
        if let Some(g) = m.support(&foot, feet + skin, o.min_normal_y) {
            consider(g.height, None, Some((mi, g.tri)), g.normal, true);
        }
    }
    best
}

/// What a [`CharacterController::step`] did.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StepReport<S> {
    pub on_ground: bool,
    pub hit_wall: bool,
    pub hit_ceiling: bool,
    pub stepped: bool,
    pub sliding: bool,
    /// Followed the ground down (snap) during the step.
    pub snapped: bool,
    pub standing_on: Option<usize>,
    pub mesh_ground: Option<(usize, u32)>,
    pub ground_normal: V3<S>,
    /// `step_world`: more candidate surfaces than its buffer (some ignored).
    pub truncated: bool,
}

/// A walking, jumping body. The game sets `vel.x`/`vel.z` (its own
/// acceleration and turning), calls [`jump`](Self::jump) and
/// [`step`](Self::step) once per frame, and reads `pos`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CharacterController<S> {
    pub cfg: ControllerConfig<S>,
    /// Center of the body's box.
    pub pos: V3<S>,
    pub vel: V3<S>,
    pub on_ground: bool,
    pub standing_on: Option<usize>,
    pub ground_normal: V3<S>,
    coyote: S,
    buffered: S,
    /// Downhill speed picked up on ground too steep to stand on: the
    /// controller's own, since the game sets `vel.x`/`vel.z` every frame.
    /// Gone on walkable ground or against a wall.
    slide: V3<S>,
    /// The last substep ended on steep ground (snapping follows a slide).
    was_sliding: bool,
}

impl<S: Scalar> CharacterController<S> {
    pub fn new(cfg: ControllerConfig<S>, pos: V3<S>) -> Self {
        CharacterController {
            cfg,
            pos,
            vel: V3::ZERO,
            on_ground: false,
            standing_on: None,
            ground_normal: up(),
            coyote: S::ZERO,
            buffered: S::ZERO,
            slide: V3::ZERO,
            was_sliding: false,
        }
    }

    /// Put the body at `pos`, at rest in the air (respawns, level loads).
    pub fn teleport(&mut self, pos: V3<S>) {
        *self = Self::new(self.cfg, pos);
    }

    /// Move by a platform's displacement this frame (call before
    /// [`step`](Self::step) while `standing_on` is that platform).
    pub fn carry(&mut self, delta: V3<S>) {
        self.pos += delta;
    }

    /// Jump at `speed` when `pressed` (this frame) or a buffered press
    /// meets the ground or coyote time. Returns whether it jumped. With
    /// coyote time and buffering off: `pressed && on_ground`.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn jump(&mut self, pressed: bool, speed: S) -> bool {
        if pressed {
            self.buffered = self.cfg.jump_buffer;
        }
        let want = pressed || self.buffered > S::ZERO;
        if want && (self.on_ground || self.coyote > S::ZERO) {
            self.vel.y = speed;
            self.on_ground = false;
            self.coyote = S::ZERO;
            self.buffered = S::ZERO;
            return true;
        }
        false
    }

    /// Advance `dt` seconds: gravity and a move per substep, then the
    /// state from the last one. One compiled body per scalar, in this crate
    /// (`Scalar::controller_step`): every game crate that steps a body
    /// shares it instead of compiling its own 8 KB copy (the N64's RAM).
    #[inline]
    pub fn step(
        &mut self,
        dt: S,
        surfaces: &[Surface<S>],
        meshes: &[&dyn MeshQuery<S>],
    ) -> StepReport<S> {
        S::controller_step(self, dt, surfaces, meshes)
    }

    /// [`step`](Self::step)'s body.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub(crate) fn step_impl(
        &mut self,
        dt: S,
        surfaces: &[Surface<S>],
        meshes: &[&dyn MeshQuery<S>],
    ) -> StepReport<S> {
        let c = self.cfg;
        // Timers run per frame.
        if self.on_ground {
            self.coyote = c.coyote_time;
        } else {
            self.coyote = (self.coyote - dt).max(S::ZERO);
        }
        self.buffered = (self.buffered - dt).max(S::ZERO);
        let steps = ((dt * c.substep_hz).trunc_i32() + 1).clamp(1, c.max_substeps.max(1));
        let h = dt / S::from_int(steps);
        let mut rep = StepReport {
            on_ground: self.on_ground,
            hit_wall: false,
            hit_ceiling: false,
            stepped: false,
            sliding: false,
            snapped: false,
            standing_on: self.standing_on,
            mesh_ground: None,
            ground_normal: self.ground_normal,
            truncated: false,
        };
        for _ in 0..steps {
            self.vel.y = (self.vel.y - c.gravity * h).max(-c.max_fall);
            let o = MoveOpts {
                step_height: c.step_height,
                max_slope_tan: c.max_slope_tan,
                min_normal_y: c.min_normal_y,
                mask: c.mask,
                was_grounded: self.on_ground,
            };
            // The slide rides on top of the game's velocity (only while
            // there is one: castle64's arithmetic is untouched).
            let sliding = self.slide != V3::ZERO;
            let vel = if sliding {
                V3::new(
                    self.vel.x + self.slide.x,
                    self.vel.y,
                    self.vel.z + self.slide.z,
                )
            } else {
                self.vel
            };
            let mut out = move_body(self.pos, c.half, vel, h, surfaces, meshes, &o);
            if sliding {
                // A wall stops the slide on its axis; the game's own
                // velocity comes back without it.
                for a in [0usize, 2] {
                    if out.vel.get(a) == S::ZERO {
                        self.slide.set(a, S::ZERO);
                    } else {
                        out.vel.set(a, self.vel.get(a));
                    }
                }
            }
            if c.snap_down > S::ZERO
                && (o.was_grounded || self.was_sliding)
                && !out.on_ground
                && !out.sliding
                && out.vel.y <= S::ZERO
                && let Some((height, s, m, n, walk)) =
                    snap_target(out.pos, c.half, c.snap_down, surfaces, meshes, &o)
            {
                out.pos.y = height + c.half.y;
                out.vel.y = S::ZERO;
                out.on_ground = walk;
                out.sliding = !walk;
                out.standing_on = s;
                out.mesh_ground = m;
                out.ground_normal = n;
                rep.snapped = true;
            }
            if out.sliding {
                // Down the slope: the normal's horizontal part points
                // downhill.
                let g = c.gravity * h;
                self.slide.x += out.ground_normal.x * g;
                self.slide.z += out.ground_normal.z * g;
            } else if out.on_ground {
                self.slide = V3::ZERO;
            }
            self.was_sliding = out.sliding;
            self.pos = out.pos;
            self.vel = out.vel;
            self.on_ground = out.on_ground;
            self.standing_on = out.standing_on;
            self.ground_normal = out.ground_normal;
            rep.hit_wall |= out.hit_wall;
            rep.hit_ceiling |= out.hit_ceiling;
            rep.stepped |= out.stepped;
            rep.sliding = out.sliding;
            rep.mesh_ground = out.mesh_ground;
        }
        rep.on_ground = self.on_ground;
        rep.standing_on = self.standing_on;
        rep.ground_normal = self.ground_normal;
        rep
    }

    /// The body's world box.
    pub fn aabb(&self) -> Box3<S> {
        Box3::from_center_half(self.pos, self.cfg.half)
    }
}

/// Most surfaces [`CharacterController::step_world`] considers in a frame.
pub const STEP_WORLD_CANDIDATES: usize = 64;

impl CharacterController<f32> {
    /// [`step`](Self::step) against a [`World`]: the grid's and the dynamic
    /// list's surfaces near the frame's move, in world index order, so
    /// `standing_on` is a world index.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn step_world(
        &mut self,
        dt: f32,
        world: &World,
        meshes: &[&dyn MeshQuery<f32>],
    ) -> StepReport<f32> {
        let c = self.cfg;
        let reach = self.vel.x.abs().max(self.vel.z.abs()).max(self.vel.y.abs()) * dt
            + c.step_height
            + c.snap_down * c.max_substeps as f32
            + c.max_fall * dt
            + 0.5;
        let r = V3::new(c.half.x + reach, c.half.y + reach, c.half.z + reach);
        let q = Box3::from_center_half(self.pos, r).to_aabb();
        let mut ids = [0u16; STEP_WORLD_CANDIDATES];
        let total = world.overlap_aabb(&q, c.mask, &mut ids);
        let n = total.min(ids.len());
        let mut local = [Surface::solid(Box3::new(V3::ZERO, V3::ZERO)); STEP_WORLD_CANDIDATES];
        let base = world.statics.len();
        for (k, &id) in ids[..n].iter().enumerate() {
            let id = id as usize;
            let s = if id < base {
                &world.statics.solids()[id]
            } else {
                &world.dynamics[id - base]
            };
            local[k] = s.into();
        }
        // standing_on is kept in world indices between frames.
        self.standing_on = self
            .standing_on
            .and_then(|w| ids[..n].iter().position(|&x| x as usize == w));
        let mut rep = self.step(dt, &local[..n], meshes);
        self.standing_on = self.standing_on.map(|l| ids[l] as usize);
        rep.standing_on = self.standing_on;
        rep.truncated = total > ids.len();
        rep
    }
}
