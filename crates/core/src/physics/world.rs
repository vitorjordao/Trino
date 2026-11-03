//! Physics 2.0, phase 1 (issue #66): a broadphase for level solids, layer
//! masks, shape casts and trigger events. `no_std`, no allocation: the
//! caller owns every buffer, and every query is deterministic (candidates
//! are reduced in a fixed order, ties go to the lowest index), so the grid
//! answers exactly what a scan of the same slice would.
//!
//! - [`Solid`]: an [`Aabb`] plus a layer bit. A query carries a `mask`; a
//!   solid takes part when `solid.layer & mask != 0` ([`LAYER_ALL`] = all).
//! - [`StaticWorld`]: a uniform XZ grid over a slice of level solids, built
//!   once (at load, or from a baked scene) into two caller-owned `u16`
//!   buffers sized by [`StaticWorld::size`]. Levels are wide and flat, so
//!   the grid is 2D; a column of solids shares a cell list.
//! - [`World`]: the grid plus a small dynamic slice scanned linearly (moving
//!   platforms, enemies). Dynamic indices follow the static ones.
//! - [`CastShape`]: sphere, Y-aligned capsule or box swept from a point to
//!   another. Sphere and capsule casts are exact against boxes (face, edge
//!   and corner contacts); a box cast is exact too.
//! - [`TriggerTracker`]: Enter / Stay / Exit events for tracked bodies
//!   against trigger volumes.

use super::controller::{RampDir, SurfaceKind};
use super::convex::{Convex, one_way_takes};
use super::{Aabb, RayHit, ray_box};
use crate::math::Vec3;
use crate::math3d::{sqrt, to_i32};

/// Every layer: the mask of a query that ignores layers.
pub const LAYER_ALL: u16 = 0xFFFF;

/// A level solid or a dynamic body's box, on a layer. `kind` makes it a
/// one-way platform or a ramp for the
/// [`CharacterController`](super::CharacterController), rays, overlaps and
/// shape casts: a ramp is its wedge everywhere (issue #115), a one-way box
/// stops casts that start above it and is its box to a (motionless)
/// overlap.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Solid {
    pub aabb: Aabb,
    /// Layer bits (a solid usually has one set).
    pub layer: u16,
    pub kind: SurfaceKind,
}

impl Solid {
    pub const fn new(aabb: Aabb, layer: u16) -> Self {
        Solid {
            aabb,
            layer,
            kind: SurfaceKind::Solid,
        }
    }

    pub const fn one_way(aabb: Aabb, layer: u16) -> Self {
        Solid {
            aabb,
            layer,
            kind: SurfaceKind::OneWay,
        }
    }

    pub const fn ramp(aabb: Aabb, layer: u16, dir: RampDir) -> Self {
        Solid {
            aabb,
            layer,
            kind: SurfaceKind::Ramp(dir),
        }
    }

    #[inline]
    pub(crate) fn takes(&self, mask: u16) -> bool {
        self.layer & mask != 0
    }

    /// Does `q` overlap it? A ramp is its wedge, any other kind its box.
    /// Touching does not overlap.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn overlaps_aabb(&self, q: &Aabb) -> bool {
        if !self.aabb.overlaps(q) {
            return false;
        }
        let SurfaceKind::Ramp(dir) = self.kind else {
            return true;
        };
        // The wedge is the box under the slope's plane: `q ∩ box` (a box)
        // meets it where its lowest point toward the low side lies below
        // the plane.
        let b = &self.aabb;
        let (lo, hi) = (
            Vec3::new(
                q.min.x.max(b.min.x),
                q.min.y.max(b.min.y),
                q.min.z.max(b.min.z),
            ),
            Vec3::new(
                q.max.x.min(b.max.x),
                q.max.y.min(b.max.y),
                q.max.z.min(b.max.z),
            ),
        );
        let rise = b.max.y - b.min.y;
        // u: how far into the run the box reaches from the low side.
        let (u, run) = match dir {
            RampDir::PosX => (hi.x - b.min.x, b.max.x - b.min.x),
            RampDir::NegX => (b.max.x - lo.x, b.max.x - b.min.x),
            RampDir::PosZ => (hi.z - b.min.z, b.max.z - b.min.z),
            RampDir::NegZ => (b.max.z - lo.z, b.max.z - b.min.z),
        };
        run <= 0.0 || (lo.y - b.min.y) * run < u * rise
    }

    /// Does `shape` centered at `c` overlap it (exactly: round shapes by
    /// distance, ramps as their wedge)?
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn overlaps_shape(&self, shape: CastShape, c: Vec3) -> bool {
        if !shape.bounds(c).overlaps(&self.aabb) {
            return false;
        }
        match self.kind {
            SurfaceKind::Ramp(dir) => match shape {
                CastShape::Box { half } => {
                    Convex::wedge(&self.aabb, dir, 0.0).overlaps_box(c, half)
                }
                _ => {
                    let (r, seg) = shape.round();
                    Convex::wedge(&self.aabb, dir, seg).closest(c).1 < r * r
                }
            },
            _ => shape.overlaps(c, &self.aabb),
        }
    }
}

/// What an overlap query tests a solid against (one compiled query for
/// all of them: the N64's RAM).
#[derive(Clone, Copy)]
pub(crate) enum Probe {
    /// The solid's bounding box against a box (a superset).
    Bounds(Aabb),
    /// [`Solid::overlaps_aabb`].
    Aabb(Aabb),
}

impl Probe {
    pub(crate) fn bounds(&self) -> Aabb {
        match *self {
            Probe::Bounds(q) | Probe::Aabb(q) => q,
        }
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub(crate) fn hits(&self, s: &Solid) -> bool {
        match *self {
            Probe::Bounds(q) => s.aabb.overlaps(&q),
            Probe::Aabb(q) => s.overlaps_aabb(&q),
        }
    }
}

/// Buffer lengths [`StaticWorld::build`] needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GridSize {
    /// Length of the `starts` buffer (cells + 1).
    pub starts: usize,
    /// Length of the `refs` buffer (solid references over all cells).
    pub refs: usize,
}

/// A uniform grid over level solids. See the module docs.
#[derive(Clone, Copy)]
pub struct StaticWorld<'a> {
    solids: &'a [Solid],
    ox: f32,
    oz: f32,
    cell: f32,
    inv: f32,
    nx: usize,
    nz: usize,
    starts: &'a [u16],
    refs: &'a [u16],
}

#[inline]
fn cell_of(v: f32, o: f32, inv: f32, n: usize) -> usize {
    let t = (v - o) * inv;
    if t <= 0.0 {
        0
    } else {
        (to_i32(t) as usize).min(n - 1)
    }
}

/// Grid origin and dimensions for `solids` at cell size `cell`.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn grid_shape(solids: &[Solid], cell: f32) -> Option<(f32, f32, usize, usize)> {
    if solids.is_empty() || cell <= 0.0 || solids.len() > u16::MAX as usize {
        return None;
    }
    let mut b = solids[0].aabb;
    for s in &solids[1..] {
        b = b.union(&s.aabb);
    }
    let inv = 1.0 / cell;
    let nx = to_i32((b.max.x - b.min.x) * inv) as usize + 1;
    let nz = to_i32((b.max.z - b.min.z) * inv) as usize + 1;
    Some((b.min.x, b.min.z, nx, nz))
}

impl<'a> StaticWorld<'a> {
    /// The buffers `solids` need at `cell` metres per cell, or `None` for
    /// no solids, a bad cell size, or more than 65 535 solids / references.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn size(solids: &[Solid], cell: f32) -> Option<GridSize> {
        let (ox, oz, nx, nz) = grid_shape(solids, cell)?;
        let inv = 1.0 / cell;
        let mut refs = 0usize;
        for s in solids {
            let (x0, x1, z0, z1) = span(&s.aabb, ox, oz, inv, nx, nz);
            refs += (x1 - x0 + 1) * (z1 - z0 + 1);
        }
        let cells = nx.checked_mul(nz)?;
        (cells < u16::MAX as usize && refs <= u16::MAX as usize).then_some(GridSize {
            starts: cells + 1,
            refs,
        })
    }

    /// Build the grid into `starts` / `refs` (lengths from [`size`](Self::size)
    /// or longer). `None` when a buffer is too small. A good `cell` is a few
    /// times the size of the things that query it.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn build(
        solids: &'a [Solid],
        cell: f32,
        starts: &'a mut [u16],
        refs: &'a mut [u16],
    ) -> Option<Self> {
        let need = Self::size(solids, cell)?;
        if starts.len() < need.starts || refs.len() < need.refs {
            return None;
        }
        let (ox, oz, nx, nz) = grid_shape(solids, cell)?;
        let inv = 1.0 / cell;
        let cells = nx * nz;
        starts[..=cells].fill(0);
        for s in solids {
            let (x0, x1, z0, z1) = span(&s.aabb, ox, oz, inv, nx, nz);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    starts[z * nx + x + 1] += 1;
                }
            }
        }
        for c in 0..cells {
            starts[c + 1] += starts[c];
        }
        // Fill with starts[c] as the cursor (solids ascend, so each cell's
        // list does too); afterwards starts[c] holds the next cell's begin.
        for (i, s) in solids.iter().enumerate() {
            let (x0, x1, z0, z1) = span(&s.aabb, ox, oz, inv, nx, nz);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let c = z * nx + x;
                    refs[starts[c] as usize] = i as u16;
                    starts[c] += 1;
                }
            }
        }
        for c in (1..cells).rev() {
            starts[c] = starts[c - 1];
        }
        starts[0] = 0;
        let (starts, refs): (&'a [u16], &'a [u16]) = (starts, refs);
        Some(StaticWorld {
            solids,
            ox,
            oz,
            cell,
            inv,
            nx,
            nz,
            starts: &starts[..=cells],
            refs: &refs[..need.refs],
        })
    }

    pub fn solids(&self) -> &'a [Solid] {
        self.solids
    }

    /// The grid as data: (origin x, origin z, cell size, nx, nz, starts,
    /// refs) — what a baked scene stores ([`super::write_section`]).
    pub fn raw(&self) -> (f32, f32, f32, usize, usize, &'a [u16], &'a [u16]) {
        (
            self.ox,
            self.oz,
            self.cell,
            self.nx,
            self.nz,
            self.starts,
            self.refs,
        )
    }

    /// A grid from [`raw`](Self::raw)'s data (a baked scene's), validated:
    /// `None` unless the cell lists fit `starts`, ascend and name solids of
    /// `solids` only.
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn from_raw(
        solids: &'a [Solid],
        ox: f32,
        oz: f32,
        cell: f32,
        nx: usize,
        nz: usize,
        starts: &'a [u16],
        refs: &'a [u16],
    ) -> Option<Self> {
        let cells = nx.checked_mul(nz)?;
        if cell.is_nan()
            || cell <= 0.0
            || nx == 0
            || nz == 0
            || starts.len() != cells + 1
            || starts[0] != 0
            || starts[cells] as usize != refs.len()
            || starts.windows(2).any(|w| w[0] > w[1])
            || refs.iter().any(|&r| r as usize >= solids.len())
        {
            return None;
        }
        Some(StaticWorld {
            solids,
            ox,
            oz,
            cell,
            inv: 1.0 / cell,
            nx,
            nz,
            starts,
            refs,
        })
    }

    pub fn len(&self) -> usize {
        self.solids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.solids.is_empty()
    }

    #[inline]
    fn cell_refs(&self, x: usize, z: usize) -> &'a [u16] {
        let c = z * self.nx + x;
        &self.refs[self.starts[c] as usize..self.starts[c + 1] as usize]
    }

    /// Visit each solid whose cells meet `q` once, in cell order.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn candidates(&self, q: &Aabb, mut f: impl FnMut(usize)) {
        let (qx0, qx1, qz0, qz1) = span(q, self.ox, self.oz, self.inv, self.nx, self.nz);
        for z in qz0..=qz1 {
            for x in qx0..=qx1 {
                for &r in self.cell_refs(x, z) {
                    let i = r as usize;
                    // Report a solid in the first cell it shares with the
                    // query only, so it is visited once.
                    let (sx0, _, sz0, _) = span(
                        &self.solids[i].aabb,
                        self.ox,
                        self.oz,
                        self.inv,
                        self.nx,
                        self.nz,
                    );
                    if x == sx0.max(qx0) && z == sz0.max(qz0) {
                        f(i);
                    }
                }
            }
        }
    }

    /// Indices of the solids overlapping `q` on `mask`, ascending, into
    /// `out`. Returns how many overlap; when that is more than `out.len()`,
    /// `out` holds the lowest indices (what [`overlap_solids`] keeps).
    /// A ramp counts where its wedge overlaps `q` ([`Solid::overlaps_aabb`]).
    pub fn overlap_aabb(&self, q: &Aabb, mask: u16, out: &mut [u16]) -> usize {
        self.overlap_probe(&Probe::Aabb(*q), mask, out)
    }

    /// [`overlap_aabb`](Self::overlap_aabb) by bounding boxes only (a
    /// superset): what the controller gathers its surfaces with.
    pub fn overlap_bounds(&self, q: &Aabb, mask: u16, out: &mut [u16]) -> usize {
        self.overlap_probe(&Probe::Bounds(*q), mask, out)
    }

    /// Solids on `mask` that `shape` centered at `c` overlaps, exactly
    /// ([`Solid::overlaps_shape`]); ascending like
    /// [`overlap_aabb`](Self::overlap_aabb).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn overlap_shape(&self, shape: CastShape, c: Vec3, mask: u16, out: &mut [u16]) -> usize {
        let (mut total, mut kept) = (0, 0);
        self.candidates(&shape.bounds(c), |i| {
            let s = &self.solids[i];
            if s.takes(mask) && s.overlaps_shape(shape, c) {
                total += 1;
                kept = insert_lowest(out, kept, i as u16);
            }
        });
        sort_small(&mut out[..kept]);
        total
    }

    /// Every overlap query: the solids on `mask` `probe` meets, ascending.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub(crate) fn overlap_probe(&self, probe: &Probe, mask: u16, out: &mut [u16]) -> usize {
        let (mut total, mut kept) = (0, 0);
        self.candidates(&probe.bounds(), |i| {
            let s = &self.solids[i];
            if s.takes(mask) && probe.hits(s) {
                total += 1;
                kept = insert_lowest(out, kept, i as u16);
            }
        });
        sort_small(&mut out[..kept]);
        total
    }

    /// Does anything on `mask` overlap `q`?
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn any_overlap(&self, q: &Aabb, mask: u16) -> bool {
        let mut hit = false;
        self.candidates(q, |i| {
            hit |= self.solids[i].takes(mask) && self.solids[i].overlaps_aabb(q);
        });
        hit
    }

    /// Nearest hit of `origin + dir * t` (`dir` normalized, `t <= max_t`)
    /// among the solids on `mask`; the lowest index wins ties. Walks the
    /// grid cell by cell (Amanatides-Woo) and stops once the next cell
    /// starts beyond the best hit.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_t: f32, mask: u16) -> Option<RayHit> {
        // Clip the ray to the grid's XZ extent.
        let (w, d) = (self.nx as f32 * self.cell, self.nz as f32 * self.cell);
        let (mut t0, mut t1) = (0.0f32, max_t);
        for (o, v, lo, hi) in [
            (origin.x, dir.x, self.ox, self.ox + w),
            (origin.z, dir.z, self.oz, self.oz + d),
        ] {
            if v.abs() < 1e-6 {
                if o < lo || o > hi {
                    return None;
                }
            } else {
                let (a, b) = ((lo - o) / v, (hi - o) / v);
                t0 = t0.max(a.min(b));
                t1 = t1.min(a.max(b));
            }
        }
        if t0 > t1 {
            return None;
        }
        let p = Vec3::new(origin.x + dir.x * t0, 0.0, origin.z + dir.z * t0);
        let mut cx = cell_of(p.x, self.ox, self.inv, self.nx);
        let mut cz = cell_of(p.z, self.oz, self.inv, self.nz);
        let axis = |o: f32, v: f32, c: usize, base: f32| -> (isize, f32, f32) {
            if v > 1e-6 {
                (
                    1,
                    ((c + 1) as f32 * self.cell + base - o) / v,
                    self.cell / v,
                )
            } else if v < -1e-6 {
                (-1, (c as f32 * self.cell + base - o) / v, self.cell / -v)
            } else {
                (0, f32::INFINITY, f32::INFINITY)
            }
        };
        let (sx, mut tx, dx) = axis(origin.x, dir.x, cx, self.ox);
        let (sz, mut tz, dz) = axis(origin.z, dir.z, cz, self.oz);
        let mut best: Option<RayHit> = None;
        loop {
            for &r in self.cell_refs(cx, cz) {
                let i = r as usize;
                let s = &self.solids[i];
                if !s.takes(mask) {
                    continue;
                }
                let limit = best.map_or(max_t, |h| h.t);
                if let Some((t, normal)) = ray_solid(origin, dir, limit, s)
                    && best.is_none_or(|h| t < h.t || (t == h.t && i < h.index))
                {
                    best = Some(RayHit {
                        t,
                        index: i,
                        normal,
                    });
                }
            }
            let exit = tx.min(tz);
            if best.is_some_and(|h| h.t <= exit) || exit > t1 {
                break;
            }
            if tx <= tz {
                let n = cx as isize + sx;
                if n < 0 || n >= self.nx as isize {
                    break;
                }
                cx = n as usize;
                tx += dx;
            } else {
                let n = cz as isize + sz;
                if n < 0 || n >= self.nz as isize {
                    break;
                }
                cz = n as usize;
                tz += dz;
            }
        }
        best
    }

    /// Sweep `shape` (centered at `from`) to `to` against the solids on
    /// `mask`. See [`shape_cast_solids`] for the semantics.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn shape_cast(
        &self,
        shape: CastShape,
        from: Vec3,
        to: Vec3,
        mask: u16,
    ) -> Option<ShapeHit> {
        let mut best: Option<ShapeHit> = None;
        let swept = shape.bounds(from).union(&shape.bounds(to));
        self.candidates(&swept, |i| {
            let s = &self.solids[i];
            if s.takes(mask) {
                merge_hit(&mut best, cast_solid(shape, from, to, s, i));
            }
        });
        best
    }
}

/// Inclusive cell range a box covers.
#[inline]
fn span(
    b: &Aabb,
    ox: f32,
    oz: f32,
    inv: f32,
    nx: usize,
    nz: usize,
) -> (usize, usize, usize, usize) {
    (
        cell_of(b.min.x, ox, inv, nx),
        cell_of(b.max.x, ox, inv, nx),
        cell_of(b.min.z, oz, inv, nz),
        cell_of(b.max.z, oz, inv, nz),
    )
}

/// Where the controller's [`step_in`](super::CharacterController::step_in)
/// finds its surfaces: a [`World`] or a streamed
/// [`CellWorld`](super::CellWorld).
pub trait SolidSource {
    /// Ids of the solids on `mask` whose boxes overlap `q`, ascending
    /// (the lowest kept); returns how many.
    fn gather(&self, q: &Aabb, mask: u16, out: &mut [u16]) -> usize;
    /// The solid with id `id` (one [`gather`](Self::gather) returned).
    fn solid_at(&self, id: usize) -> Solid;
}

impl SolidSource for World<'_> {
    fn gather(&self, q: &Aabb, mask: u16, out: &mut [u16]) -> usize {
        self.overlap_bounds(q, mask, out)
    }

    fn solid_at(&self, id: usize) -> Solid {
        let base = self.statics.len();
        if id < base {
            self.statics.solids()[id]
        } else {
            self.dynamics[id - base]
        }
    }
}

/// The level grid plus a small dynamic list (scanned linearly). Dynamic
/// solid `k` has index `statics.len() + k` in every result.
#[derive(Clone, Copy)]
pub struct World<'a> {
    pub statics: StaticWorld<'a>,
    pub dynamics: &'a [Solid],
}

impl World<'_> {
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_t: f32, mask: u16) -> Option<RayHit> {
        let mut best = self.statics.raycast(origin, dir, max_t, mask);
        let base = self.statics.len();
        for (k, s) in self.dynamics.iter().enumerate() {
            let limit = best.map_or(max_t, |h| h.t);
            if s.takes(mask)
                && let Some((t, normal)) = ray_solid(origin, dir, limit, s)
                && best.is_none_or(|h| t < h.t)
            {
                best = Some(RayHit {
                    t,
                    index: base + k,
                    normal,
                });
            }
        }
        best
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn shape_cast(
        &self,
        shape: CastShape,
        from: Vec3,
        to: Vec3,
        mask: u16,
    ) -> Option<ShapeHit> {
        let mut best = self.statics.shape_cast(shape, from, to, mask);
        let base = self.statics.len();
        for (k, s) in self.dynamics.iter().enumerate() {
            if s.takes(mask) {
                merge_hit(&mut best, cast_solid(shape, from, to, s, base + k));
            }
        }
        best
    }

    /// Solids overlapping `q` on `mask`, statics then dynamics, ascending
    /// (the lowest indices when `out` is short); returns how many overlap.
    /// Ramps count where their wedge does.
    pub fn overlap_aabb(&self, q: &Aabb, mask: u16, out: &mut [u16]) -> usize {
        self.overlap_probe(&Probe::Aabb(*q), mask, out)
    }

    /// [`overlap_aabb`](Self::overlap_aabb) by bounding boxes (a superset):
    /// the controller's candidates.
    pub fn overlap_bounds(&self, q: &Aabb, mask: u16, out: &mut [u16]) -> usize {
        self.overlap_probe(&Probe::Bounds(*q), mask, out)
    }

    /// Solids `shape` centered at `c` overlaps, exactly; ordered like
    /// [`overlap_aabb`](Self::overlap_aabb).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn overlap_shape(&self, shape: CastShape, c: Vec3, mask: u16, out: &mut [u16]) -> usize {
        let total = self.statics.overlap_shape(shape, c, mask, out);
        shape_dynamics(
            total,
            self.statics.len(),
            self.dynamics,
            shape,
            c,
            mask,
            out,
        )
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn overlap_probe(&self, probe: &Probe, mask: u16, out: &mut [u16]) -> usize {
        let total = self.statics.overlap_probe(probe, mask, out);
        dynamics_into(total, self.statics.len(), self.dynamics, probe, mask, out)
    }
}

/// [`dynamics_into`] for an exact shape overlap.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub(crate) fn shape_dynamics(
    mut total: usize,
    base: usize,
    dynamics: &[Solid],
    shape: CastShape,
    c: Vec3,
    mask: u16,
    out: &mut [u16],
) -> usize {
    let mut kept = total.min(out.len());
    for (k, s) in dynamics.iter().enumerate() {
        if s.takes(mask) && s.overlaps_shape(shape, c) {
            total += 1;
            if kept < out.len() {
                out[kept] = (base + k) as u16;
                kept += 1;
            }
        }
    }
    total
}

/// Append the dynamic solids `probe` meets (indices from `base`) to a
/// static query's `total` results in `out`.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub(crate) fn dynamics_into(
    mut total: usize,
    base: usize,
    dynamics: &[Solid],
    probe: &Probe,
    mask: u16,
    out: &mut [u16],
) -> usize {
    let mut kept = total.min(out.len());
    for (k, s) in dynamics.iter().enumerate() {
        if s.takes(mask) && probe.hits(s) {
            total += 1;
            if kept < out.len() {
                out[kept] = (base + k) as u16;
                kept += 1;
            }
        }
    }
    total
}

/// The shape of a swept volume. Sphere and capsule are exact against
/// boxes; they are not the "collide as their box" shapes of
/// [`Collider`](super::Collider).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CastShape {
    Sphere {
        radius: f32,
    },
    /// Y-aligned, `height` total including the caps, centered.
    Capsule {
        radius: f32,
        height: f32,
    },
    Box {
        half: Vec3,
    },
}

impl CastShape {
    /// Bounding box with the shape centered at `c`.
    pub fn bounds(&self, c: Vec3) -> Aabb {
        let half = match *self {
            CastShape::Sphere { radius } => Vec3::new(radius, radius, radius),
            CastShape::Capsule { radius, height } => {
                Vec3::new(radius, (height * 0.5).max(radius), radius)
            }
            CastShape::Box { half } => half,
        };
        Aabb::from_center_half(c, half)
    }

    /// Does the shape centered at `c` overlap `b`? (Touching does not.)
    pub fn overlaps(&self, c: Vec3, b: &Aabb) -> bool {
        match *self {
            CastShape::Box { half } => Aabb::from_center_half(c, half).overlaps(b),
            _ => {
                let (r, b) = self.round_vs(b);
                let d = c - clamp3(c, &b);
                d.dot(d) < r * r
            }
        }
    }

    /// A round shape's radius and half segment (0 for a sphere or a box).
    pub(crate) fn round(&self) -> (f32, f32) {
        match *self {
            CastShape::Sphere { radius } => (radius, 0.0),
            CastShape::Capsule { radius, height } => (radius, (height * 0.5 - radius).max(0.0)),
            CastShape::Box { .. } => (0.0, 0.0),
        }
    }

    /// For a round shape: its radius and `b` grown along Y by the
    /// capsule's half segment (a capsule is a sphere swept along it).
    fn round_vs(&self, b: &Aabb) -> (f32, Aabb) {
        match *self {
            CastShape::Sphere { radius } => (radius, *b),
            CastShape::Capsule { radius, height } => {
                let seg = (height * 0.5 - radius).max(0.0);
                let mut g = *b;
                g.min.y -= seg;
                g.max.y += seg;
                (radius, g)
            }
            CastShape::Box { .. } => (0.0, *b),
        }
    }
}

/// Where a swept shape first touches a solid.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ShapeHit {
    /// Distance travelled along `from → to` before contact (0 when the
    /// shape starts touching or inside).
    pub t: f32,
    /// `t` as a fraction of the whole move.
    pub fraction: f32,
    /// Which solid (statics, then dynamics).
    pub index: usize,
    /// Contact normal, pointing out of the solid (zero when `inside`).
    pub normal: Vec3,
    /// The shape already overlapped the solid at `from`.
    pub inside: bool,
}

fn clamp3(p: Vec3, b: &Aabb) -> Vec3 {
    Vec3::new(
        // A box with min past max (damaged data) must not panic.
        crate::math::clamp_or(p.x, b.min.x, b.max.x),
        crate::math::clamp_or(p.y, b.min.y, b.max.y),
        crate::math::clamp_or(p.z, b.min.z, b.max.z),
    )
}

pub(crate) fn normalize_or_zero(v: Vec3) -> Vec3 {
    let l = sqrt(v.dot(v));
    if l < 1e-9 { Vec3::ZERO } else { v * (1.0 / l) }
}

#[inline]
pub(crate) fn merge_hit(best: &mut Option<ShapeHit>, h: Option<ShapeHit>) {
    if let Some(h) = h
        && best.is_none_or(|b| h.t < b.t || (h.t == b.t && h.index < b.index))
    {
        *best = Some(h);
    }
}

/// Sweep `shape` from center `from` to `to` against `solids` (those on
/// `mask`): the earliest contact, the lowest index on ties. This is the
/// brute-force reference [`StaticWorld::shape_cast`] must equal.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn shape_cast_solids(
    shape: CastShape,
    from: Vec3,
    to: Vec3,
    mask: u16,
    solids: &[Solid],
) -> Option<ShapeHit> {
    let mut best = None;
    for (i, s) in solids.iter().enumerate() {
        if s.takes(mask) {
            merge_hit(&mut best, cast_solid(shape, from, to, s, i));
        }
    }
    best
}

/// One solid of any kind: a box (and a one-way box, for a shape that
/// starts at or above its top) by [`cast_one`]'s exact box test, a ramp as
/// its wedge.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn cast_solid(shape: CastShape, from: Vec3, to: Vec3, s: &Solid, index: usize) -> Option<ShapeHit> {
    match s.kind {
        SurfaceKind::Solid => cast_one(shape, from, to, &s.aabb, index),
        SurfaceKind::OneWay => {
            if one_way_takes(&shape, from, s.aabb.max.y) {
                cast_one(shape, from, to, &s.aabb, index)
            } else {
                None
            }
        }
        SurfaceKind::Ramp(dir) => cast_convex(
            shape,
            from,
            to,
            &|seg| Convex::wedge(&s.aabb, dir, seg),
            index,
        ),
    }
}

/// `shape` from `from` to `to` against a convex polytope `build(seg)`
/// returns (extruded by `seg` along Y for a capsule): the same contract as
/// [`cast_one`] (an `inside` start, else the earliest contact).
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub(crate) fn cast_convex(
    shape: CastShape,
    from: Vec3,
    to: Vec3,
    build: &dyn Fn(f32) -> Convex,
    index: usize,
) -> Option<ShapeHit> {
    let delta = to - from;
    let len = sqrt(delta.dot(delta));
    let dir = if len > 1e-9 {
        delta * (1.0 / len)
    } else {
        Vec3::ZERO
    };
    let hit = |t: f32, normal: Vec3, inside: bool| ShapeHit {
        t,
        fraction: if len > 1e-9 { t / len } else { 0.0 },
        index,
        normal,
        inside,
    };
    match shape {
        CastShape::Box { half } => match build(0.0).box_cast(from, half, dir, len) {
            Err(()) => Some(hit(0.0, Vec3::ZERO, true)),
            Ok(_) if len <= 1e-9 => None,
            Ok(h) => h.map(|(t, n)| hit(t, n, false)),
        },
        _ => {
            let (r, seg) = shape.round();
            let cv = build(seg);
            let (q, d2) = cv.closest(from);
            if d2 < r * r {
                return Some(hit(0.0, normalize_or_zero(from - q), true));
            }
            if len <= 1e-9 {
                return None;
            }
            cv.sphere_sweep(from, r, dir, len)
                .map(|(t, n)| hit(t, n, false))
        }
    }
}

/// Reference for tests and tools: solids on `mask` that `shape` centered
/// at `c` overlaps, ascending, into `out`; returns how many.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn overlap_shape_solids(
    shape: CastShape,
    c: Vec3,
    mask: u16,
    solids: &[Solid],
    out: &mut [u16],
) -> usize {
    let mut n = 0;
    for (i, s) in solids.iter().enumerate() {
        if s.takes(mask) && s.overlaps_shape(shape, c) {
            if n < out.len() {
                out[n] = i as u16;
            }
            n += 1;
        }
    }
    n
}

#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn cast_one(shape: CastShape, from: Vec3, to: Vec3, b: &Aabb, index: usize) -> Option<ShapeHit> {
    let delta = to - from;
    let len = sqrt(delta.dot(delta));
    let dir = if len > 1e-9 {
        delta * (1.0 / len)
    } else {
        Vec3::ZERO
    };
    let hit = |t: f32, normal: Vec3, inside: bool| ShapeHit {
        t,
        fraction: if len > 1e-9 { t / len } else { 0.0 },
        index,
        normal,
        inside,
    };
    if shape.overlaps(from, b) {
        // Out of the solid along the shortest way: from the closest point.
        let n = match shape {
            CastShape::Box { .. } => Vec3::ZERO,
            _ => {
                let (_, g) = shape.round_vs(b);
                normalize_or_zero(from - clamp3(from, &g))
            }
        };
        return Some(hit(0.0, n, true));
    }
    if len <= 1e-9 {
        return None;
    }
    match shape {
        CastShape::Box { half } => {
            let grown = Aabb::new(b.min - half, b.max + half);
            ray_box(from, dir, len, &grown).map(|(t, n)| hit(t, n, false))
        }
        _ => {
            let (r, g) = shape.round_vs(b);
            sphere_vs_box(from, r, dir, len, &g).map(|(t, n)| hit(t, n, false))
        }
    }
}

/// A sphere of radius `r` centered `c`, moving along `dir` (normalized)
/// up to `limit`, against a box it does not start inside: the Minkowski
/// sum is three face slabs, twelve edge cylinders and eight corner
/// spheres, and the earliest contact of those is the answer.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn sphere_vs_box(c: Vec3, r: f32, dir: Vec3, limit: f32, b: &Aabb) -> Option<(f32, Vec3)> {
    let best = core::cell::Cell::new(limit);
    let found = core::cell::Cell::new(false);
    let take = |t: f32| {
        if (0.0..=best.get()).contains(&t) {
            best.set(t);
            found.set(true);
        }
    };
    // Face slabs: the box grown by r along one axis.
    let e = |a: usize| {
        let mut g = *b;
        match a {
            0 => (g.min.x, g.max.x) = (g.min.x - r, g.max.x + r),
            1 => (g.min.y, g.max.y) = (g.min.y - r, g.max.y + r),
            _ => (g.min.z, g.max.z) = (g.min.z - r, g.max.z + r),
        }
        g
    };
    for a in 0..3 {
        if let Some((t, _)) = ray_box(c, dir, best.get(), &e(a)) {
            take(t);
        }
    }
    let get = |v: Vec3, a: usize| match a {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    };
    // Edge cylinders along axis `a`, through the box's other two extremes.
    for a in 0..3 {
        let (p, q) = ((a + 1) % 3, (a + 2) % 3);
        for pe in [get(b.min, p), get(b.max, p)] {
            for qe in [get(b.min, q), get(b.max, q)] {
                let (o1, o2) = (get(c, p) - pe, get(c, q) - qe);
                let (v1, v2) = (get(dir, p), get(dir, q));
                let a2 = v1 * v1 + v2 * v2;
                if a2 < 1e-12 {
                    continue;
                }
                let hb = o1 * v1 + o2 * v2;
                let disc = hb * hb - a2 * (o1 * o1 + o2 * o2 - r * r);
                if disc < 0.0 {
                    continue;
                }
                let t = (-hb - sqrt(disc)) / a2;
                let along = get(c, a) + get(dir, a) * t;
                if along >= get(b.min, a) && along <= get(b.max, a) {
                    take(t);
                }
            }
        }
    }
    // Corner spheres.
    for cx in [b.min.x, b.max.x] {
        for cy in [b.min.y, b.max.y] {
            for cz in [b.min.z, b.max.z] {
                let oc = c - Vec3::new(cx, cy, cz);
                let hb = oc.dot(dir);
                let disc = hb * hb - (oc.dot(oc) - r * r);
                if disc >= 0.0 {
                    take(-hb - sqrt(disc));
                }
            }
        }
    }
    if !found.get() {
        return None;
    }
    let best = best.get();
    let p = c + dir * best;
    Some((best, normalize_or_zero(p - clamp3(p, b))))
}

/// What happened between a body and a trigger this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TriggerPhase {
    Enter,
    Stay,
    Exit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TriggerEvent {
    pub body: u16,
    pub trigger: u16,
    pub phase: TriggerPhase,
}

/// Enter / Stay / Exit events for up to `N` (body, trigger) overlaps at a
/// time. Per frame: [`begin_frame`](Self::begin_frame), one
/// [`track`](Self::track) per body (or [`touch`](Self::touch) for overlaps
/// you found yourself), then [`end_frame`](Self::end_frame). Events come
/// out ordered by (body, trigger), so every platform sees the same list.
#[derive(Clone, Debug)]
pub struct TriggerTracker<const N: usize> {
    prev: [u32; N],
    prev_len: usize,
    cur: [u32; N],
    cur_len: usize,
    /// Overlaps dropped this frame because `N` was full.
    pub dropped: usize,
}

impl<const N: usize> Default for TriggerTracker<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> TriggerTracker<N> {
    pub const fn new() -> Self {
        TriggerTracker {
            prev: [0; N],
            prev_len: 0,
            cur: [0; N],
            cur_len: 0,
            dropped: 0,
        }
    }

    /// Start a frame's observations.
    pub fn begin_frame(&mut self) {
        self.cur_len = 0;
        self.dropped = 0;
    }

    /// Record that `body` overlaps `trigger` this frame.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn touch(&mut self, body: u16, trigger: u16) {
        let key = (body as u32) << 16 | trigger as u32;
        let live = &self.cur[..self.cur_len];
        let at = match live.binary_search(&key) {
            Ok(_) => return,
            Err(at) => at,
        };
        if self.cur_len == N {
            self.dropped += 1;
            return;
        }
        self.cur.copy_within(at..self.cur_len, at + 1);
        self.cur[at] = key;
        self.cur_len += 1;
    }

    /// Record every trigger of `triggers` on `mask` that `aabb` overlaps
    /// (a trigger's id is its index in the slice).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn track(&mut self, body: u16, aabb: &Aabb, triggers: &[Solid], mask: u16) {
        for (i, t) in triggers.iter().enumerate() {
            if t.takes(mask) && t.aabb.overlaps(aabb) {
                self.touch(body, i as u16);
            }
        }
    }

    /// As [`track`](Self::track) for a grid of triggers (a stadium's
    /// zones): only the cells `aabb` meets are visited.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn track_in(&mut self, body: u16, aabb: &Aabb, triggers: &StaticWorld, mask: u16) {
        let mut ids = [0u16; 32];
        let n = triggers.overlap_aabb(aabb, mask, &mut ids);
        for &id in &ids[..n.min(ids.len())] {
            self.touch(body, id);
        }
        self.dropped += n.saturating_sub(ids.len());
    }

    /// Close the frame: write the events (Enter for new overlaps, Stay for
    /// kept ones, Exit for ended ones) into `out`, return how many fit.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn end_frame(&mut self, out: &mut [TriggerEvent]) -> usize {
        let (mut i, mut j, mut n) = (0, 0, 0);
        let (prev, cur) = (&self.prev[..self.prev_len], &self.cur[..self.cur_len]);
        let mut push = |key: u32, phase| {
            if n < out.len() {
                out[n] = TriggerEvent {
                    body: (key >> 16) as u16,
                    trigger: key as u16,
                    phase,
                };
                n += 1;
            }
        };
        while i < prev.len() || j < cur.len() {
            if j == cur.len() || (i < prev.len() && prev[i] < cur[j]) {
                push(prev[i], TriggerPhase::Exit);
                i += 1;
            } else if i == prev.len() || cur[j] < prev[i] {
                push(cur[j], TriggerPhase::Enter);
                j += 1;
            } else {
                push(cur[j], TriggerPhase::Stay);
                i += 1;
                j += 1;
            }
        }
        self.prev[..self.cur_len].copy_from_slice(&self.cur[..self.cur_len]);
        self.prev_len = self.cur_len;
        n
    }
}

impl<const N: usize> TriggerTracker<N> {
    /// [`end_frame`](Self::end_frame) across a stream: a pair whose trigger
    /// is not `live` (its cell unloaded) and that nobody touched this frame
    /// is held silently instead of ending (no Exit). Back in a loaded cell,
    /// a body still inside it is a Stay; one that left meanwhile, an Exit.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn end_frame_held(
        &mut self,
        out: &mut [TriggerEvent],
        live: &dyn Fn(u16) -> bool,
    ) -> usize {
        for i in 0..self.prev_len {
            let key = self.prev[i];
            if !live(key as u16) {
                self.touch((key >> 16) as u16, key as u16);
            }
        }
        // Held pairs are now in both lists: a Stay, filtered out below.
        let n = self.end_frame(out);
        let mut k = 0;
        for i in 0..n {
            let e = out[i];
            if e.phase == TriggerPhase::Stay && !live(e.trigger) {
                continue;
            }
            out[k] = e;
            k += 1;
        }
        k
    }
}

/// Reference for tests and tools: indices of `solids` on `mask`
/// overlapping `q`, ascending, into `out`.
/// Returns how many overlap; `out` keeps the lowest indices.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn overlap_solids(q: &Aabb, mask: u16, solids: &[Solid], out: &mut [u16]) -> usize {
    let mut n = 0;
    for (i, s) in solids.iter().enumerate() {
        if s.takes(mask) && s.overlaps_aabb(q) {
            if n < out.len() {
                out[n] = i as u16;
            }
            n += 1;
        }
    }
    n
}

/// Insertion sort: query buffers are a few dozen ids, and it compiles to a
/// fraction of `sort_unstable`'s sorting network (the N64's RAM).
fn sort_small(v: &mut [u16]) {
    for i in 1..v.len() {
        let x = v[i];
        let mut j = i;
        while j > 0 && v[j - 1] > x {
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = x;
    }
}

/// Keep the `out.len()` lowest values seen so far in `out[..kept]`
/// (unordered): returns the new `kept`.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn insert_lowest(out: &mut [u16], kept: usize, v: u16) -> usize {
    if kept < out.len() {
        out[kept] = v;
        return kept + 1;
    }
    if let Some((at, &max)) = out.iter().enumerate().max_by_key(|&(_, x)| *x)
        && v < max
    {
        out[at] = v;
    }
    kept
}

/// Reference raycast with a layer mask: [`super::raycast`] over the solids
/// on `mask`, lowest index on ties.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn raycast_solids(
    origin: Vec3,
    dir: Vec3,
    max_t: f32,
    mask: u16,
    solids: &[Solid],
) -> Option<RayHit> {
    let mut best: Option<RayHit> = None;
    for (index, s) in solids.iter().enumerate() {
        if !s.takes(mask) {
            continue;
        }
        let limit = best.map_or(max_t, |h| h.t);
        if let Some((t, normal)) = ray_solid(origin, dir, limit, s)
            && best.is_none_or(|h| t < h.t)
        {
            best = Some(RayHit { t, index, normal });
        }
    }
    best
}

/// A ray against one solid: its box, or a ramp's wedge (box under the
/// slope, entered through the slope with the slope's normal).
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub(crate) fn ray_solid(origin: Vec3, dir: Vec3, limit: f32, s: &Solid) -> Option<(f32, Vec3)> {
    let SurfaceKind::Ramp(rd) = s.kind else {
        return ray_box(origin, dir, limit, &s.aabb);
    };
    let b = &s.aabb;
    let (run, horizontal_lo, horizontal_hi, along_x) = match rd {
        RampDir::PosX | RampDir::NegX => (b.max.x - b.min.x, b.min.x, b.max.x, true),
        RampDir::PosZ | RampDir::NegZ => (b.max.z - b.min.z, b.min.z, b.max.z, false),
    };
    let rise = b.max.y - b.min.y;
    // Under the slope: (y - min.y)·run <= u·rise, u from the low side.
    let (k, c) = match rd {
        RampDir::PosX | RampDir::PosZ => (-rise, run * b.min.y - rise * horizontal_lo),
        RampDir::NegX | RampDir::NegZ => (rise, run * b.min.y + rise * horizontal_hi),
    };
    let slope_n = if along_x {
        Vec3::new(k, run, 0.0)
    } else {
        Vec3::new(0.0, run, k)
    };
    let planes = [
        (Vec3::new(-1.0, 0.0, 0.0), -b.min.x),
        (Vec3::new(1.0, 0.0, 0.0), b.max.x),
        (Vec3::new(0.0, -1.0, 0.0), -b.min.y),
        (Vec3::new(0.0, 1.0, 0.0), b.max.y),
        (Vec3::new(0.0, 0.0, -1.0), -b.min.z),
        (Vec3::new(0.0, 0.0, 1.0), b.max.z),
        (slope_n, c),
    ];
    let (mut t_enter, mut t_exit, mut normal) = (0.0f32, limit, Vec3::ZERO);
    for (n, d) in planes {
        let denom = n.dot(dir);
        let dist = d - n.dot(origin);
        if denom.abs() < 1e-9 {
            if dist < 0.0 {
                return None;
            }
            continue;
        }
        let t = dist / denom;
        if denom < 0.0 {
            if t > t_enter {
                t_enter = t;
                normal = n;
            }
        } else {
            t_exit = t_exit.min(t);
        }
        if t_enter > t_exit {
            return None;
        }
    }
    let l = sqrt(normal.dot(normal));
    let normal = if l > 0.0 { normal * (1.0 / l) } else { normal };
    Some((t_enter, normal))
}

pub const LAYERS_MAGIC: &[u8; 4] = b"TPHY";
pub const LAYERS_VERSION: u16 = 1;

/// The project's layer matrix (`physics/layers` blob, from
/// `assets/physics.ron`): which layers each layer collides with.
///
/// ```text
/// magic "TPHY"  u16 version (1)  u16 layer_count  u16 mask per layer
/// ```
#[derive(Clone, Copy, Debug)]
pub struct LayerMatrix<'a> {
    data: &'a [u8],
    n: usize,
}

impl<'a> LayerMatrix<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < 8 || &data[..4] != LAYERS_MAGIC {
            return None;
        }
        if u16::from_le_bytes([data[4], data[5]]) != LAYERS_VERSION {
            return None;
        }
        let n = u16::from_le_bytes([data[6], data[7]]) as usize;
        (n <= 16 && data.len() >= 8 + n * 2).then_some(LayerMatrix { data, n })
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The mask a body on layer `index` (bit `1 << index`) queries with
    /// (every layer for an unknown index).
    pub fn mask(&self, index: usize) -> u16 {
        if index >= self.n {
            return LAYER_ALL;
        }
        let at = 8 + index * 2;
        u16::from_le_bytes([self.data[at], self.data[at + 1]])
    }

    /// The mask for a body whose layer bits are `bits` (the union).
    pub fn mask_for(&self, bits: u16) -> u16 {
        (0..16)
            .filter(|i| bits & (1 << i) != 0)
            .fold(0, |m, i| m | self.mask(i))
    }
}
