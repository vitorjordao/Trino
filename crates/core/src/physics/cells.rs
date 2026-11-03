//! Physics per cell (issue #115): a streamed level's solids, triggers and
//! mesh colliders load and unload with their cell (ADR 0022's
//! [`CellGroups`](crate::vis::CellGroups)), and the grid of each cell can
//! come baked in the scene (TSCN v9's physics section) instead of being
//! built at load.
//!
//! - **[`CellPart`]**: one cell's solids (gridded), triggers and meshes. A
//!   part's solids keep **fixed global indices** (`base + k`) whatever else
//!   is loaded, so a trigger key or a `standing_on` never shifts when a
//!   neighbour streams in. Parts ascend by cell; [`NO_CELL`] (the level
//!   outside any cell) is always loaded.
//! - **[`CellWorld`]**: the parts, a mask of loaded cells and a dynamic
//!   list. Every query visits the loaded parts only and answers what a scan
//!   of their solids (in global order) answers: the lowest index on ties,
//!   ascending overlaps, the lowest indices kept in a short buffer.
//! - **Triggers** across a stream: [`TriggerTracker::end_frame_held`] keeps
//!   the pairs of a trigger whose cell unloaded under its body (no Exit),
//!   and the pair is a Stay again when the cell comes back with the body
//!   still inside.
//! - **Ground** across a stream: a body standing on an unloaded cell's
//!   floor loses it and falls. Games keep the cells under their bodies
//!   loaded ([`CellWorld::cells_under`]) with what their groups stream.
//!
//! The section (little-endian, after the scene's data table, ADR 0005 §3):
//!
//! ```text
//! u16 part_count  u16 reserved  u32 solids  u32 triggers  u32 meshes  u32 u16s
//! solids, then triggers: f32*3 min, f32*3 max, u16 layer, u8 kind
//!   (0 box, 1 one-way, 2 ramp), u8 ramp direction          (28 bytes each)
//! meshes: u8 cell, u8*3 reserved, u32 node                 (8 bytes each)
//! parts: u8 cell, u8 gridded, u16 reserved, u32 solid base, u32 solid count,
//!   u32 trigger base, u32 trigger count, u32 mesh base, u32 mesh count,
//!   f32 cell size, f32 origin x, f32 origin z, u16 nx, u16 nz,
//!   u32 starts (u16 offset), u32 refs (u16 offset), u32 refs length
//!                                                           (56 bytes each)
//! u16 table: every part's grid starts and refs
//! ```

use super::controller::{RampDir, SurfaceKind};
use super::scalar::MeshQuery;
use super::world::{Probe, dynamics_into, shape_dynamics};
use super::{
    Aabb, CastShape, Collider, MeshInstance, RayHit, ShapeHit, Solid, StaticWorld, TriggerTracker,
    merge_hit,
};
use crate::math::Vec3;
use crate::math3d::Mat34;
use crate::scene::TscnView;
use crate::vis::NO_CELL;

/// One cell's static physics. See the module docs.
#[derive(Clone, Copy)]
pub struct CellPart<'a> {
    /// The cell, or [`NO_CELL`] for what is in none (always loaded).
    pub cell: u8,
    /// Global index of `solids[0]`.
    pub base: u16,
    /// The solids, gridded (`None`: the cell has none).
    pub grid: Option<StaticWorld<'a>>,
    /// Global id of `triggers[0]` (trigger ids are their own space).
    pub trigger_base: u16,
    pub triggers: &'a [Solid],
}

impl CellPart<'_> {
    pub fn solids(&self) -> &[Solid] {
        self.grid.as_ref().map_or(&[], |g| g.solids())
    }

    fn len(&self) -> usize {
        self.grid.as_ref().map_or(0, |g| g.len())
    }

    /// The bounds of everything in the part (`None` when empty).
    pub fn bounds(&self) -> Option<Aabb> {
        self.solids()
            .iter()
            .chain(self.triggers)
            .map(|s| s.aabb)
            .reduce(|a, b| a.union(&b))
    }
}

/// A mesh collider of a cell.
#[derive(Clone, Copy)]
pub struct CellMesh<'a> {
    pub cell: u8,
    pub mesh: MeshInstance<'a>,
}

/// A streamed level's physics: its [`CellPart`]s, which cells are loaded,
/// and a dynamic list (scanned; indices after every part's). See the
/// module docs.
#[derive(Clone, Copy)]
pub struct CellWorld<'a> {
    parts: &'a [CellPart<'a>],
    meshes: &'a [CellMesh<'a>],
    pub dynamics: &'a [Solid],
    loaded: u64,
    end: usize,
}

#[inline]
fn cell_bit(cell: u8) -> u64 {
    if (cell as usize) < 64 {
        1u64 << cell
    } else {
        0
    }
}

impl<'a> CellWorld<'a> {
    /// `None` when the parts' index ranges overlap or don't ascend.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn new(
        parts: &'a [CellPart<'a>],
        meshes: &'a [CellMesh<'a>],
        dynamics: &'a [Solid],
    ) -> Option<Self> {
        let (mut next, mut next_t) = (0usize, 0usize);
        for p in parts {
            if (p.base as usize) < next || (p.trigger_base as usize) < next_t {
                return None;
            }
            next = p.base as usize + p.len();
            next_t = p.trigger_base as usize + p.triggers.len();
        }
        (next + dynamics.len() <= u16::MAX as usize).then_some(CellWorld {
            parts,
            meshes,
            dynamics,
            loaded: 0,
            end: next,
        })
    }

    /// The cells whose physics takes part, as bits (cell `c` = `1 << c`).
    pub fn set_loaded(&mut self, cells: u64) {
        self.loaded = cells;
    }

    pub fn loaded(&self) -> u64 {
        self.loaded
    }

    pub fn parts(&self) -> &'a [CellPart<'a>] {
        self.parts
    }

    #[inline]
    fn live(&self, cell: u8) -> bool {
        cell == NO_CELL || self.loaded & cell_bit(cell) != 0
    }

    /// Global index of `dynamics[0]`.
    pub fn dynamics_base(&self) -> usize {
        self.end
    }

    /// The solid with global index `id`, if its cell is loaded.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn solid(&self, id: usize) -> Option<&'a Solid> {
        if id >= self.end {
            return self.dynamics.get(id - self.end);
        }
        let p = self
            .parts
            .iter()
            .find(|p| id >= p.base as usize && id < p.base as usize + p.len())?;
        self.live(p.cell).then(|| &p.solids()[id - p.base as usize])
    }

    /// Is trigger `id` in a loaded cell (a tracker pair of an unloaded one
    /// is held, not ended)?
    pub fn trigger_live(&self, id: u16) -> bool {
        self.parts
            .iter()
            .find(|p| {
                id as usize >= p.trigger_base as usize
                    && (id as usize) < p.trigger_base as usize + p.triggers.len()
            })
            .is_none_or(|p| self.live(p.cell))
    }

    /// The cells (bits) whose physics meets `q`: what a game keeps loaded
    /// under its bodies.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn cells_under(&self, q: &Aabb) -> u64 {
        let mut bits = 0;
        for p in self.parts {
            if p.cell != NO_CELL
                && p.solids()
                    .iter()
                    .chain(p.triggers)
                    .any(|s| touches(&s.aabb, q))
            {
                bits |= cell_bit(p.cell);
            }
        }
        for m in self.meshes {
            if m.cell != NO_CELL && touches(&m.mesh.bounds(), q) {
                bits |= cell_bit(m.cell);
            }
        }
        bits
    }

    /// The loaded cells' meshes, in order, into `out`; returns how many.
    pub fn meshes(&self, out: &mut [&'a dyn MeshQuery<f32>]) -> usize {
        let mut n = 0;
        for m in self.meshes {
            if self.live(m.cell) && n < out.len() {
                out[n] = &m.mesh;
                n += 1;
            }
        }
        n
    }

    /// Nearest hit among the loaded solids on `mask`; the lowest global
    /// index wins ties.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_t: f32, mask: u16) -> Option<RayHit> {
        let mut best: Option<RayHit> = None;
        for p in self.parts {
            let Some(g) = p.grid.filter(|_| self.live(p.cell)) else {
                continue;
            };
            let limit = best.map_or(max_t, |h| h.t);
            if let Some(mut h) = g.raycast(origin, dir, limit, mask)
                && best.is_none_or(|b| h.t < b.t)
            {
                h.index += p.base as usize;
                best = Some(h);
            }
        }
        for (k, s) in self.dynamics.iter().enumerate() {
            let limit = best.map_or(max_t, |h| h.t);
            if s.layer & mask != 0
                && let Some((t, normal)) = super::world::ray_solid(origin, dir, limit, s)
                && best.is_none_or(|h| t < h.t)
            {
                best = Some(RayHit {
                    t,
                    index: self.end + k,
                    normal,
                });
            }
        }
        best
    }

    /// Sweep `shape` against the loaded solids (as [`World::shape_cast`](super::World::shape_cast)).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn shape_cast(
        &self,
        shape: CastShape,
        from: Vec3,
        to: Vec3,
        mask: u16,
    ) -> Option<ShapeHit> {
        let mut best: Option<ShapeHit> = None;
        for p in self.parts {
            if let Some(g) = p.grid.filter(|_| self.live(p.cell)) {
                let h = g.shape_cast(shape, from, to, mask).map(|mut h| {
                    h.index += p.base as usize;
                    h
                });
                merge_hit(&mut best, h);
            }
        }
        let d =
            super::world::shape_cast_solids(shape, from, to, mask, self.dynamics).map(|mut h| {
                h.index += self.end;
                h
            });
        merge_hit(&mut best, d);
        best
    }

    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    fn overlap_probe(&self, probe: &Probe, mask: u16, out: &mut [u16]) -> usize {
        let (mut total, mut kept) = (0usize, 0usize);
        for p in self.parts {
            let Some(g) = p.grid.filter(|_| self.live(p.cell)) else {
                continue;
            };
            let n = g.overlap_probe(probe, mask, &mut out[kept..]);
            let k = n.min(out.len() - kept);
            for o in &mut out[kept..kept + k] {
                *o += p.base;
            }
            kept += k;
            total += n;
        }
        let _ = kept;
        dynamics_into(total, self.end, self.dynamics, probe, mask, out)
    }

    /// Loaded solids overlapping `q` on `mask`, ascending global indices
    /// (the lowest when `out` is short); returns how many.
    pub fn overlap_aabb(&self, q: &Aabb, mask: u16, out: &mut [u16]) -> usize {
        self.overlap_probe(&Probe::Aabb(*q), mask, out)
    }

    /// By bounding boxes (the controller's candidates).
    pub fn overlap_bounds(&self, q: &Aabb, mask: u16, out: &mut [u16]) -> usize {
        self.overlap_probe(&Probe::Bounds(*q), mask, out)
    }

    /// Exact shape overlaps.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn overlap_shape(&self, shape: CastShape, c: Vec3, mask: u16, out: &mut [u16]) -> usize {
        let (mut total, mut kept) = (0usize, 0usize);
        for p in self.parts {
            let Some(g) = p.grid.filter(|_| self.live(p.cell)) else {
                continue;
            };
            let n = g.overlap_shape(shape, c, mask, &mut out[kept..]);
            let k = n.min(out.len() - kept);
            for o in &mut out[kept..kept + k] {
                *o += p.base;
            }
            kept += k;
            total += n;
        }
        shape_dynamics(total, self.end, self.dynamics, shape, c, mask, out)
    }

    /// Record the loaded triggers on `mask` that `aabb` overlaps for `body`
    /// (trigger ids are global).
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn track<const N: usize>(
        &self,
        tracker: &mut TriggerTracker<N>,
        body: u16,
        aabb: &Aabb,
        mask: u16,
    ) {
        for p in self.parts {
            if !self.live(p.cell) {
                continue;
            }
            for (k, t) in p.triggers.iter().enumerate() {
                if t.layer & mask != 0 && t.aabb.overlaps(aabb) {
                    tracker.touch(body, p.trigger_base + k as u16);
                }
            }
        }
    }
}

/// Inclusive overlap.
fn touches(a: &Aabb, b: &Aabb) -> bool {
    a.min.x <= b.max.x
        && b.min.x <= a.max.x
        && a.min.y <= b.max.y
        && b.min.y <= a.max.y
        && a.min.z <= b.max.z
        && b.min.z <= a.max.z
}

impl super::SolidSource for CellWorld<'_> {
    fn gather(&self, q: &Aabb, mask: u16, out: &mut [u16]) -> usize {
        self.overlap_bounds(q, mask, out)
    }

    fn solid_at(&self, id: usize) -> Solid {
        // Gathered ids are loaded.
        *self.solid(id).expect("a gathered solid is loaded")
    }
}

// ------------------------------------------------- gathering and building ----

/// What a scene node contributes to its cell's physics.
pub enum NodePhysics {
    Solid(Solid),
    Trigger(Solid),
    /// A mesh collider (its blob and local bounds are the node's).
    Mesh,
}

/// A node's collider in the world (`None`: no collider).
pub fn node_physics(collider: &Collider, world: &Mat34) -> NodePhysics {
    if collider.trigger {
        return NodePhysics::Trigger(collider.world_trigger(world).expect("a trigger"));
    }
    match collider.world_solid(world) {
        Some(s) => NodePhysics::Solid(s),
        None => NodePhysics::Mesh,
    }
}

/// What the gathering found (counts; buffers keep what fits).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Gathered {
    pub solids: usize,
    pub triggers: usize,
    pub meshes: usize,
}

/// Gather a scene's physics from its `count` nodes (`node(i)`: the cell,
/// collider and world matrix of a node with a collider) into caller buffers, each sorted by cell afterwards: the
/// solids, the triggers and the mesh colliders' `(cell, node)` records.
/// The bake and a scene without a baked section both run it.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn gather_nodes(
    count: usize,
    node: &dyn Fn(usize) -> Option<(u8, Collider, Mat34)>,
    solids: &mut [Solid],
    solid_cells: &mut [u8],
    triggers: &mut [Solid],
    trigger_cells: &mut [u8],
    meshes: &mut [(u8, u32)],
) -> Gathered {
    let mut g = Gathered::default();
    for i in 0..count {
        let Some((cell, collider, world)) = node(i) else {
            continue;
        };
        let node = i as u32;
        match node_physics(&collider, &world) {
            NodePhysics::Solid(s) => {
                if g.solids < solids.len().min(solid_cells.len()) {
                    solids[g.solids] = s;
                    solid_cells[g.solids] = cell;
                }
                g.solids += 1;
            }
            NodePhysics::Trigger(s) => {
                if g.triggers < triggers.len().min(trigger_cells.len()) {
                    triggers[g.triggers] = s;
                    trigger_cells[g.triggers] = cell;
                }
                g.triggers += 1;
            }
            NodePhysics::Mesh => {
                if g.meshes < meshes.len() {
                    meshes[g.meshes] = (cell, node);
                }
                g.meshes += 1;
            }
        }
    }
    let (ns, nt, nm) = (
        g.solids.min(solids.len()).min(solid_cells.len()),
        g.triggers.min(triggers.len()).min(trigger_cells.len()),
        g.meshes.min(meshes.len()),
    );
    sort_by_cell(&mut solids[..ns], &mut solid_cells[..ns]);
    sort_by_cell(&mut triggers[..nt], &mut trigger_cells[..nt]);
    // Meshes by cell, stably.
    for i in 1..nm {
        let x = meshes[i];
        let mut j = i;
        while j > 0 && meshes[j - 1].0 > x.0 {
            meshes[j] = meshes[j - 1];
            j -= 1;
        }
        meshes[j] = x;
    }
    g
}

/// The grid cell size the bake and a scene's load pick for a cell's
/// `solids`: about 1.4 × the spacing of the solids over their XZ extent,
/// within 4..64 m, doubled until the grid fits its `u16` buffers.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn grid_cell_size(solids: &[Solid]) -> f32 {
    let Some(first) = solids.first() else {
        return 8.0;
    };
    let mut b = first.aabb;
    for s in solids {
        b = b.union(&s.aabb);
    }
    let area = ((b.max.x - b.min.x) * (b.max.z - b.min.z)).max(1.0);
    let mut cell = (crate::math3d::sqrt(area / solids.len() as f32) * 1.4).clamp(4.0, 64.0);
    // Whole metres: the same value from every machine's arithmetic.
    cell = crate::math3d::to_i32(cell) as f32;
    while StaticWorld::size(solids, cell).is_none() && cell < 65536.0 {
        cell *= 2.0;
    }
    cell
}

/// Sort `items` by `cells` (stably; scenes list a cell's nodes together, so
/// this is near linear).
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn sort_by_cell<T: Copy>(items: &mut [T], cells: &mut [u8]) {
    for i in 1..items.len().min(cells.len()) {
        let (x, c) = (items[i], cells[i]);
        let mut j = i;
        while j > 0 && cells[j - 1] > c {
            items[j] = items[j - 1];
            cells[j] = cells[j - 1];
            j -= 1;
        }
        items[j] = x;
        cells[j] = c;
    }
}

/// `u16`s the grids of `solids` (sorted by `cells`) need at `cell_size`,
/// or `None` when a cell's grid would not fit.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn grids_size(solids: &[Solid], cells: &[u8], cell_size: f32) -> Option<usize> {
    let mut n = 0;
    for (a, b) in runs(cells) {
        let s = &solids[a..b];
        let cs = if cell_size > 0.0 {
            cell_size
        } else {
            grid_cell_size(s)
        };
        let s = StaticWorld::size(s, cs)?;
        n += s.starts + s.refs;
    }
    Some(n)
}

/// Runs of equal cells: (start, end).
fn runs(cells: &[u8]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut i = 0;
    core::iter::from_fn(move || {
        if i >= cells.len() {
            return None;
        }
        let a = i;
        while i < cells.len() && cells[i] == cells[a] {
            i += 1;
        }
        Some((a, i))
    })
}

/// Distinct cells of `solids` and `triggers` (both sorted by cell), in
/// ascending order: the parts [`build_parts`] makes.
pub fn part_count(solid_cells: &[u8], trigger_cells: &[u8]) -> usize {
    let mut seen = [false; 256];
    let mut n = 0;
    for &c in solid_cells.iter().chain(trigger_cells) {
        if !seen[c as usize] {
            seen[c as usize] = true;
            n += 1;
        }
    }
    n
}

/// Build one part per cell (ascending; [`NO_CELL`] last) from solids and
/// triggers already sorted by cell, gridding each cell's solids at
/// `cell_size` (`0`: [`grid_cell_size`] per cell) into `u16s` (length from
/// [`grids_size`]). What a scene
/// without a baked section does at load; the bake does the same and writes
/// the result.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn build_parts<'b>(
    solids: &'b [Solid],
    solid_cells: &[u8],
    triggers: &'b [Solid],
    trigger_cells: &[u8],
    cell_size: f32,
    u16s: &'b mut [u16],
    parts: &'b mut [CellPart<'b>],
) -> Option<&'b [CellPart<'b>]> {
    let mut rest: &'b mut [u16] = u16s;
    let (mut si, mut ti, mut n) = (0usize, 0usize, 0usize);
    while si < solids.len() || ti < triggers.len() {
        let cell = match (solid_cells.get(si), trigger_cells.get(ti)) {
            (Some(&a), Some(&b)) => a.min(b),
            (Some(&a), None) => a,
            (None, Some(&b)) => b,
            (None, None) => break,
        };
        let s_end = si + solid_cells[si..].iter().take_while(|&&c| c == cell).count();
        let t_end = ti
            + trigger_cells[ti..]
                .iter()
                .take_while(|&&c| c == cell)
                .count();
        let grid = if s_end > si {
            let s = &solids[si..s_end];
            let cs = if cell_size > 0.0 {
                cell_size
            } else {
                grid_cell_size(s)
            };
            let need = StaticWorld::size(s, cs)?;
            if rest.len() < need.starts + need.refs {
                return None;
            }
            let (st, r) = core::mem::take(&mut rest).split_at_mut(need.starts);
            let (rf, r) = r.split_at_mut(need.refs);
            rest = r;
            Some(StaticWorld::build(s, cs, st, rf)?)
        } else {
            None
        };
        *parts.get_mut(n)? = CellPart {
            cell,
            base: si as u16,
            grid,
            trigger_base: ti as u16,
            triggers: &triggers[ti..t_end],
        };
        n += 1;
        si = s_end;
        ti = t_end;
    }
    let parts: &'b [CellPart<'b>] = parts;
    Some(&parts[..n])
}

// ---------------------------------------------------------------- section ----

const SECTION_HEADER: usize = 20;
const SOLID_BYTES: usize = 28;
const MESH_BYTES: usize = 8;
const PART_BYTES: usize = 56;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SectionError {
    Truncated,
    BadPart,
    BadGrid,
    /// A caller buffer is too small.
    Buffer,
}

/// Bytes [`write_section`] writes.
pub fn section_bytes(parts: &[CellPart], meshes: usize) -> usize {
    let solids: usize = parts.iter().map(|p| p.len()).sum();
    let triggers: usize = parts.iter().map(|p| p.triggers.len()).sum();
    let u16s: usize = parts
        .iter()
        .filter_map(|p| p.grid.as_ref())
        .map(|g| {
            let (_, _, _, _, _, st, rf) = g.raw();
            st.len() + rf.len()
        })
        .sum();
    SECTION_HEADER
        + (solids + triggers) * SOLID_BYTES
        + meshes * MESH_BYTES
        + parts.len() * PART_BYTES
        + u16s * 2
}

fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_f32(b: &mut [u8], at: usize, v: f32) {
    put_u32(b, at, v.to_bits());
}
fn get_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn get_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn get_f32(b: &[u8], at: usize) -> f32 {
    f32::from_bits(get_u32(b, at))
}

fn put_solid(b: &mut [u8], at: usize, s: &Solid) {
    for (k, v) in [
        s.aabb.min.x,
        s.aabb.min.y,
        s.aabb.min.z,
        s.aabb.max.x,
        s.aabb.max.y,
        s.aabb.max.z,
    ]
    .into_iter()
    .enumerate()
    {
        put_f32(b, at + k * 4, v);
    }
    put_u16(b, at + 24, s.layer);
    let (kind, dir) = match s.kind {
        SurfaceKind::Solid => (0, 0),
        SurfaceKind::OneWay => (1, 0),
        SurfaceKind::Ramp(d) => (2, d.code()),
    };
    b[at + 26] = kind;
    b[at + 27] = dir;
}

fn get_solid(b: &[u8], at: usize) -> Solid {
    let v = |k: usize| get_f32(b, at + k * 4);
    let aabb = Aabb::new(Vec3::new(v(0), v(1), v(2)), Vec3::new(v(3), v(4), v(5)));
    let kind = match b[at + 26] {
        1 => SurfaceKind::OneWay,
        2 => SurfaceKind::Ramp(RampDir::from_code(b[at + 27])),
        _ => SurfaceKind::Solid,
    };
    Solid {
        aabb,
        layer: get_u16(b, at + 24),
        kind,
    }
}

/// Write the section for `parts` and the mesh records `(cell, node)` into
/// `out` (length from [`section_bytes`]); returns the bytes written.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn write_section(
    parts: &[CellPart],
    meshes: &[(u8, u32)],
    out: &mut [u8],
) -> Result<usize, SectionError> {
    let need = section_bytes(parts, meshes.len());
    if out.len() < need {
        return Err(SectionError::Buffer);
    }
    let solids: usize = parts.iter().map(|p| p.len()).sum();
    let triggers: usize = parts.iter().map(|p| p.triggers.len()).sum();
    let u16s = (need
        - SECTION_HEADER
        - (solids + triggers) * SOLID_BYTES
        - meshes.len() * MESH_BYTES
        - parts.len() * PART_BYTES)
        / 2;
    put_u16(out, 0, parts.len() as u16);
    put_u16(out, 2, 0);
    put_u32(out, 4, solids as u32);
    put_u32(out, 8, triggers as u32);
    put_u32(out, 12, meshes.len() as u32);
    put_u32(out, 16, u16s as u32);
    let mut at = SECTION_HEADER;
    for p in parts {
        for s in p.solids() {
            put_solid(out, at, s);
            at += SOLID_BYTES;
        }
    }
    for p in parts {
        for s in p.triggers {
            put_solid(out, at, s);
            at += SOLID_BYTES;
        }
    }
    for &(cell, node) in meshes {
        out[at] = cell;
        out[at + 1..at + 4].fill(0);
        put_u32(out, at + 4, node);
        at += MESH_BYTES;
    }
    let table = at + parts.len() * PART_BYTES;
    let mut u = 0usize;
    for p in parts {
        out[at] = p.cell;
        out[at + 1] = p.grid.is_some() as u8;
        put_u16(out, at + 2, 0);
        put_u32(out, at + 4, p.base as u32);
        put_u32(out, at + 8, p.len() as u32);
        put_u32(out, at + 12, p.trigger_base as u32);
        put_u32(out, at + 16, p.triggers.len() as u32);
        put_u32(out, at + 20, 0);
        put_u32(out, at + 24, 0);
        out[at + 28..at + PART_BYTES].fill(0);
        if let Some(g) = &p.grid {
            let (ox, oz, cell, nx, nz, st, rf) = g.raw();
            put_f32(out, at + 28, cell);
            put_f32(out, at + 32, ox);
            put_f32(out, at + 36, oz);
            put_u16(out, at + 40, nx as u16);
            put_u16(out, at + 42, nz as u16);
            put_u32(out, at + 44, u as u32);
            put_u32(out, at + 48, (u + st.len()) as u32);
            put_u32(out, at + 52, rf.len() as u32);
            for &v in st.iter().chain(rf) {
                put_u16(out, table + u * 2, v);
                u += 1;
            }
        }
        at += PART_BYTES;
    }
    Ok(need)
}

/// A parsed physics section (borrowed, validated up front).
#[derive(Clone, Copy, Debug)]
pub struct BakedPhysics<'a> {
    data: &'a [u8],
    parts: usize,
    solids: usize,
    triggers: usize,
    meshes: usize,
    u16s: usize,
}

impl<'a> BakedPhysics<'a> {
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn parse(data: &'a [u8]) -> Result<Self, SectionError> {
        if data.len() < SECTION_HEADER {
            return Err(SectionError::Truncated);
        }
        let b = BakedPhysics {
            data,
            parts: get_u16(data, 0) as usize,
            solids: get_u32(data, 4) as usize,
            triggers: get_u32(data, 8) as usize,
            meshes: get_u32(data, 12) as usize,
            u16s: get_u32(data, 16) as usize,
        };
        let need = (b.solids as u64 + b.triggers as u64) * SOLID_BYTES as u64
            + b.meshes as u64 * MESH_BYTES as u64
            + b.parts as u64 * PART_BYTES as u64
            + b.u16s as u64 * 2
            + SECTION_HEADER as u64;
        if (data.len() as u64) < need {
            return Err(SectionError::Truncated);
        }
        let (mut next, mut next_t) = (0usize, 0usize);
        for i in 0..b.parts {
            let at = b.part_at(i);
            let (base, len) = (
                get_u32(data, at + 4) as usize,
                get_u32(data, at + 8) as usize,
            );
            let (tb, tl) = (
                get_u32(data, at + 12) as usize,
                get_u32(data, at + 16) as usize,
            );
            if base != next || tb != next_t || (data[at + 1] != 0) != (len > 0) {
                return Err(SectionError::BadPart);
            }
            next = base + len;
            next_t = tb + tl;
            if data[at + 1] != 0 {
                let (nx, nz) = (
                    get_u16(data, at + 40) as usize,
                    get_u16(data, at + 42) as usize,
                );
                let (st, rf, rl) = (
                    get_u32(data, at + 44) as usize,
                    get_u32(data, at + 48) as usize,
                    get_u32(data, at + 52) as usize,
                );
                let cells = nx * nz;
                if nx == 0 || nz == 0 || rf != st + cells + 1 || rf + rl > b.u16s {
                    return Err(SectionError::BadGrid);
                }
            }
        }
        if next != b.solids || next_t != b.triggers {
            return Err(SectionError::BadPart);
        }
        Ok(b)
    }

    fn part_at(&self, i: usize) -> usize {
        SECTION_HEADER
            + (self.solids + self.triggers) * SOLID_BYTES
            + self.meshes * MESH_BYTES
            + i * PART_BYTES
    }

    /// (parts, solids, triggers, meshes, u16s): the buffers
    /// [`decode`](Self::decode) needs.
    pub fn counts(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.parts,
            self.solids,
            self.triggers,
            self.meshes,
            self.u16s,
        )
    }

    /// Mesh record `i`: (cell, node index).
    pub fn mesh(&self, i: usize) -> (u8, u32) {
        let at = SECTION_HEADER + (self.solids + self.triggers) * SOLID_BYTES + i * MESH_BYTES;
        (self.data[at], get_u32(self.data, at + 4))
    }

    /// Decode into caller buffers (lengths from [`counts`](Self::counts) or
    /// longer): the parts, gridded exactly as the bake gridded them.
    #[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
    pub fn decode<'b>(
        &self,
        solids: &'b mut [Solid],
        triggers: &'b mut [Solid],
        u16s: &'b mut [u16],
        parts: &'b mut [CellPart<'b>],
    ) -> Result<&'b [CellPart<'b>], SectionError> {
        if solids.len() < self.solids
            || triggers.len() < self.triggers
            || u16s.len() < self.u16s
            || parts.len() < self.parts
        {
            return Err(SectionError::Buffer);
        }
        let d = self.data;
        let mut at = SECTION_HEADER;
        for s in solids[..self.solids].iter_mut() {
            *s = get_solid(d, at);
            at += SOLID_BYTES;
        }
        for s in triggers[..self.triggers].iter_mut() {
            *s = get_solid(d, at);
            at += SOLID_BYTES;
        }
        let table = self.part_at(self.parts);
        for (k, v) in u16s[..self.u16s].iter_mut().enumerate() {
            *v = get_u16(d, table + k * 2);
        }
        let (solids, triggers, u16s): (&'b [Solid], &'b [Solid], &'b [u16]) =
            (solids, triggers, u16s);
        for (i, part) in parts[..self.parts].iter_mut().enumerate() {
            let at = self.part_at(i);
            let (base, len) = (get_u32(d, at + 4) as usize, get_u32(d, at + 8) as usize);
            let (tb, tl) = (get_u32(d, at + 12) as usize, get_u32(d, at + 16) as usize);
            let grid = if d[at + 1] != 0 {
                let (st, rf, rl) = (
                    get_u32(d, at + 44) as usize,
                    get_u32(d, at + 48) as usize,
                    get_u32(d, at + 52) as usize,
                );
                Some(
                    StaticWorld::from_raw(
                        &solids[base..base + len],
                        get_f32(d, at + 32),
                        get_f32(d, at + 36),
                        get_f32(d, at + 28),
                        get_u16(d, at + 40) as usize,
                        get_u16(d, at + 42) as usize,
                        &u16s[st..rf],
                        &u16s[rf..rf + rl],
                    )
                    .ok_or(SectionError::BadGrid)?,
                )
            } else {
                None
            };
            *part = CellPart {
                cell: d[at],
                base: base as u16,
                grid,
                trigger_base: tb as u16,
                triggers: &triggers[tb..tb + tl],
            };
        }
        let parts: &'b [CellPart<'b>] = parts;
        Ok(&parts[..self.parts])
    }
}

/// A baked scene's physics (`BakedScene::view`, or a [`TscnView`] of a
/// grouped scene's blob) into caller buffers: the parts and the mesh
/// records `(cell, node)`. A v9 scene decodes its section (the grids as
/// baked); a v8 scene, or one baked without a section, gathers its nodes
/// and grids them here: the same parts either way. `solid_cells` and
/// `trigger_cells` are scratch for the second path.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn load_scene<'b>(
    scene: &TscnView,
    solids: &'b mut [Solid],
    solid_cells: &mut [u8],
    triggers: &'b mut [Solid],
    trigger_cells: &mut [u8],
    meshes: &mut [(u8, u32)],
    u16s: &'b mut [u16],
    parts: &'b mut [CellPart<'b>],
) -> Result<(&'b [CellPart<'b>], usize), SectionError> {
    if let Some(p) = scene.physics() {
        let (_, _, _, nm, _) = p.counts();
        if meshes.len() < nm {
            return Err(SectionError::Buffer);
        }
        for (i, m) in meshes[..nm].iter_mut().enumerate() {
            *m = p.mesh(i);
        }
        return Ok((p.decode(solids, triggers, u16s, parts)?, nm));
    }
    let g = gather_nodes(
        scene.node_count(),
        &|i| scene.collider(i),
        solids,
        solid_cells,
        triggers,
        trigger_cells,
        meshes,
    );
    if g.solids > solids.len().min(solid_cells.len())
        || g.triggers > triggers.len().min(trigger_cells.len())
        || g.meshes > meshes.len()
    {
        return Err(SectionError::Buffer);
    }
    let (solids, triggers): (&'b [Solid], &'b [Solid]) = (solids, triggers);
    let built = build_parts(
        &solids[..g.solids],
        &solid_cells[..g.solids],
        &triggers[..g.triggers],
        &trigger_cells[..g.triggers],
        0.0,
        u16s,
        parts,
    )
    .ok_or(SectionError::Buffer)?;
    Ok((built, g.meshes))
}
